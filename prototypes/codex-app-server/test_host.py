import unittest

from host import Host


class HostTests(unittest.TestCase):
    def test_grants_and_paths_checked_before_dispatch(self):
        host = Host(lambda _: self.fail("Unexpected dispatch"))
        self.assertEqual(host.definitions(), [])
        with self.assertRaisesRegex(ValueError, "grant"):
            host.call("chat", "/work", "host_read_text", {"path": "a"}, lambda: None)
        host.configure({"fs": {"readTextFile": True}})
        self.assertEqual([d["name"] for d in host.definitions()], ["host_read_text"])
        for path in ("", "/outside", "../outside"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                host.call(
                    "chat", "/work", "host_read_text", {"path": path}, lambda: None
                )

    def test_duplicate_reply_cannot_change_result(self):
        def dispatch(msg):
            for text in ("original", "late"):
                host.response({"id": msg["id"], "result": {"content": text}})

        host = Host(dispatch)
        host.configure({"fs": {"readTextFile": True}})
        result = host.call(
            "chat", "/work", "host_read_text", {"path": "a"}, lambda: None
        )
        self.assertEqual(result["contentItems"][0]["text"], "original")
