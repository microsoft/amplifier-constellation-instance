#!/usr/bin/env python3
"""Independent scripted ACP peer: no model, network, or real side effects."""
import json, os, sys, threading, uuid, pathlib, time
NS = "com.microsoft.amplifier.approval-policy"
lock = threading.Lock()
waiting = {}
producer = os.environ.get("APPROVAL_PRODUCER", "fixture.semantic/1")
def emit(v):
    with lock:
        print(json.dumps(v), flush=True)
def result(req, value):
    emit({"jsonrpc":"2.0", "id":req["id"], "result":value})
def prompt(req):
    sid=req["params"]["sessionId"]
    spec=json.loads(req["params"]["prompt"][-1]["text"])
    if spec.get("gate"):
        emit({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"fixture-gated-before-permission"}}}})
        deadline=time.monotonic()+5
        while not pathlib.Path("release-permission").exists() and time.monotonic()<deadline:
            time.sleep(.005)
    event=threading.Event()
    row={"event":event,"req":req}
    waiting[sid]=row
    descriptor={"version":1,"producer":spec.get("producer",producer),"key":spec.get("key","a"*64),"description":"Write the fixture’s fixed disposable report","allowOptionId":"permit","authorityLifetime":spec.get("lifetime","request")}
    descriptor.update(spec.get("descriptor",{}))
    options=[{"optionId":"permit","kind":"allow_once","name":"Allow"},{"optionId":"refuse","kind":"reject_once","name":"Refuse"}]
    if spec.get("engine_remember"):
        options.append({"optionId":"engine-cache","kind":"allow_always","name":"Engine remember"})
    if spec.get("ambiguous"):
        options.append({"optionId":"other-permit","kind":"allow_once","name":"Other permission"})
    emit({"jsonrpc":"2.0","id":1,"method":"_fixture/question" if spec.get("question") else "session/request_permission","params":{"sessionId":sid,"toolCall":{"toolCallId":"call-1","title":"Disposable fixture effect","rawInput":{"secret":"raw-arguments-canary"},"_meta":{NS:descriptor}},"options":options}})
    if not event.wait(5):
        result(req,{"stopReason":"cancelled","fixtureError":"callback did not settle"})
    elif not row.get("cancelled"):
        result(req,{"stopReason":"end_turn","decision":row.get("decision"),"initProfile":init_profile})
    waiting.pop(sid,None)
init_profile=None
for line in sys.stdin:
    req=json.loads(line)
    method=req.get("method")
    if method=="initialize":
        init_profile=req.get("params",{}).get("_meta",{}).get(NS)
        profile={"version":1,"owner":"instance","producer":producer,"reusableRequests":True}
        result(req,{"protocolVersion":1,"agentInfo":{"name":"approval-fixture","version":"1"},"agentCapabilities":{},"_meta":{NS:profile} if producer!="none" else {}})
    elif method=="session/new":
        result(req,{"sessionId":str(uuid.uuid4())})
    elif method=="session/prompt":
        threading.Thread(target=prompt,args=(req,),daemon=True).start()
    elif method in ("session/cancel","session/close"):
        row=waiting.get(req["params"]["sessionId"])
        if row:
            row["cancelled"]=True
            result(row["req"],{"stopReason":"cancelled"})
            row["event"].set()
        if "id" in req: result(req,{})
    elif method is None:
        for row in list(waiting.values()):
            row["decision"]=req.get("result",{}).get("outcome")
            row["event"].set()
    elif "id" in req:
        emit({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32601,"message":"Unsupported fixture method"}})
