"""Single-owner ACP feasibility adapter for the official local Codex app server."""

import argparse
import fcntl
import hashlib
import json
import os
import queue
import sqlite3
import subprocess
import sys
import threading
from collections import deque
from contextlib import contextmanager
from pathlib import Path
from uuid import UUID


def digest(data):
    return hashlib.sha256(data).hexdigest()


class WorkspaceChanged(ValueError):
    pass


class TurnCancelled(ValueError):
    pass


class Adapter:
    def __init__(
        self, binary, profile, directory, images, model, workspace_write=False
    ):
        self.profile, self.directory = profile.resolve(), directory.resolve()
        self.images, self.model = images, model
        self.workspace_write = workspace_write
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.db = sqlite3.connect(self.directory / "journal.sqlite")
        self.db.executescript("""
          CREATE TABLE IF NOT EXISTS sessions(sid TEXT PRIMARY KEY, binding TEXT, cwd TEXT);
          CREATE TABLE IF NOT EXISTS operations(sid TEXT, op TEXT, prompt_hash TEXT, turn TEXT, done TEXT, PRIMARY KEY(sid,op));
          CREATE TABLE IF NOT EXISTS stops(sid TEXT, op TEXT, reason TEXT, PRIMARY KEY(sid,op));
          CREATE TABLE IF NOT EXISTS callbacks(sid TEXT, turn TEXT, call TEXT, state TEXT, PRIMARY KEY(sid,turn,call));
          CREATE TABLE IF NOT EXISTS accepted_effects(sid TEXT, op TEXT, sha TEXT, PRIMARY KEY(sid,op));
          CREATE TABLE IF NOT EXISTS images(sid TEXT, op TEXT, item TEXT, sha TEXT, bytes BLOB, PRIMARY KEY(sid,item));
        """)
        env = {
            k: os.environ[k]
            for k in ("HOME", "PATH", "TMPDIR", "LANG")
            if k in os.environ
        }
        env["CODEX_HOME"] = str(self.profile)
        self.auth_file = self.profile / "auth.json"
        self.generation = self.auth_generation()
        self.incoming = queue.Queue(maxsize=128)
        self.deferred = deque()
        self.cancel = threading.Event()
        self.host = None
        args = [str(binary), "app-server", "-c", 'cli_auth_credentials_store="file"']
        for feature in ("image_generation", "view_image"):
            args.extend(["--enable" if images else "--disable", feature])
        self.server = subprocess.Popen(
            args, stdin=-1, stdout=-1, stderr=sys.stderr, text=True, env=env
        )

        def reader():
            try:
                for line in self.server.stdout:
                    self.incoming.put(json.loads(line))
            finally:
                self.incoming.put(None)

        threading.Thread(target=reader, daemon=True).start()
        self.seq = 0
        self.rpc(
            "initialize",
            {
                "clientInfo": {"name": "constellation_image_lab", "version": "0.1"},
                "capabilities": {"experimentalApi": True},
            },
        )
        self.send({"method": "initialized", "params": {}})
        self.loaded = None

    def close(self):
        self.server.terminate()
        try:
            self.server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.server.kill()
            self.server.wait()
        self.db.close()

    def auth_generation(self):
        info = self.auth_file.stat()
        return (
            info.st_dev,
            info.st_ino,
            info.st_size,
            info.st_mtime_ns,
            info.st_ctime_ns,
        )

    def guard(self):
        try:
            current = self.auth_generation()
        except OSError:
            current = None
        if current != self.generation:
            self.server.terminate()
            raise WorkspaceChanged(
                "Managed login changed. Reconnect and recheck the saved workspace."
            )

    def send(self, event):
        self.guard()
        self.server.stdin.write(json.dumps(event) + "\n")
        self.server.stdin.flush()

    def wire(self):
        while True:
            self.guard()
            try:
                msg = self.incoming.get(timeout=0.1)
            except queue.Empty:
                if self.active_cancel():
                    raise TurnCancelled()
                continue
            if msg is None:
                raise RuntimeError("Codex disconnected; reconcile the saved operation.")
            self.guard()
            return msg

    def active_cancel(self):
        return getattr(self, "interruptible", False) and self.cancel.is_set()

    def read(self):
        self.guard()
        if self.active_cancel():
            raise TurnCancelled()
        return self.deferred.popleft() if self.deferred else self.wire()

    def rpc(self, method, params):
        self.seq += 1
        self.send({"id": self.seq, "method": method, "params": params})
        while True:
            msg = self.wire()
            if "method" not in msg and msg.get("id") == self.seq:
                if "error" in msg:
                    raise ValueError(msg["error"]["message"])
                return msg["result"]
            if "method" in msg:
                self.deferred.append(msg)

    def binding(self):
        r = self.rpc("account/read", {})
        account = r.get("account") or {}
        if account.get("type") != "chatgpt" or not r.get("userId"):
            raise ValueError(
                "A managed ChatGPT login with a stable user ID is required."
            )
        sessions = self.rpc("accountSession/list", {})
        active = [
            s
            for s in sessions["sessions"]
            if s["isActive"] and s["sessionId"] == sessions["activeSessionId"]
        ]
        if (
            len(active) != 1
            or active[0].get("userId") != r["userId"]
            or not active[0].get("selectedWorkspaceAccountId")
        ):
            raise ValueError("Cannot identify the current ChatGPT workspace.")
        return digest(
            json.dumps(
                [
                    "workspace-v1",
                    r["userId"],
                    active[0]["selectedWorkspaceAccountId"],
                    self.model,
                    self.images,
                ]
                + (["workspace-write"] if self.workspace_write else [])
            ).encode()
        )

    def check(self, sid):
        row = self.db.execute(
            "select binding,cwd from sessions where sid=?", (sid,)
        ).fetchone()
        if row is None or self.binding() != row[0]:
            raise ValueError(
                "Account or route changed. Select the original account or start a new chat."
            )
        return row[1]

    @contextmanager
    def lease(self, sid):
        with (self.directory / (str(UUID(sid)) + ".lock")).open("a") as f:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
            yield

    def start(self, cwd):
        binding = self.binding()
        args = {
            "cwd": cwd,
            "approvalPolicy": "never",
            "sandbox": "workspace-write" if self.workspace_write else "read-only",
            "model": self.model,
        }
        if self.host:
            args["dynamicTools"] = self.host.definitions()
        sid = self.rpc("thread/start", args)["thread"]["id"]
        with self.db:
            self.db.execute("insert into sessions values(?,?,?)", (sid, binding, cwd))
        self.loaded = sid
        return sid

    def resume(self, sid, cwd):
        if self.check(sid) != cwd:
            raise ValueError("Workspace changed.")
        with self.lease(sid):
            t = self.rpc(
                "thread/resume",
                {
                    "threadId": sid,
                    "cwd": cwd,
                    "approvalPolicy": "never",
                    "sandbox": "workspace-write"
                    if self.workspace_write
                    else "read-only",
                },
            )["thread"]
            if t["id"] != sid:
                raise ValueError("Thread identity mismatch")
            self.loaded = sid
            return t

    def save_image(self, sid, op, item):
        if not self.images:
            raise ValueError("Image disabled")
        if item.get("status") != "completed":
            return
        root = (self.profile / "generated_images" / str(UUID(sid))).resolve()
        p = Path(item["savedPath"]).resolve(strict=True)
        if p.parent != root or p.stat().st_size > 32 * 1024 * 1024:
            raise ValueError("Unexpected image source")
        data = p.read_bytes()
        sha = digest(data)
        with self.db:
            self.db.execute(
                "insert or ignore into images values(?,?,?,?,?)",
                (sid, op, item["id"], sha, data),
            )
            self.guard()
            if self.db.execute(
                "select sha from images where sid=? and item=?", (sid, item["id"])
            ).fetchone() != (sha,):
                raise ValueError("Conflicting artifact identity")
        return sha

    def artifacts(self, sid, op):
        return [
            s
            for (s,) in self.db.execute(
                "select sha from images where sid=? and op=? order by rowid", (sid, op)
            )
        ]

    def prompt(self, sid, op, text, emit):
        op = str(UUID(op))
        with self.lease(sid):
            self.check(sid)
            if self.loaded != sid:
                raise ValueError("Load the chat first.")
            h = digest(text.encode())
            previous = self.db.execute(
                "select prompt_hash,turn,done from operations where sid=? and op=?",
                (sid, op),
            ).fetchone()
            if previous:
                if previous[0] != h:
                    raise ValueError("Operation ID reused with a different prompt")
                if self.db.execute(
                    "select 1 from stops where sid=? and op=?", (sid, op)
                ).fetchone():
                    raise TurnCancelled("Codex confirmed cancellation")
                if previous[2] is not None:
                    return previous[2], self.artifacts(sid, op)
                accepted = self.db.execute(
                    "select sha from accepted_effects where sid=? and op=?", (sid, op)
                ).fetchone()
                if accepted:
                    raise ValueError(
                        "Recovered artifact accepted; the interrupted turn was not replayed: "
                        + accepted[0]
                    )
                # Query the saved Codex turn. A known effect is never a reason to resend.
                thread = self.rpc(
                    "thread/read", {"threadId": sid, "includeTurns": True}
                )["thread"]
                turn = next(
                    (t for t in thread["turns"] if t["id"] == previous[1]), None
                )
                if turn:
                    for item in turn.get("items", []):
                        if item["type"] == "imageGeneration":
                            self.save_image(sid, op, item)
                arts = self.artifacts(sid, op)
                if not turn or turn["status"] != "completed":
                    raise ValueError(
                        "Turn outcome uncertain; no retry sent. Saved artifacts: "
                        + ",".join(arts)
                    )
                answer = "\n".join(
                    i["text"] for i in turn["items"] if i["type"] == "agentMessage"
                )
            else:
                if self.db.execute(
                    "select 1 from operations o where sid=? and done is null and not exists (select 1 from stops s where s.sid=o.sid and s.op=o.op) and not exists (select 1 from accepted_effects a where a.sid=o.sid and a.op=o.op)",
                    (sid,),
                ).fetchone():
                    raise ValueError(
                        "Reconcile the outstanding operation before starting another."
                    )
                with self.db:
                    self.db.execute(
                        "insert into operations values(?,?,?,null,null)", (sid, op, h)
                    )
                t = self.rpc(
                    "turn/start",
                    {"threadId": sid, "input": [{"type": "text", "text": text}]},
                )["turn"]["id"]
                with self.db:
                    self.db.execute(
                        "update operations set turn=? where sid=? and op=?",
                        (t, sid, op),
                    )
                chunks = []
                self.interruptible = True
                interrupted = False
                try:
                    while True:
                        try:
                            m = self.read()
                            p = m.get("params", {})
                            if "id" in m:
                                if (
                                    m.get("method") == "item/tool/call"
                                    and p.get("threadId") == sid
                                    and p.get("turnId") == t
                                    and self.host
                                    and not interrupted
                                ):
                                    key = (sid, t, p["callId"])
                                    with self.db:
                                        inserted = self.db.execute(
                                            "insert or ignore into callbacks values(?,?,?,'issued')",
                                            key,
                                        ).rowcount
                                    if not inserted:
                                        raise ValueError(
                                            "Duplicate host callback; previous outcome retained"
                                        )

                                    def check_callback():
                                        self.guard()
                                        if self.cancel.is_set():
                                            raise TurnCancelled()

                                    try:
                                        result = self.host.call(
                                            sid,
                                            self.check(sid),
                                            p["tool"],
                                            p["arguments"],
                                            check_callback,
                                        )
                                    except (WorkspaceChanged, TurnCancelled):
                                        raise
                                    except (KeyError, ValueError, TypeError) as exc:
                                        result = {
                                            "success": False,
                                            "contentItems": [
                                                {"type": "inputText", "text": str(exc)}
                                            ],
                                        }
                                    check_callback()
                                    with self.db:
                                        self.db.execute(
                                            "update callbacks set state='answered' where sid=? and turn=? and call=?",
                                            key,
                                        )
                                    self.send({"id": m["id"], "result": result})
                                else:
                                    self.send(
                                        {
                                            "id": m["id"],
                                            "error": {
                                                "code": -32601,
                                                "message": "Unsupported callback",
                                            },
                                        }
                                    )
                                continue
                            if p.get("threadId") != sid:
                                continue
                            if (
                                m.get("method") == "item/completed"
                                and p.get("turnId") == t
                            ):
                                i = p["item"]
                                if i["type"] == "imageGeneration":
                                    sha = self.save_image(sid, op, i)
                                    if sha:
                                        emit("Stored original bytes: " + sha)
                                elif i["type"] == "agentMessage":
                                    chunks.append(i["text"])
                            if (
                                m.get("method") == "turn/completed"
                                and p["turn"]["id"] == t
                            ):
                                if p["turn"]["status"] == "interrupted":
                                    with self.db:
                                        self.db.execute(
                                            "insert or ignore into stops values(?,?,'cancelled')",
                                            (sid, op),
                                        )
                                    raise TurnCancelled("Codex confirmed cancellation")
                                if p["turn"]["status"] != "completed":
                                    raise ValueError(
                                        "Turn outcome uncertain; reconcile before retrying."
                                    )
                                break
                        except TurnCancelled as exc:
                            if str(exc):
                                raise
                            self.interruptible = False
                            interrupted = True
                            self.rpc("turn/interrupt", {"threadId": sid, "turnId": t})
                finally:
                    self.interruptible = False
                answer = "\n".join(chunks)
            with self.db:
                self.db.execute(
                    "update operations set done=? where sid=? and op=?",
                    (answer, sid, op),
                )
            return answer, self.artifacts(sid, op)

    def accept_effect(self, sid, op, sha):
        """Owner accepts a recovered output. Never mark an interrupted turn completed."""
        with self.lease(sid):
            self.original(sid, sha)
            row = self.db.execute(
                "select done from operations where sid=? and op=?", (sid, op)
            ).fetchone()
            if row != (None,) or sha not in self.artifacts(sid, op):
                raise ValueError(
                    "Select an original output of the interrupted operation."
                )
            thread = self.rpc("thread/read", {"threadId": sid, "includeTurns": False})[
                "thread"
            ]
            if thread["status"]["type"] not in ("idle", "notLoaded"):
                raise ValueError("The Codex thread is still active or unavailable.")
            with self.db:
                self.db.execute(
                    "insert or ignore into accepted_effects values(?,?,?)",
                    (sid, op, sha),
                )

    def original(self, sid, sha):
        self.check(sid)
        row = self.db.execute(
            "select bytes from images where sid=? and sha=?", (sid, sha)
        ).fetchone()
        if not row or digest(row[0]) != sha:
            raise ValueError("Artifact missing or corrupt")
        self.guard()
        return row[0]


