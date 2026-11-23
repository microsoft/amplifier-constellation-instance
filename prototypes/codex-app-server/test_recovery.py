"""Reconcile provider-owned completed effects without issuing another model request."""

import sqlite3
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import Mock
from uuid import uuid4

from adapter import Adapter, TurnCancelled, digest


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
            CREATE TABLE callbacks(sid,turn,call,state);
            CREATE TABLE turn_effects(sid,turn,UNIQUE(sid,turn));
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

    def test_cancel_before_dispatch_then_new_instruction(self):
        a = self.adapter
        a.db.execute("delete from operations")
        a.seq = 0
        a.dispatch_lock = threading.Lock()
        a.cancel = threading.Event()
        a.request_cancel()
        a.rpc = Adapter.rpc.__get__(a)
        a.send = Mock()
        for _ in range(2):
            with self.assertRaises(TurnCancelled):
                a.prompt(self.sid, self.op, "revise", self.fail)
        a.send.assert_not_called()
        a.cancel.clear()
        self.assert_next_instruction_succeeds()

    def assert_next_instruction_succeeds(self):
        a = self.adapter
        a.rpc = Mock(return_value={"turn": {"id": "next"}})
        a.read = Mock(
            return_value={
                "method": "turn/completed",
                "params": {
                    "threadId": self.sid,
                    "turn": {"id": "next", "status": "completed"},
                },
            }
        )
        self.assertEqual(
            a.prompt(self.sid, str(uuid4()), "continue", self.fail), ("", [])
        )
        self.assertEqual(a.rpc.call_count, 1)

    def simulate_start_error(self, error):
        a = self.adapter
        a.db.execute("delete from operations")
        a.rpc = Adapter.rpc.__get__(a)
        a.seq = 0
        a.dispatch_lock = threading.Lock()
        a.cancel = threading.Event()
        a.send = Mock()
        a.wire = Mock(return_value={"id": 1, "error": error})
        with self.assertRaises(ValueError):
            a.prompt(self.sid, self.op, "revise", self.fail)
        self.assertEqual(a.send.call_count, 1)

    def test_definite_rejection_is_durable_and_allows_new_instruction(self):
        self.simulate_start_error({"code": -32602, "message": "invalid input"})
        with self.assertRaisesRegex(ValueError, "rejected"):
            self.adapter.prompt(self.sid, self.op, "revise", self.fail)
        self.assert_next_instruction_succeeds()

    def test_ambiguous_start_blocks_new_instruction(self):
        self.simulate_start_error({"code": -32603, "message": "connection lost"})
        with self.assertRaisesRegex(ValueError, "outstanding"):
            self.adapter.prompt(self.sid, str(uuid4()), "continue", self.fail)
        self.assertEqual(self.adapter.send.call_count, 1)

    def test_confirmed_text_failure_remains_failed_on_retry_and_unblocks_chat(self):
        a = self.adapter
        a.db.execute("delete from operations")
        turn = {
            "id": self.turn,
            "status": "failed",
            "itemsView": "full",
            "items": [{"type": "userMessage"}],
        }
        a.rpc = Mock(side_effect=[{"turn": turn}, {"thread": {"turns": [turn]}}])
        a.read = Mock(
            return_value={
                "method": "turn/completed",
                "params": {"threadId": self.sid, "turn": turn},
            }
        )
        for _ in range(2):
            with self.assertRaisesRegex(ValueError, "text turn failed"):
                a.prompt(self.sid, self.op, "revise", self.fail)
        self.assertEqual(a.rpc.call_count, 2)
        self.assert_next_instruction_succeeds()

    def test_unresolved_effect_in_live_stream_or_partial_history_blocks_replay(self):
        a = self.adapter
        turn = {
            "id": self.turn,
            "status": "failed",
            "itemsView": "summary",
            "items": [],
        }
        a.rpc = Mock(return_value={"thread": {"turns": [turn]}})
        for view in ("summary", "full"):
            turn["itemsView"] = view
            if view == "full":
                a.db.execute(
                    "insert into turn_effects values(?,?)", (self.sid, self.turn)
                )
            with self.assertRaisesRegex(ValueError, "uncertain"):
                a.prompt(self.sid, self.op, "revise", self.fail)
            with self.assertRaisesRegex(ValueError, "outstanding"):
                a.prompt(self.sid, str(uuid4()), "continue", self.fail)
