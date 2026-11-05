#!/usr/bin/env python3
"""Independent ACP + HTTP MCP test peer; credentials stay in process memory."""
import json
import os
import pathlib
import sys
import urllib.request
import uuid

root = pathlib.Path(sys.argv[1])
root.mkdir(parents=True, exist_ok=True)
sessions = {}


def emit(obj):
    print(json.dumps(obj), flush=True)


def answer(req, result):
    emit({"jsonrpc": "2.0", "id": req["id"], "result": result})


def mcp(server, method, params):
    headers = {h["name"]: h["value"] for h in server["headers"]}
    headers.update({"Content-Type": "application/json", "Accept": "application/json, text/event-stream"})
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    with urllib.request.urlopen(urllib.request.Request(server["url"], body, headers), timeout=5) as response:
        return json.load(response)


for line in sys.stdin:
    req = json.loads(line)
    method, params = req.get("method"), req.get("params", {})
    if method == "initialize":
        caps = {"loadSession": True, "sessionCapabilities": {"list": {}, "resume": {}, "close": {}}}
        if os.getenv("FIXTURE_NO_HTTP") != "1":
            caps["mcpCapabilities"] = {"http": True}
        answer(req, {"protocolVersion": 1, "agentCapabilities": caps, "agentInfo": {"name": "context-fixture", "version": "1"},
                     "_meta": {"steering": {"supported": True, "version": 1, "idleBehavior": ["promptRequired"]}}})
    elif method in ("session/new", "session/load", "session/resume"):
        sid = params.get("sessionId", uuid.uuid4().hex)
        sessions[sid] = params
        (root / (sid + ".json")).write_text(json.dumps({"sessionId": sid, "cwd": params["cwd"]}))
        answer(req, {"sessionId": sid} if method == "session/new" else {})
    elif method == "session/list":
        answer(req, {"sessions": [json.loads(p.read_text()) for p in root.glob("*.json")]})
    elif method == "session/close":
        sessions.pop(params["sessionId"], None)
        answer(req, {})
    elif method in ("_session/input_receipts", "_session/acknowledge_inputs"):
        answer(req, {"sessionId": params["sessionId"], "receipts": [], "recoveryFixture": method})
    elif method in ("session/prompt", "_session/steering"):
        sid = params["sessionId"]
        settings = sessions[sid]
        detail = {"mcp_names": [s["name"] for s in settings["mcpServers"]], "metadata": settings.get("_meta", {}),
                  "orientation": params["prompt"][0].get("text", "")}
        content = params["prompt"][-1].get("text", "")
        if content.startswith("CONTEXT_TOOL "):
            server = next(s for s in settings["mcpServers"] if s["name"] == "agent-continuity")
            detail["initialize"] = mcp(server, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "fixture", "version": "1"}})
            detail["tools"] = mcp(server, "tools/list", {})
            detail["call"] = mcp(server, "tools/call", json.loads(content[13:]))
        emit({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": json.dumps(detail)}}}})
        answer(req, {"outcome": "promptRequired"} if method == "_session/steering" else {"stopReason": "end_turn"})
    else:
        emit({"jsonrpc": "2.0", "id": req.get("id"), "error": {"code": -32601, "message": "Unknown fixture method"}})
