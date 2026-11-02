#!/usr/bin/env python3
"""Real TLS service -> stdio attach -> official SDK interoperability smoke test.

Run after cargo build and npm ci --prefix tests/acp-client. No installed agents,
PAM accounts, Slack apps, or existing configuration are changed.
"""
import argparse
import json
import os
import pathlib
import select
import signal
import ssl
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

REPO = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("--binary", type=pathlib.Path, default=REPO / "target/debug/agent-instance")
parser.add_argument("--custom-certificate", action="store_true", help="Use a different self-signed certificate at a configured external path")
args = parser.parse_args()
binary = args.binary.resolve()

with tempfile.TemporaryDirectory(prefix="agent-acp-wss-") as directory:
    home = pathlib.Path(directory) / "instance"
    subprocess.run([str(binary), "init", "--home", str(home), "--name", "ACP interoperability test", "--bind", "127.0.0.1"], check=True, stdout=subprocess.DEVNULL)
    document = json.loads((home / "config.json").read_text())
    config = document["config"]
    config.update(enabled=True, setup_complete=True, purpose="Independent interoperability check")
    config["backend"] = {
        "command": "python3", "args": [str(REPO / "tests/fixtures/acp_backend.py"), str(home / "fixture-state")],
        "env": {}, "secret_env": {}, "working_directory": str(home / "workspace"),
        "allowed_working_roots": [str(home / "workspace")],
    }
    certificate = home / "tls/cert.pem"
    trust = home / "tls/root-ca.crt" if (home / "tls/root-ca.crt").exists() else certificate
    if args.custom_certificate:
        custom = pathlib.Path(directory) / "custom-certificate-source"
        custom.mkdir(mode=0o700)
        # Exercise an operator's independent self-signed deployment, not a
        # second managed local CA that could mask incorrect trust selection.
        certificate = custom / "cert.pem"
        key = custom / "key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256",
                        "-days", "2", "-noenc", "-subj", "/CN=ACP interoperability test",
                        "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1",
                        "-addext", "basicConstraints=critical,CA:FALSE",
                        "-addext", "keyUsage=critical,digitalSignature",
                        "-addext", "extendedKeyUsage=serverAuth",
                        "-keyout", str(key), "-out", str(certificate)],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        assert certificate.read_bytes() != (home / "tls/cert.pem").read_bytes()
        config["server"]["certificate_file"] = str(certificate)
        config["server"]["private_key_file"] = str(key)
        trust = certificate
    (home / "config.json").write_text(json.dumps(document))
    tls = ssl.create_default_context(cafile=str(trust))
    with (home / "server-test.log").open("w") as log:
        server = subprocess.Popen([str(binary), "serve", "--home", str(home)], stdout=log, stderr=log)
        attach = None
        try:
            deadline = time.monotonic() + 20
            base = None
            while time.monotonic() < deadline:
                if server.poll() is not None:
                    raise RuntimeError("Service failed: " + (home / "server-test.log").read_text())
                try:
                    base = json.loads((home / "endpoints.json").read_text())["base_url"]
                    urllib.request.urlopen(base, context=tls, timeout=1).close()
                    break
                except (FileNotFoundError, urllib.error.URLError, ConnectionError):
                    time.sleep(0.05)
            else:
                raise RuntimeError("Service did not become reachable")

            def api(path, data=None, token=None):
                headers = {"Content-Type": "application/json"}
                if token:
                    headers["Authorization"] = "Bearer " + token
                request = urllib.request.Request(base + "/api/v1/" + path, data=json.dumps(data).encode() if data is not None else None, headers=headers)
                with urllib.request.urlopen(request, context=tls, timeout=5) as response:
                    return json.load(response)

            try:
                api("status")
                raise AssertionError("Unauthenticated management access was accepted")
            except urllib.error.HTTPError as error:
                assert error.code == 401, error.code

            sdk = subprocess.run([os.environ.get("AGENT_INSTANCE_TEST_NODE", "node"), str(REPO / "tests/acp-client/official-client.mjs"), str(home / "workspace"), str(binary), "acp", "--home", str(home)], capture_output=True, text=True, timeout=40)
            if sdk.returncode:
                raise RuntimeError("Official SDK test failed:\n" + sdk.stderr)
            report = json.loads(sdk.stdout)
            assert report["success"] is True

            # Keep stdin open while the remote service ends the connection. The
            # attach process must exit without waiting forever for another line.
            attach = subprocess.Popen([str(binary), "acp", "--home", str(home)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            attach.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": 1, "clientCapabilities": {}}}) + "\n")
            attach.stdin.flush()
            ready, _, _ = select.select([attach.stdout], [], [], 5)
            assert ready, "Attach initialization timed out"
            assert json.loads(attach.stdout.readline())["result"]["protocolVersion"] == 1
            token = (home / "secrets/control-token").read_text().strip()
            api("lifecycle", {"action": "stop"}, token)
            attach.wait(timeout=5)
            assert not attach.stdin.closed, "Test inadvertently closed stdin"
            print(json.dumps({**report, "unauthenticated_access_denied": True, "remote_stop_with_open_stdin_exits": True, "custom_certificate": args.custom_certificate, "transport": "agent-instance.wss/1"}))
        finally:
            if attach and attach.poll() is None:
                attach.kill()
                attach.wait()
            server.send_signal(signal.SIGTERM)
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
