#!/usr/bin/env python3
"""Opt-in checks for an already installed DISPOSABLE native test instance.

No model calls or owner password are needed. This changes only the specified test
instance: a profile, setup/enabled state, and a browser view. It never seeds auth.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import ssl
import subprocess
import threading
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--expected-engine", choices=["native", "codex"], default="native")
    parser.add_argument("--allow-disposable-mutations", action="store_true", required=True)
    args = parser.parse_args()
    home = args.home.resolve()
    binary = home / "bin" / ("agent-instance.exe" if os.name == "nt" else "agent-instance")
    endpoints = json.loads((home / "endpoints.json").read_text())
    token = (home / "secrets/control-token").read_text().strip()
    context = ssl.create_default_context(cafile=str(home / "tls/root-ca.crt"))
    checks = []

    def request(path, body=None, method=None, authenticated=True):
        headers = {"Content-Type": "application/json"}
        if authenticated:
            headers["Authorization"] = "Bearer " + token
        req = urllib.request.Request(endpoints["base_url"] + "/api/v1/" + path,
            data=None if body is None else json.dumps(body).encode(), headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, context=context, timeout=20) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    code, status = request("status")
    assert code == 200 and status["authentication"] == ("windows_logon" if os.name == "nt" else "pam")
    checks.append("Native HTTPS with the instance CA and scoped token")
    assert request("status", authenticated=False)[0] == 401
    # Wrong owner is refused before OS authentication, avoiding attempts against
    # a real domain account or changing its account lockout state.
    assert request("auth/login", {"username": "not-this-instance-owner", "password": "fixture-only"}, authenticated=False)[0] == 401
    checks.append("Unauthenticated management and wrong-owner sign-in rejected")
    code, backend = request("backend")
    assert code == 200 and backend["installed"] and backend["kind"] == args.expected_engine
    if args.expected_engine == "native":
        assert not (home / "backend/runtime").exists(), "Native setup must not install Codex/Node"
        receipt = json.loads((home / "engine/installation.json").read_text())
        assert receipt["implementation"] == "Constellation" and receipt["credentials_copied"] is False
    checks.append("Pinned native default runtime installed")
    assert request("profiles/communication", {"text": "Use concrete examples. Native lifecycle persistence fixture."}, "PUT")[0] == 200
    assert request("views", {}, "POST")[0] == 200
    checks.append("Profile and view writes persisted through the normal management API")
    _, document = request("config")
    document["config"]["setup_complete"] = True
    document["config"]["enabled"] = True
    assert request("config", {"expected_revision": document["revision"], "config": document["config"]}, "PUT")[0] == 200
    child = subprocess.Popen([str(binary), "acp", "--home", str(home)], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    received = queue.Queue()
    def read():
        for line in child.stdout:
            received.put(json.loads(line))
    threading.Thread(target=read, daemon=True).start()
    try:
        child.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": 1, "clientCapabilities": {}, "clientInfo": {"name": "native-validation", "version": "1"}}}) + "\n")
        child.stdin.flush()
        response = received.get(timeout=30)
        while response.get("id") != 1:
            response = received.get(timeout=30)
        assert "result" in response, response
        assert response["result"]["protocolVersion"] == 1
        checks.append(f"Installed {args.expected_engine} ACP initializes through the instance stdio attach boundary")
    finally:
        child.stdin.close()
        try:
            child.wait(timeout=8)
        except subprocess.TimeoutExpired:
            child.terminate(); child.wait(timeout=8)
    print(json.dumps({"instance_id": status["instance_id"], "version": status["version"], "checks": checks,
                      "model_call": False, "real_owner_password_login": "not tested"}, indent=2))


if __name__ == "__main__":
    main()
