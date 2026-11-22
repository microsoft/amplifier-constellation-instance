"""ACP host filesystem callbacks. The client supplies all file contents."""

import os
import queue
import threading
import time
from pathlib import PurePath
from uuid import uuid4


class Host:
    def __init__(self, output):
        self.output = output
        self.files = {}
        self.lock = threading.Lock()
        self.pending = {}

    def configure(self, capabilities):
        self.files = dict(capabilities.get("fs", {}))

    def definitions(self):
        fields = {"path": {"type": "string", "description": "Workspace-relative path"}}
        tools = []
        for grant, name, description in [
            (
                "readTextFile",
                "host_read_text",
                "Read a text document from the connected host workspace.",
            ),
            (
                "writeTextFile",
                "host_write_text",
                "Write a text document to the connected host workspace.",
            ),
        ]:
            if self.files.get(grant) is not True:
                continue
            properties = dict(fields)
            if grant == "writeTextFile":
                properties["content"] = {"type": "string"}
            tools.append(
                {
                    "type": "function",
                    "name": name,
                    "description": description,
                    "inputSchema": {
                        "type": "object",
                        "properties": properties,
                        "required": list(properties),
                        "additionalProperties": False,
                    },
                }
            )
        return tools

    def response(self, event):
        with self.lock:
            box = self.pending.pop(event.get("id"), None)
        if box is not None:
            box.put_nowait(event)

    def call(self, sid, cwd, name, arguments, guard):
        methods = {
            "host_read_text": ("readTextFile", "fs/read_text_file"),
            "host_write_text": ("writeTextFile", "fs/write_text_file"),
        }
        grant, method = methods[name]
        if self.files.get(grant) is not True:
            raise ValueError("Host did not grant this capability")
        if set(arguments) != (
            {"path"} if grant == "readTextFile" else {"path", "content"}
        ):
            raise ValueError("Invalid host arguments")
        path = PurePath(arguments["path"])
        if not path.parts or path.is_absolute() or ".." in path.parts:
            raise ValueError("Use a workspace-relative path")
        params = {**arguments, "path": os.path.join(cwd, str(path)), "sessionId": sid}
        if "content" in params and not isinstance(params["content"], str):
            raise ValueError("Expected text")
        rid = str(uuid4())
        box = queue.Queue(1)
        with self.lock:
            self.pending[rid] = box
        try:
            guard()
            self.output({"id": rid, "method": method, "params": params})
            deadline = time.monotonic() + 120
            while True:
                if time.monotonic() >= deadline:
                    raise RuntimeError(
                        "Host response timed out; operation outcome uncertain."
                    )
                guard()
                try:
                    event = box.get(timeout=0.1)
                except queue.Empty:
                    continue
                guard()
                if "error" in event:
                    raise ValueError("The host rejected the file operation")
                data = (
                    event["result"]["content"] if grant == "readTextFile" else "Saved"
                )
                if not isinstance(data, str) or len(data.encode()) > 256 * 1024:
                    raise ValueError("Host result is too large or not text")
                return {
                    "success": True,
                    "contentItems": [{"type": "inputText", "text": data}],
                }
        finally:
            with self.lock:
                self.pending.pop(rid, None)
