"""An enrolled idle selection survives refresh from a differently versioned Instance."""

import base64
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "worker", Path(__file__).with_name("worker.py")
)
w = importlib.util.module_from_spec(spec)
spec.loader.exec_module(w)

CAPABLE = b"WHEN_IDLE_COMPONENTS_VERSION = 1\n"
LEGACY = b"BOOTSTRAP = True\n"


class ScopedHelperRefreshTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.binary = self.root / "instance"
        self.binary.write_bytes(b"installed Instance binary")
        self.path = self.root / "worker.py"
        self.path.write_bytes(CAPABLE + b"# local enrollment\n")
        self.old = self.path.read_bytes()
        self.obj = object.__new__(w.Worker)
        self.obj.root = self.root
        self.obj.file = self.root / "state.json"
        self.obj.registration = self.root / "registration.json"
        self.obj.state = {"completed_requests": ["previous"], "state": "up_to_date"}
        self.obj.c = {
            "when_idle_components": ["minecraft"],
            "components": [
                {"id": "instance", "kind": "instance", "binary": str(self.binary)},
                {"id": "minecraft"},
            ],
        }

    def test_scoped_native_skips_incapable_export_and_retains_all_files(self):
        def export(args):
            if args[1] == "--version":
                return b"agent-instance 0.22.15"
            self.assertEqual(args[1], "export-update-helper")
            Path(args[-1]).write_bytes(LEGACY)
            return b""

        with (
            patch.object(w, "__file__", str(self.path)),
            patch.object(w, "run", side_effect=export) as run,
            patch.object(w.os, "execv") as execute,
        ):
            self.obj.refresh_helper()
            self.obj.refresh_helper()
        self.assertEqual(run.call_count, 2)
        execute.assert_not_called()
        self.assertFalse((self.root / "worker-next.py").exists())
        self.assertEqual(self.path.read_bytes(), self.old)
        self.assertEqual(
            self.obj.state["helper_instance_sha256"], w.digest(self.binary)
        )
        self.assertEqual(self.obj.state["completed_requests"], ["previous"])

    def exported(self, worker):
        return {
            name: base64.b64encode(worker if name == "worker.py" else b"{}\n").decode()
            for name in w.HELPER_FILES
        }

    def management(self, source):
        self.obj.c["management"] = {"protocol": 1}
        return {
            "binary_sha256": "a" * 64,
            "files": self.exported(source),
            "supported": True,
        }

    def test_scoped_management_preserves_bundle_on_missing_capability(self):
        response = self.management(LEGACY)
        with (
            patch.object(w, "management_call", return_value=response),
            patch.object(w.os, "execv") as execute,
        ):
            self.obj.refresh_helper()
        execute.assert_not_called()
        self.assertEqual(self.path.read_bytes(), self.old)
        self.assertFalse((self.root / "engine_provider.py").exists())
        self.assertNotIn("helper_refresh", self.obj.state)
        self.assertEqual(
            self.obj.state["helper_instance_sha256"], response["binary_sha256"]
        )

    def test_scoped_management_recovery_refuses_incapable_pending_bundle(self):
        response = self.management(LEGACY)
        self.obj.save(
            helper_refresh={
                "binary_sha256": response["binary_sha256"],
                "files": response["files"],
            }
        )
        with (
            patch.object(w.os, "execv") as execute,
            self.assertRaisesRegex(
                ValueError, "pending_helper_missing_scoped_idle_support"
            ),
        ):
            self.obj.finish_helper_refresh()
        execute.assert_not_called()
        self.assertEqual(self.path.read_bytes(), self.old)
        self.assertIn("helper_refresh", w.read(self.obj.file))
        self.assertNotIn("helper_instance_sha256", self.obj.state)

    def test_capable_export_publishes_and_restarts(self):
        response = self.management(CAPABLE + b"# newer version\n")
        with (
            patch.object(w, "management_call", return_value=response),
            patch.object(w.os, "execv") as execute,
        ):
            self.obj.refresh_helper()
        execute.assert_called_once()
        self.assertEqual(self.path.read_bytes(), CAPABLE + b"# newer version\n")
        self.assertEqual((self.root / "engine_provider.py").read_bytes(), b"{}\n")
        self.assertEqual(self.obj.state["completed_requests"], ["previous"])

    def test_marker_is_a_module_level_integer_declaration(self):
        for source in (
            b"# WHEN_IDLE_COMPONENTS_VERSION = 1\n",
            b'"WHEN_IDLE_COMPONENTS_VERSION = 1"',
            b"if False:\n WHEN_IDLE_COMPONENTS_VERSION = 1",
            b"WHEN_IDLE_COMPONENTS_VERSION = True",
            b"WHEN_IDLE_COMPONENTS_VERSION = 0",
        ):
            with self.subTest(source=source):
                self.assertFalse(self.obj.helper_supports_registration(source))
        self.assertTrue(self.obj.helper_supports_registration(CAPABLE))
        del self.obj.c["when_idle_components"]
        self.assertTrue(self.obj.helper_supports_registration(LEGACY))


if __name__ == "__main__":
    unittest.main()
