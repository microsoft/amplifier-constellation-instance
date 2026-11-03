"""Release transport regressions: immutable publication and partial retries."""
import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("upload", Path(__file__).with_name("upload-release.py"))
upload = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upload)


class ReleaseTests(unittest.TestCase):
    def exercise(self, draft, remote, expected_error=None):
        calls = []
        original = dict(remote)

        def run(argv, **kwargs):
            command = argv[2]
            calls.append(command)
            if command == "view":
                return subprocess.CompletedProcess(argv, 0 if draft is not None else 1,
                    json.dumps({"isDraft": draft, "assets": [{"name": n} for n in remote]}))
            if command == "download":
                name = argv[argv.index("--pattern") + 1]
                (Path(argv[argv.index("--dir") + 1]) / name).write_bytes(remote[name])
            if command == "upload":
                path = Path(argv[-1])
                self.assertNotIn(path.name, remote)
                remote[path.name] = path.read_bytes()
            return subprocess.CompletedProcess(argv, 0, "")

        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            (directory / "binary.tar.gz").write_bytes(b"reviewed build")
            (directory / "binary.tar.gz.sha256").write_bytes(b"checksum")
            if expected_error:
                with self.assertRaisesRegex(ValueError, expected_error):
                    upload.stage("v0.12.0", directory, run)
                self.assertEqual(remote, original)
            else:
                upload.stage("v0.12.0", directory, run)
        return calls, remote

    def test_published_reviewed_package_is_unchanged(self):
        calls, _ = self.exercise(False, {"binary.tar.gz": b"reviewed package with notices",
                                        "binary.tar.gz.sha256": b"published checksum"})
        self.assertEqual(calls, ["view"])

    def test_published_incomplete_release_requires_manual_repair(self):
        self.exercise(False, {"binary.tar.gz": b"original"}, "missing expected assets")

    def test_new_release_stays_draft(self):
        calls, remote = self.exercise(None, {})
        self.assertEqual(calls, ["view", "create", "upload", "upload"])
        self.assertEqual(len(remote), 2)

    def test_matching_partial_draft_upload_resumes(self):
        calls, remote = self.exercise(True, {"binary.tar.gz": b"reviewed build"})
        self.assertEqual(calls, ["view", "download", "upload"])
        self.assertEqual(len(remote), 2)

    def test_differing_partial_draft_is_not_overwritten(self):
        self.exercise(True, {"binary.tar.gz": b"different build"}, "Draft asset differs")


if __name__ == "__main__":
    unittest.main()
