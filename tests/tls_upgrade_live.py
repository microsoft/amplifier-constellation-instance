#!/usr/bin/env python3
"""Disposable managed-CA upgrade regression against actual historical binaries."""
import argparse
import hashlib
import json
import ssl
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def exercise(legacy, faulty, candidate, insert_fault, candidate_version):
    with tempfile.TemporaryDirectory(prefix="agent-instance-tls-upgrade-") as folder:
        home = Path(folder)
        subprocess.run([str(legacy), "--home", folder, "init", "--name", "TLS upgrade fixture", "--bind", "127.0.0.1", "--port", "0"], check=True, capture_output=True)
        root = (home / "tls/root-ca.crt").read_bytes()
        key = (home / "tls/root-ca.key").read_bytes()
        original_config = json.loads((home / "config.json").read_text())
        # Model an existing copied CA with a new agent identity. Only this
        # disposable folder is changed; the production sources are never read.
        config = json.loads(json.dumps(original_config))
        config["config"]["instance_id"] = str(uuid.uuid4())
        (home / "config.json").write_text(json.dumps(config))
        ctx = ssl.create_default_context(cafile=str(home / "tls/root-ca.crt"))
        token = (home / "secrets/control-token").read_text().strip()
        process = None

        def start(binary):
            nonlocal process
            (home / "endpoints.json").unlink(missing_ok=True)
            process = subprocess.Popen([str(binary), "--home", folder, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            for _ in range(200):
                if process.poll() is not None:
                    raise AssertionError(process.stderr.read())
                try:
                    endpoint = json.loads((home / "endpoints.json").read_text())
                    if endpoint["pid"] == process.pid:
                        return endpoint["base_url"]
                except (FileNotFoundError, json.JSONDecodeError):
                    pass
                time.sleep(0.05)
            raise AssertionError("Temporary service did not start")

        def stop():
            nonlocal process
            if process is not None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
                process.stderr.close()
                process = None

        def status(url, context):
            request = urllib.request.Request(url + "/api/v1/status", headers={"Authorization": "Bearer " + token})
            with urllib.request.urlopen(request, context=context, timeout=5) as response:
                return json.load(response)

        try:
            url = start(legacy)
            assert status(url, ctx)["version"] == "0.6.0"
            stop()
            if insert_fault:
                url = start(faulty)
                try:
                    status(url, ctx)
                    raise AssertionError("Faulty renewal unexpectedly retained a valid chain")
                except urllib.error.URLError as error:
                    assert isinstance(error.reason, ssl.SSLCertVerificationError), error
                marker = json.loads((home / "tls/managed.json").read_text())
                assert marker["leaf_profile"] == 2
                stop()
            url = start(candidate)
            result = status(url, ctx)  # Context cached before either upgrade.
            assert result["version"] == candidate_version, result["version"]
            strict = ssl.create_default_context(cafile=str(home / "tls/root-ca.crt"))
            strict.verify_flags |= ssl.VERIFY_X509_STRICT
            assert status(url, strict)["version"] == candidate_version
            with urllib.request.urlopen(url + "/welcome/info", context=strict, timeout=5) as response:
                help_info = json.load(response)
            original_id = original_config["config"]["instance_id"]
            assert help_info["certificate_name"] == "Agent Instance " + original_id[:8] + " Local CA"
            assert (home / "tls/root-ca.crt").read_bytes() == root
            assert (home / "tls/root-ca.key").read_bytes() == key
            marker = json.loads((home / "tls/managed.json").read_text())
            assert marker["leaf_profile"] == 3
            assert json.loads((home / "config.json").read_text())["config"]["instance_id"] == config["config"]["instance_id"]
            return {"scenario": "repair_0.7.1_invalid_leaf" if insert_fault else "direct_0.6_upgrade", "passed": True, "cached_context_https": True, "strict_context_https": True, "ca_certificate_and_key_unchanged": True, "instance_identity_unchanged": True, "certificate_help_name_matches_retained_root": True, "leaf_profile": 3}
        finally:
            stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--legacy-binary", type=Path, required=True, help="Actual agent-instance 0.6.0 binary")
    parser.add_argument("--faulty-binary", type=Path, required=True, help="Actual agent-instance 0.7.1 binary")
    parser.add_argument("--candidate", type=Path, required=True, help="Candidate agent-instance binary containing the renewal repair")
    args = parser.parse_args()
    binaries = {key: value.resolve() for key, value in vars(args).items()}
    candidate_version = subprocess.check_output([str(binaries["candidate"]), "--version"], text=True).strip().removeprefix("agent-instance ")
    expected = {"legacy_binary": "0.6.0", "faulty_binary": "0.7.1", "candidate": candidate_version}
    for key, path in binaries.items():
        assert subprocess.check_output([str(path), "--version"], text=True).strip() == "agent-instance " + expected[key]
    checks = [exercise(binaries["legacy_binary"], binaries["faulty_binary"], binaries["candidate"], fault, candidate_version) for fault in (False, True)]
    print(json.dumps({"passed": True, "checks": checks, "binaries": {key: {"path": str(path), "sha256": digest(path), "version": expected[key]} for key, path in binaries.items()}, "existing_services_changed": False, "temporary_services_and_instances_removed": True}))


if __name__ == "__main__":
    main()
