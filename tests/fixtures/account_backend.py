#!/usr/bin/python3
"""A private app-server account fixture. Never connects to a real provider."""
import json
import os
from pathlib import Path
import sys
import threading
import time

provider = Path(os.environ["CODEX_HOME"])
(provider / "fixture-pid").write_text(str(os.getpid()))
with (provider / "fixture-starts").open("a") as counter:
    counter.write("start\n")


def send(value):
    print(json.dumps(value), flush=True)


def complete():
    # The test releases completion only after checking the pending login twice.
    # A fixed sleep lets loaded CI runners complete before that assertion.
    deadline = time.monotonic() + 30
    while not (provider / "fixture-complete").exists():
        if time.monotonic() >= deadline:
            return
        time.sleep(0.01)
    (provider / "auth.json").write_text('{"tokens":{"access_token":"fixture-only-private-token"}}')
    send({"method": "account/login/completed", "params": {"loginId": "fixture-login", "success": True}})


for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        send({"id": request["id"], "result": {"userAgent": "fixture"}})
    elif method == "account/read":
        send({"id": request["id"], "result": {"account": None, "requiresOpenaiAuth": True}})
    elif method == "account/login/start":
        send({"id": request["id"], "result": {"type": "chatgptDeviceCode", "loginId": "fixture-login", "verificationUrl": "https://auth.openai.com/codex/device", "userCode": "ABCD-1234"}})
        threading.Thread(target=complete, daemon=True).start()
