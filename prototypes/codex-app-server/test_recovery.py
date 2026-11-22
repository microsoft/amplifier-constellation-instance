"""Reconcile provider-owned completed effects without issuing another model request."""

import sqlite3
import tempfile
import unittest
from pathlib import Path
from uuid import uuid4

from adapter import Adapter, digest


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.sid, self.op, self.turn = (str(uuid4()) for _ in range(3))
        self.adapter = Adapter.__new__(Adapter)
        self.adapter.profile = self.adapter.directory = Path(scratch.name)
        self.adapter.loaded = self.sid
        self.adapter.images = True
        self.adapter.guard = lambda: None
        self.adapter.binding = lambda: "route"
        db = self.adapter.db = sqlite3.connect(":memory:")
        self.addCleanup(db.close)
        db.executescript("""
            CREATE TABLE sessions(sid, binding, cwd);
            CREATE TABLE operations(sid, op, prompt_hash, turn, done);
            CREATE TABLE stops(sid, op, reason);
            CREATE TABLE accepted_effects(sid, op, sha);
            CREATE TABLE images(sid, op, item, sha, bytes, UNIQUE(sid,item));
            CREATE TABLE image_sources(sid, op, path, sha, UNIQUE(sid,op,path));
        """)
        db.execute("INSERT INTO sessions VALUES (?, 'route', 'work')", (self.sid,))
        db.execute(
            "INSERT INTO operations VALUES (?,?,?,?,NULL)",
            (self.sid, self.op, digest(b"revise"), self.turn),
        )
        self.items = []

        def request(method, params):
            self.assertEqual(method, "thread/read", "Recovery attempted new inference")
            self.assertEqual(params, {"threadId": self.sid, "includeTurns": True})
            return {
                "thread": {
                    "turns": [
                        {"id": self.turn, "status": "interrupted", "items": self.items}
                    ]
                }
            }

        self.adapter.rpc = request

    def test_pre_receipt_crash_imports_exact_bytes_once(self):
        source = self.adapter.profile / "generated_images" / self.sid / "edit.png"
        source.parent.mkdir(parents=True)
        data = b"\x89PNG\r\n\x1a\noriginal edited bytes"
        source.write_bytes(data)
        self.items.append(
            {
                "type": "imageGeneration",
                "id": "revision",
                "status": "completed",
                "savedPath": str(source),
            }
        )
        for _ in range(2):
            with self.assertRaisesRegex(ValueError, "outcome uncertain"):
                self.adapter.prompt(self.sid, self.op, "revise", self.fail)
            self.assertEqual(self.adapter.artifacts(self.sid, self.op), [digest(data)])
        source.unlink()
        with self.assertRaisesRegex(ValueError, "outcome uncertain"):
            self.adapter.prompt(self.sid, self.op, "revise", self.fail)
        self.assertEqual(
            self.adapter.original_for_path(self.sid, self.op, str(source)), data
        )
        with self.assertRaises(ValueError):
            self.adapter.original_for_path(self.sid, str(uuid4()), str(source))

    def test_missing_provider_effect_stays_uncertain(self):
        with self.assertRaisesRegex(ValueError, "outcome uncertain"):
            self.adapter.prompt(self.sid, self.op, "revise", self.fail)
        self.assertEqual(self.adapter.artifacts(self.sid, self.op), [])
