import sqlite3
import unittest
from unittest.mock import Mock

from adapter import Adapter


class BindingTest(unittest.TestCase):
    def setUp(self):
        self.adapter = Adapter.__new__(Adapter)
        self.adapter.model = "test"
        self.adapter.images = True
        self.adapter.workspace_write = False
        self.adapter.db = sqlite3.connect(":memory:")
        self.addCleanup(self.adapter.db.close)
        self.adapter.db.execute("create table sessions(sid, binding, cwd)")
        self.user = "user-a"
        self.workspace = "workspace-a"
        self.adapter.rpc = Mock(side_effect=self.provider)
        self.adapter.db.execute(
            "insert into sessions values(?,?,?)",
            ("chat", self.adapter.binding(), "/work"),
        )

    def provider(self, method, params):
        if method == "account/read":
            return {
                "userId": self.user,
                "account": {"type": "chatgpt", "workspaceName": "Same display name"},
            }
        if method == "accountSession/list":
            return {
                "activeSessionId": "active",
                "sessions": [
                    {
                        "isActive": True,
                        "sessionId": "active",
                        "userId": self.user,
                        "selectedWorkspaceAccountId": self.workspace,
                    }
                ],
            }
        self.fail("Unexpected request before checking identity: " + method)

    def test_different_user_same_workspace(self):
        self.user = "user-b"
        with self.assertRaisesRegex(ValueError, "Account or route changed"):
            self.adapter.resume("chat", "/work")

    def test_workspace_switch_and_return(self):
        self.workspace = "workspace-b"
        with self.assertRaisesRegex(ValueError, "Account or route changed"):
            self.adapter.original("chat", "some-sha")
        self.workspace = "workspace-a"
        self.assertEqual(self.adapter.check("chat"), "/work")

    def test_missing_workspace(self):
        self.workspace = None
        with self.assertRaisesRegex(ValueError, "Cannot identify"):
            self.adapter.check("chat")

    def test_resume_refuses_substituted_model_before_loading_history(self):
        from contextlib import nullcontext

        self.adapter.lease = lambda sid: nullcontext()
        self.adapter.loaded = "chat"

        def provider(method, params):
            if method == "thread/resume":
                self.assertEqual(params["model"], "test")
                return {"model": "other", "thread": {"id": "chat", "turns": []}}
            return self.provider(method, params)

        self.adapter.rpc = provider
        with self.assertRaisesRegex(ValueError, "saved model"):
            self.adapter.resume("chat", "/work")
        self.assertIsNone(self.adapter.loaded)
