"""Live ACP host callback, cancellation and optional disposable workspace-switch test."""

import argparse
import json
import sqlite3
import subprocess
import uuid
from pathlib import Path

from adapter import Adapter

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("mode", choices=["host", "switch"])
parser.add_argument("--cwd", type=Path, required=True)
for option in ["binary", "profile", "directory"]:
    parser.add_argument("--" + option, type=Path, required=True)
parser.add_argument("command", nargs="+", help="ACP client command after --")
args = parser.parse_args()


def client():
    return subprocess.Popen(args.command, stdin=-1, stdout=-1, text=True)


class Control(Adapter):
    def guard(self):
        pass  # sole test driver authorized to switch the disposable login


def workspace(target=None):
    a = Control(args.binary, args.profile, args.directory, False, "gpt-6.2-sol")
    try:
        s = next(
            x
            for x in a.rpc("accountSession/list", {"refreshWorkspaceMetadata": True})[
                "sessions"
            ]
            if x["isActive"]
        )
        if target:
            a.rpc(
                "accountSession/switch",
                {
                    "sessionId": s["sessionId"],
                    "accountId": target,
                    "allowBrowserLogin": False,
                },
            )
        return s["selectedWorkspaceAccountId"], [
            x["accountId"] for x in s["workspaces"]
        ]
    finally:
        a.close()


mode = args.mode
p = client()


def send(**msg):
    p.stdin.write(json.dumps({"jsonrpc": "2.0", **msg}) + "\n")
    p.stdin.flush()


def call(i, m, **args):
    send(id=i, method=m, params=args)


def wait(i):
    global held
    while line := p.stdout.readline():
        e = json.loads(line)
        if e.get("id") == i and not e.get("method"):
            return e
        if e.get("method", "").startswith("fs/"):
            assert e["params"]["sessionId"] == sid
            held = e["id"]
            if i == 3:
                assert e["params"]["path"] == str(args.cwd.resolve() / "ink.txt")
                send(id=held, result={"content": nonce})
            elif i == 4:
                assert e["params"]["path"] == str(args.cwd.resolve() / "proof.txt")
                assert e["params"]["content"] == nonce
                send(id=held, result={})
            elif i == 5:
                if mode == "switch":
                    workspace(other)
                    assert workspace()[0] == other
                else:
                    send(method="session/cancel", params={"sessionId": sid})
                print("Pending callback reached; " + mode, flush=True)
            else:
                raise AssertionError("Unexpected callback")
            events.append(e["method"])
        if e.get("method") == "session/update":
            messages.append(e["params"]["update"].get("content", {}).get("text", ""))
    raise RuntimeError("closed")


def prompt(i, text, op=None):
    call(
        i,
        "session/prompt",
        sessionId=sid,
        prompt=[{"type": "text", "text": text}],
        _meta={"operationId": op or str(uuid.uuid4())},
    )
    return wait(i)


def connect():
    call(
        1,
        "initialize",
        protocolVersion=1,
        clientCapabilities={"fs": {"readTextFile": True, "writeTextFile": True}},
    )
    assert "result" in wait(1)


events = []
messages = []
nonce = uuid.uuid4().hex
if mode == "switch":
    base, options = workspace()
    other = next(w for w in options if w != base)
try:
    connect()
    call(2, "session/new", cwd=str(args.cwd.resolve()), mcpServers=[])
    sid = wait(2)["result"]["sessionId"]
    if mode != "switch":
        r = prompt(
            3,
            "Use the host_read_text tool once for ink.txt. Return exactly the text from the host.",
        )
        assert r["result"]["stopReason"] == "end_turn" and nonce in "".join(messages), (
            r,
            messages,
            events,
        )
        r = prompt(
            4,
            "Call the host_write_text tool now for the host-managed virtual file proof.txt with the exact previous text. The host explicitly grants the write. Other tools cannot write this host document.",
        )
        assert r["result"]["stopReason"] == "end_turn" and events == [
            "fs/read_text_file",
            "fs/write_text_file",
        ], (r, events, messages)
        print("Host read and write passed", flush=True)
    op = str(uuid.uuid4())
    t = "Call host_read_text once with path pending.txt. Return its contents."
    r = prompt(5, t, op)

    assert events[-1] == "fs/read_text_file"
    if mode == "switch":
        assert "Managed login changed" in r["error"]["message"]
    else:
        assert r["result"]["stopReason"] == "cancelled"
    send(id=held, result={"content": "LATE-" + nonce})
    if mode != "switch":
        assert prompt(6, t, op)["result"]["stopReason"] == "cancelled"
        assert (
            prompt(7, "Reply exactly READY, without tools.")["result"]["stopReason"]
            == "end_turn"
        )
    else:
        p.terminate()
        p.wait()
        p = client()
        connect()
        call(
            8, "session/load", cwd=str(args.cwd.resolve()), mcpServers=[], sessionId=sid
        )
        assert "Account or route changed" in wait(8)["error"]["message"]
    with sqlite3.connect(args.directory / "journal.sqlite") as db:
        states = db.execute(
            "SELECT c.state FROM callbacks c JOIN operations o ON c.sid=o.sid AND c.turn=o.turn WHERE o.sid=? AND o.op=?",
            (sid, op),
        ).fetchall()
        assert states == [("issued",)], states
    print(mode, "passed", flush=True)
finally:
    p.terminate()
    p.wait()
    if mode == "switch":
        workspace(base)
        assert workspace()[0] == base
        print("Workspace restored", flush=True)
