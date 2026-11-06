#!/usr/bin/env python3
"""Independent stdlib-only ACP peer used to test the Rust boundary.

The fixture owns session persistence and replay. It does not import the wrapper,
and intentionally emits callbacks with IDs that overlap client request IDs.
"""
import json
import os
import pathlib
import subprocess
import sys
import threading
import time
import uuid

HOME = pathlib.Path(sys.argv[1])
HOME.mkdir(parents=True, exist_ok=True)
OUT_LOCK = threading.Lock()
LOCK = threading.RLock()
sessions = {}
active = set()
cancel = {}
callbacks = {}
running = {}
steering_receipts = {}
held_steering = {}
initialized = False


def emit(value):
    with OUT_LOCK:
        print(json.dumps(value, separators=(",", ":")), flush=True)


def answer(req, result):
    emit({"jsonrpc": "2.0", "id": req["id"], "result": result})


def fail(req, code, message):
    if "id" in req:
        emit({"jsonrpc": "2.0", "id": req["id"], "error": {"code": code, "message": message}})


def path(sid):
    return HOME / (sid + ".json")


def persist(sid):
    path(sid).write_text(json.dumps(sessions[sid]))


def update(sid, text, kind="agent_message_chunk", message_id=None):
    entry = {"sessionUpdate": kind, "content": {"type": "text", "text": text}, "messageId": message_id or uuid.uuid4().hex}
    emit({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": entry}})
    return entry


def prompt(req):
    params = req["params"]
    sid = params["sessionId"]
    values = params["prompt"]
    text = "\n".join(v.get("text", "") for v in values)
    if "OVERSIZE_BACKEND_FRAME" in text:
        update(sid, "x" * 16384)
        return
    if text == "/compact":
        # A slash command is deterministic; decoration would bypass this branch.
        done=threading.Event();callbacks["maintenance-permission"]={"event":done}
        emit({"jsonrpc":"2.0","id":"maintenance-permission","method":"session/request_permission","params":{"sessionId":sid,"options":[{"optionId":"yes","kind":"allow_once","name":"Allow"}],"toolCall":{"toolCallId":"forbidden","title":"Write during maintenance"}}})
        done.wait(4)
        (HOME/"maintenance-callback.json").write_text(json.dumps(callbacks["maintenance-permission"].get("response")))
        compaction_id = uuid.uuid4().hex
        for entry in [
            {"sessionUpdate": "compaction_summary_chunk", "compactionId": compaction_id,
             "content": {"type": "text", "text": "Do not turn this summary into a final answer"}},
            {"sessionUpdate": "compaction_update", "compactionId": compaction_id,
             "status": "completed"},
        ]:
            emit({"jsonrpc": "2.0", "method": "session/update",
                  "params": {"sessionId": sid, "update": entry}})
        complete_prompt(req,{"stopReason":"end_turn","exactPrompt":values})
        return
    event = threading.Event()
    cancel[sid] = event
    if "SPAWN_CHILD" in text:
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)"])
        (HOME / "descendant.pid").write_text(str(child.pid))
    if "WAIT_FOR_CANCEL" in text or "SPAWN_CHILD" in text:
        update(sid, "Working")
        event.wait(30)
        complete_prompt(req, {"stopReason": "cancelled" if event.is_set() else "end_turn"})
        return
    detail = {"cwd": sessions[sid]["cwd"], "types": [v["type"] for v in values],
              "configured_context": values[0].get("text", "").startswith("[Configured agent instance context]"),
              "mcp_servers": sessions[sid]["mcp_names"],
              "secret_present": os.getenv("FIXTURE_SECRET") == "fixture-secret-canary",
              "slack_env_present": "SLACK_BOT_TOKEN" in os.environ,
              "backend_home": os.getenv("HOME"), "xdg_config_home": os.getenv("XDG_CONFIG_HOME"),
              "echo": text, "block_meta": [v.get("_meta") for v in values]}
    if "REQUEST_PERMISSION" in text:
        done = threading.Event()
        callbacks["1"] = {"event": done}
        emit({"jsonrpc": "2.0", "id": 1, "method": "session/request_permission", "params": {
            "sessionId": sid, "toolCall": {"toolCallId": "tool1", "title": "Fixture operation"},
            "options": [{"optionId": "permit", "kind": "allow_once", "name": "Allow fixture"}, {"optionId": "reject", "kind": "reject_once", "name": "Reject fixture"}]}})
        done.wait(10)
        detail["permission"] = callbacks.pop("1", {}).get("response", {})
    if "REQUEST_FILE" in text:
        done = threading.Event()
        callbacks["file-callback"] = {"event": done}
        emit({"jsonrpc": "2.0", "id": "file-callback", "method": "fs/read_text_file", "params": {"sessionId": sid, "path": sessions[sid]["cwd"] + "/input.txt"}})
        done.wait(10)
        detail["file_result"] = callbacks.pop("file-callback", {}).get("response", {})
    result = json.dumps(detail)
    with LOCK:
        sessions[sid]["history"].append({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": text}, "messageId": uuid.uuid4().hex})
        sessions[sid]["history"].append(update(sid, result))
        persist(sid)
    complete_prompt(req, {"stopReason": "end_turn"})


def complete_prompt(req, result):
    sid = req["params"]["sessionId"]
    with LOCK:
        receipts = steering_receipts.pop(sid, [])
        running.pop(sid, None)
    if receipts:
        result["_meta"] = {"io.github.bkrabach.agent-runtime": {"inputReceipts": receipts}}
    answer(req, result)


for line in sys.stdin:
    try:
        req = json.loads(line)
        method = req.get("method")
        params = req.get("params", {})
        if method is None:
            entry = callbacks.get(str(req.get("id")))
            if entry:
                entry["response"] = req.get("result", req.get("error"))
                entry["event"].set()
            continue
        if method == "initialize":
            initialized = True
            answer(req, {"protocolVersion": 1, "agentInfo": {"name": "independent-python-fixture", "version": "1.0"},
                         "agentCapabilities": {"loadSession": True, "sessionCapabilities": {"list": {}, "resume": {}, "close": {}}},
                         "authMethods": [{"id": "fixture-login", "name": "Fixture login"}],
                         "_meta": {"fixture-preserved": True, "com.microsoft.amplifier.context-snapshot": {"version":1},
                                   "steering": {"supported": os.getenv("FIXTURE_NO_STEERING") != "1", "version": 1, "idleBehavior": ["promptRequired"]}}})
            continue
        if not initialized:
            fail(req, -32600, "Initialize first")
            continue
        if method == "authenticate":
            answer(req, {}) if params.get("methodId") == "fixture-login" else fail(req, -32602, "Unknown login")
        elif method == "session/new":
            sid = uuid.uuid4().hex
            sessions[sid] = {"cwd": params["cwd"], "history": [], "mcp_names": [s["name"] for s in params["mcpServers"]]}
            active.add(sid)
            persist(sid)
            answer(req, {"sessionId": sid, "configOptions": [{"id": "model", "name": "Model", "type": "select", "currentValue": "fixture", "options": [{"value": "fixture", "name": "Fixture"}]}]})
            if os.getenv("FIXTURE_MAINTENANCE") == "1":
                emit({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"Compact conversation"}]}}})
        elif method == "session/list":
            answer(req, {"sessions": [{"sessionId": p.stem, "cwd": json.loads(p.read_text())["cwd"]} for p in HOME.glob("*.json")]})
        elif method in ("session/load", "session/resume"):
            sid = params["sessionId"]
            if not path(sid).exists():
                fail(req, -32002, "Unknown session")
                continue
            sessions[sid] = json.loads(path(sid).read_text())
            active.add(sid)
            if method == "session/load":
                for item in sessions[sid]["history"]:
                    emit({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": item}})
            answer(req, {})
        elif method == "session/close":
            sid = params["sessionId"]
            active.discard(sid)
            if sid in cancel:
                cancel[sid].set()
            answer(req, {})
        elif method == "session/prompt":
            if params["sessionId"] not in active:
                fail(req, -32002, "Session is not active on this connection")
            elif params["sessionId"] in running:
                fail(req, -32000, "Session is busy")
            else:
                with LOCK:
                    running[params["sessionId"]] = req["id"]
                threading.Thread(target=prompt, args=(req,), daemon=True).start()
        elif method == "_session/steering":
            sid = params["sessionId"]
            with LOCK:
                if any("DEFER_STEERING_REPLY" in p.get("text", "") for p in params["prompt"]):
                    held_steering[sid] = req
                    update(sid, "Steering request held by fixture", "agent_thought_chunk")
                    continue
                if sid not in running:
                    answer(req, {"outcome": "promptRequired", "fixtureParams": params})
                    continue
                receipt = {"inputId": params.get("_meta", {}).get("steering", {}).get("inputId"),
                           "state": "incorporated", "turnId": "fixture-turn-" + str(running[sid])}
                steering_receipts.setdefault(sid, []).append(receipt)
                emit({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": {
                    "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": ""},
                    "_meta": {"io.github.bkrabach.agent-runtime": {"inputReceipt": receipt}}}}})
                answer(req, {"outcome": "injected", "fixtureParams": params})
        elif method == "_fixture/release_steering":
            held = held_steering.pop(params["sessionId"])
            answer(held, {"outcome": params.get("outcome", "promptRequired")})
            answer(req, {})
        elif method == "session/cancel":
            if params["sessionId"] in cancel:
                cancel[params["sessionId"]].set()
        elif method == "session/set_config_option":
            answer(req, {"configOptions": [{"id": "model", "name": "Model", "type": "select", "currentValue": params["value"], "options": [{"value": "fixture", "name": "Fixture"}]}]})
        else:
            fail(req, -32601, "Method not found")
    except Exception:
        fail(req, -32602, "Malformed fixture request")
