"""Protocol fixture: writes only inside its temporary test workspace; no model claims."""
import json, os, pathlib, sys, threading, time
root = pathlib.Path(sys.argv[1]); root.mkdir(exist_ok=True)
sid = "fixture-background"
current_mode = "ask"
lock = threading.Lock()
cancelled = threading.Event()
def send(v):
    with lock: print(json.dumps({"jsonrpc":"2.0", **v}), flush=True)
def finish(request):
    params = request["params"]
    prompt = "\n".join(x.get("text", "") for x in params["prompt"])
    saved_path = root / "saved-session.json"
    saved = json.loads(saved_path.read_text()) if saved_path.exists() else {"turn": 0, "goal": prompt}
    saved["turn"] += 1
    saved_path.write_text(json.dumps(saved))
    scenario = saved["goal"]
    prompt = prompt + "\n" + scenario
    (root / "prompt.txt").write_text(prompt)
    (root / "prompt-mode.json").write_text(json.dumps({"mode": current_mode}))
    with (root / "prompts.log").open("a") as f: f.write("started\n")
    if "NEEDS_APPROVAL" in prompt:
        send({"id": "approval", "method": "session/request_permission", "params": {"sessionId": sid, "options": [{"optionId":"allow", "kind":"allow_once"}], "toolCall":{"toolCallId":"t1", "title":"Escalated operation"}}})
        return
    if "MODE_CHANGED" in prompt:
        send({"method":"session/update", "params":{"sessionId":sid,"update":{"sessionUpdate":"current_mode_update","currentModeId":"ask"}}})
        return
    if "DELAY_RUN" in prompt: cancelled.wait(.4)
    if "SLOW_RUN" in prompt:
        for _ in range(100):
            if cancelled.wait(.1): return
    if "TOOL_BUDGET" in prompt:
        for i in range(2): send({"method":"session/update", "params":{"sessionId":sid,"update":{"sessionUpdate":"tool_call","toolCallId":str(i),"title":"fixture tool","status":"in_progress"}}})
    if "TURN_TOOL" in scenario:
        send({"method":"session/update", "params":{"sessionId":sid,"update":{"sessionUpdate":"tool_call","toolCallId":"reused-per-turn","title":"fixture step","status":"completed"}}})
    text = "x"*2048 if "OUTPUT_BUDGET" in prompt else "Created result.txt and checked the exact fixture content. This is an agent report."
    if any(marker in scenario for marker in ["MULTITURN", "ALWAYS_CONTINUE", "REPORT_BLOCKER"]):
        disposition = "blocked" if "REPORT_BLOCKER" in scenario else "continue" if "ALWAYS_CONTINUE" in scenario or saved["turn"] < 2 else "review"
        text += "\nAGENT_WORK_STATUS " + json.dumps({"status":disposition,"summary": "Need an input file" if disposition=="blocked" else "Finished step " + str(saved["turn"])})
    (root / "result.txt").write_bytes(b"actual fixture artifact\n")
    send({"method":"session/update", "params":{"sessionId":sid,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}})
    send({"id":request["id"],"result":{"stopReason":"end_turn"}})
for line in sys.stdin:
    r=json.loads(line); m=r.get("method")
    if m=="initialize":
        (root / "initialize.json").write_text(json.dumps(r["params"]))
        send({"id":r["id"],"result":{"protocolVersion":1,"agentInfo":{"name":"ordinary-acp-fixture","version":"1"},"agentCapabilities":{"loadSession":not bool(os.environ.get("NO_LOAD"))}}})
    elif m=="session/new":
        (root / "saved-session.json").unlink(missing_ok=True)
        (root / "session.json").write_text(json.dumps(r["params"]))
        result={"sessionId":sid}
        if not os.environ.get("NO_MODES"): result["modes"]={"currentModeId":"ask","availableModes":[{"id":"ask","name":"Ask"},{"id":"workspace","name":"Workspace"},{"id":"read_only","name":"Read only"}]}
        send({"id":r["id"],"result":result})
    elif m=="session/load":
        (root / "loaded.json").write_text(json.dumps(r["params"]))
        send({"method":"session/update","params":{"sessionId":sid,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"REPLAYED HISTORY MUST NOT COUNT AS NEW OUTPUT"}}}})
        send({"id":r["id"],"result":{"modes":{"currentModeId":"ask","availableModes":[{"id":"ask","name":"Ask"},{"id":"workspace","name":"Workspace"}]}}})
    elif m=="session/set_mode":
        (root / "mode.json").write_text(json.dumps(r["params"]))
        if os.environ.get("REJECT_MODE"): send({"id":r["id"],"error":{"code":-32602,"message":"Fixture refused mode"}})
        else:
            current_mode=r["params"]["modeId"]
            send({"id":r["id"],"result":{}})
    elif m=="session/prompt": threading.Thread(target=finish,args=(r,),daemon=True).start()
    elif m=="session/cancel": cancelled.set()
    elif r.get("id")=="approval": (root / "permission.json").write_text(json.dumps(r))
