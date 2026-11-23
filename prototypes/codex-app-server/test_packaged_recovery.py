"""Exercise the distributed owner command with a workspace-write binding."""

import hashlib
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from uuid import uuid4

from adapter import Adapter


class PackagedRecoveryTests(unittest.TestCase):
    def test_artifact_command_honors_saved_policy(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            profile = root / "profile"
            profile.mkdir()
            (profile / "auth.json").write_text("{}")
            binary = root / "fixture"
            binary.write_text(
                f"#!{sys.executable}\n"
                + """
import json, sys
for line in sys.stdin:
    event = json.loads(line)
    if "id" not in event:
        continue
    method = event["method"]
    if method == "initialize":
        result = {}
    elif method == "account/read":
        result = {"account":{"type":"chatgpt"}, "userId":"test"}
    elif method == "accountSession/list":
        result = {"activeSessionId":"a","sessions":[{"sessionId":"a","isActive":True,
                  "userId":"test","selectedWorkspaceAccountId":"original"}]}
    elif method == "thread/read":
        result = {"thread":{"status":{"type":"idle"}}}
    else:
        raise ValueError(method)
    print(json.dumps({"id":event["id"],"result":result}),flush=True)
"""
            )
            binary.chmod(0o700)
            state = root / "state"
            engine = Adapter(
                binary, profile, state, True, "test-model", workspace_write=True
            )
            sid, op = str(uuid4()), str(uuid4())
            # A valid one-pixel PNG works for both export and optional Pillow preview.
            from base64 import b64decode

            data = b64decode(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
            )
            sha = hashlib.sha256(data).hexdigest()
            try:
                with engine.db:
                    engine.db.execute(
                        "insert into sessions values(?,?,?)",
                        (sid, engine.binding(), str(root)),
                    )
                    engine.db.execute(
                        "insert into operations values(?,?,?, ?,NULL)",
                        (sid, op, "prompt", "turn"),
                    )
                    engine.db.execute(
                        "insert into images values(?,?,?,?,?)",
                        (sid, op, "item", sha, data),
                    )
            finally:
                engine.close()
            packaging = (
                Path(__file__).resolve().parents[2] / "scripts/package-codex-adapter.py"
            )
            subprocess.run(
                [sys.executable, str(packaging), "--output", tmp],
                check=True,
                capture_output=True,
            )
            cmd = [
                sys.executable,
                str(root / "codex-app-server-adapter.pyz"),
                "artifact",
                "--binary",
                str(binary),
                "--profile",
                str(profile),
                "--directory",
                str(state),
                "--model",
                "test-model",
                "--session",
                sid,
                "--sha256",
                sha,
            ]
            out = root / "original.png"
            refused = subprocess.run(
                cmd + ["--export-to", str(out)], check=False, capture_output=True
            )
            self.assertNotEqual(refused.returncode, 0)
            self.assertFalse(out.exists())
            cmd.append("--workspace-write")
            subprocess.run(cmd + ["--export-to", str(out)], check=True)
            self.assertEqual(out.read_bytes(), data)
            try:
                from PIL import Image
            except ImportError:
                Image = None
            if Image:
                preview = root / "preview.png"
                subprocess.run(cmd + ["--preview-to", str(preview)], check=True)
                with Image.open(preview) as picture:
                    self.assertEqual(picture.size, (1, 1))
            subprocess.run(cmd + ["--accept-interrupted-operation", op], check=True)
            with closing(sqlite3.connect(state / "journal.sqlite")) as db:
                self.assertEqual(
                    db.execute("select sha from accepted_effects").fetchone(), (sha,)
                )