def serve(a):
    def out(msg):
        print(json.dumps({"jsonrpc": "2.0", **msg}), flush=True)

    from host import Host

    inbox = queue.Queue()
    output_lock = threading.Lock()
    raw_out = out

    def serialized(msg):
        with output_lock:
            raw_out(msg)

    out = serialized
    a.host = Host(out)
    state_lock = threading.Lock()
    active = [None]

    def reader():
        try:
            for line in sys.stdin:
                message = json.loads(line)
                if "method" not in message:
                    a.host.response(message)
                elif message["method"] == "session/cancel":
                    with state_lock:
                        if (
                            active[0] is not None
                            and message.get("params", {}).get("sessionId") == active[0]
                        ):
                            a.cancel.set()
                else:
                    with state_lock:
                        if active[0] is not None:
                            if "id" in message:
                                out(
                                    {
                                        "id": message["id"],
                                        "error": {
                                            "code": -32000,
                                            "message": "A request is active",
                                        },
                                    }
                                )
                            continue
                        if message["method"] == "session/prompt":
                            a.cancel.clear()
                            active[0] = message.get("params", {}).get("sessionId")
                    inbox.put(message)
        finally:
            a.cancel.set()
            inbox.put(None)

    threading.Thread(target=reader, daemon=True).start()
    while (r := inbox.get()) is not None:
        m = r["method"]
        p = r.get("params", {})
        sid = p.get("sessionId")

        def update(text, kind="agent_message_chunk", session_id=sid):
            a.guard()
            out(
                {
                    "method": "session/update",
                    "params": {
                        "sessionId": session_id,
                        "update": {
                            "sessionUpdate": kind,
                            "content": {"type": "text", "text": text},
                        },
                    },
                }
            )

        def finish(payload, method=m, request=r):
            if "id" in request:
                with state_lock:
                    if method == "session/prompt":
                        active[0] = None
                    out({"id": request["id"], **payload})

        try:
            if m == "initialize":
                a.host.configure(p.get("clientCapabilities", {}))
                res = {
                    "protocolVersion": 1,
                    "agentInfo": {"name": "Official Codex prototype", "version": "0.1"},
                    "agentCapabilities": {"loadSession": True},
                    "authMethods": [],
                }
            elif m in ("session/new", "session/load"):
                if p.get("mcpServers"):
                    raise ValueError("Host MCP tools are outside this feasibility test")
                if m == "session/new":
                    res = {"sessionId": a.start(p["cwd"])}
                else:
                    t = a.resume(sid, p["cwd"])
                    for turn in t.get("turns", []):
                        for item in turn.get("items", []):
                            if item["type"] == "agentMessage":
                                update(item["text"])
                            elif item["type"] == "userMessage":
                                for block in item["content"]:
                                    if block["type"] == "text":
                                        update(block["text"], "user_message_chunk")
                    res = {}
            elif m == "session/prompt":
                if any(i["type"] != "text" for i in p["prompt"]):
                    raise ValueError("This prototype accepts text prompts")
                text = "\n".join(i["text"] for i in p["prompt"])
                answer, arts = a.prompt(sid, p["_meta"]["operationId"], text, update)
                update(answer)
                res = {"stopReason": "end_turn", "_meta": {"artifacts": arts}}
            else:
                raise ValueError("Unsupported ACP method")
            a.guard()
            finish({"result": res})
        except TurnCancelled:
            finish({"result": {"stopReason": "cancelled"}})
        except (ValueError, KeyError, OSError, RuntimeError, sqlite3.Error) as e:
            finish({"error": {"code": -32000, "message": str(e)}})


if __name__ == "__main__":
    os.umask(0o077)
    ap = argparse.ArgumentParser()
    for key in ("binary", "profile", "directory"):
        ap.add_argument("--" + key, type=Path, required=True)
    ap.add_argument("--workspace-write", action="store_true")
    ap.add_argument("--images", action="store_true")
    ap.add_argument("--model", default="gpt-6.2-sol")
    a = Adapter(**vars(ap.parse_args()))
    try:
        serve(a)
    finally:
        a.close()
