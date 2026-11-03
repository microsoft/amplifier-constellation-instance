#!/usr/bin/env python3
"""Packaging contracts; never invoke sudo, start services, or use live credentials."""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


def load(filename):
    spec = importlib.util.spec_from_file_location(filename.replace("-", "_"), ROOT / "scripts" / (filename + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class InstallationTests(unittest.TestCase):
    def test_shell_syntax(self):
        subprocess.run(["bash", "-n", str(ROOT / "install.sh")], check=True)

    def test_dry_run_creates_nothing(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary) / "not-created"
            result = subprocess.run(["bash", str(ROOT / "install.sh"), "--dry-run", "--no-service", "--home", str(home)], text=True, capture_output=True)
            if hasattr(__import__("os"), "geteuid") and __import__("os").geteuid() == 0:
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("without sudo", result.stderr)
            else:
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("Dry run: no changes", result.stdout)
            self.assertFalse(home.exists())

    def test_empty_destination_and_valid_bind_required(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            (home / "unrelated.txt").write_text("do not replace")
            result = subprocess.run(["python3", str(ROOT / "scripts/install-support.py"), "preflight", str(home), "0.0.0.0"], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual((home / "unrelated.txt").read_text(), "do not replace")
            result = subprocess.run(["python3", str(ROOT / "scripts/install-support.py"), "preflight", str(home / "new"), "not-an-ip"], capture_output=True)
            self.assertNotEqual(result.returncode, 0)

    def test_systemd_quoting_is_context_specific(self):
        support = load("install-support")
        path = '/tmp/space $cash %percent "quote" \\slash'
        self.assertEqual(support.unit_quote(path), '/tmp/space $cash %%percent "quote" \\slash')
        self.assertIn("$cash", support.unit_quote(path, command=True))
        with self.assertRaises(ValueError):
            support.unit_quote("/tmp/a\nExecStart=evil")

    def test_distinct_units_and_metadata(self):
        with tempfile.TemporaryDirectory() as temporary:
            unit_names = []
            for identifier in ["aaaa-1111", "bbbb-2222"]:
                home = Path(temporary) / identifier
                home.mkdir()
                (home / "config.json").write_text(json.dumps({"config": {"instance_id": identifier}}))
                unit_names.append(subprocess.check_output(["python3", str(ROOT / "scripts/install-support.py"), "unit", str(home)], text=True).strip())
                unit = (home / "agent-instance.service").read_text()
                self.assertIn(str(home / "bin/agent-instance"), unit)
                self.assertIn("ExecStart=:", unit)
                self.assertIn("KillMode=control-group", unit)
                self.assertEqual((home / "agent-instance.service").stat().st_mode & 0o777, 0o600)
            self.assertNotEqual(*unit_names)

    def test_endpoints_never_print_tokens(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            (home / "endpoints.json").write_text(json.dumps({
                "base_url": "https://localhost:12345",
                "urls": ["https://192.168.1.2:12345", "https://host:12345?token=not-for-display", "http://insecure:12345"],
                "certificate_sha256": "fingerprint",
                "trust_certificate_sha256": "root-fingerprint",
                "welcome_url": "http://localhost:12346",
                "welcome_urls": ["http://192.168.1.2:12346", "http://host:12346?token=never-print"],
                "secret": "never-print-me",
            }))
            output = subprocess.check_output(["python3", str(ROOT / "scripts/install-support.py"), "endpoints", str(home)], text=True)
            self.assertIn("192.168.1.2", output)
            self.assertIn("fingerprint", output)
            self.assertNotIn("never-print-me", output)
            self.assertNotIn("not-for-display", output)
            self.assertNotIn("http://insecure", output)
            self.assertNotIn("never-print", output)
            self.assertIn("http://192.168.1.2:12346", output)
            self.assertIn("Trust certificate SHA-256: root-fingerprint", output)
            browser = subprocess.check_output(["python3", str(ROOT / "scripts/install-support.py"), "browser-url", str(home)], text=True).strip()
            self.assertEqual(browser, "http://localhost:12346")

    def test_browser_url_uses_verified_preferred_address_or_welcome(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            value = {"base_url": "https://localhost:12345", "welcome_url": "http://localhost:12346", "preferred_url": "https://agent.example.test"}
            for verified, expected in ((False, value["welcome_url"]), (True, value["preferred_url"])):
                value["preferred_url_verified"] = verified
                (home / "endpoints.json").write_text(json.dumps(value))
                result = subprocess.check_output(["python3", str(ROOT / "scripts/install-support.py"), "browser-url", str(home)], text=True).strip()
                self.assertEqual(result, expected)

    def test_generated_unit_with_actual_systemd_parser(self):
        runtime = Path(os.environ.get("XDG_RUNTIME_DIR", "/run/user/" + str(os.getuid())))
        if not shutil.which("systemd-analyze") or not runtime.is_dir():
            self.skipTest("A local systemd user runtime is needed for parser validation")
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary) / "instance space $cash %mark"
            (home / "bin").mkdir(parents=True)
            (home / "bin/agent-instance").write_text("#!/bin/sh\nexit 0\n")
            (home / "bin/agent-instance").chmod(0o700)
            (home / "config.json").write_text(json.dumps({"config": {"instance_id": "parser-check"}}))
            subprocess.run(["python3", str(ROOT / "scripts/install-support.py"), "unit", str(home)], check=True, capture_output=True)
            result = subprocess.run(["systemd-analyze", "--user", "verify", str(home / "agent-instance.service")],
                                    env=dict(os.environ, XDG_RUNTIME_DIR=str(runtime)), capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)

    def make_archive(self, root, members):
        archive = root / "release.tar.gz"
        with tarfile.open(archive, "w:gz") as bundle:
            for name, data in members:
                info = tarfile.TarInfo(name)
                info.size = len(data)
                bundle.addfile(info, io.BytesIO(data))
        checksum = root / "release.tar.gz.sha256"
        checksum.write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + "  " + archive.name + "\n")
        return archive, checksum

    def test_release_integrity_and_private_extraction(self):
        extract = load("extract-release").extract
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata = json.dumps({"name": "agent-instance", "commit": "a" * 40, "binary_sha256": hashlib.sha256(b"fake binary").hexdigest()}).encode()
            archive, checksum = self.make_archive(root, [("agent-instance", b"fake binary"), ("build-info.json", metadata)])
            destination = root / "extracted"
            extract(archive, checksum, destination)
            self.assertEqual((destination / "agent-instance").read_bytes(), b"fake binary")
            self.assertEqual((destination / "agent-instance").stat().st_mode & 0o777, 0o700)
            checksum.write_text("0" * 64 + "  release.tar.gz\n")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                extract(archive, checksum, root / "rejected")
            self.assertFalse((root / "rejected").exists())

    def test_archive_cannot_write_other_files(self):
        extract = load("extract-release").extract
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, checksum = self.make_archive(root, [("../outside", b"bad"), ("agent-instance", b"fake")])
            with self.assertRaisesRegex(ValueError, "Unexpected files"):
                extract(archive, checksum, root / "rejected")
            self.assertFalse((root / "outside").exists())

    def test_release_producer_consumer_round_trip(self):
        extract = load("extract-release").extract
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "test-agent"
            binary.write_text('#!/bin/sh\nprintf "agent-instance 0.2.0\\n"\n')
            binary.chmod(0o700)
            output = root / "packages"
            subprocess.run(["python3", str(ROOT / "scripts/package-release.py"), "--binary", str(binary), "--output", str(output), "--target", "test-linux-gnu", "--commit", "a" * 40], check=True, capture_output=True)
            archive = output / "agent-instance-test-linux-gnu.tar.gz"
            destination = root / "unpacked"
            extract(archive, archive.with_suffix(".gz.sha256"), destination)
            self.assertEqual((destination / "agent-instance").read_bytes(), binary.read_bytes())
            self.assertEqual((destination / "LICENSE").read_bytes(), (ROOT / "LICENSE").read_bytes())
            self.assertEqual((destination / "THIRD-PARTY-NOTICES.txt").read_bytes(), (ROOT / "THIRD-PARTY-NOTICES.txt").read_bytes())
            metadata = json.loads((destination / "build-info.json").read_text())
            self.assertEqual(metadata["version"], "0.2.0")
            self.assertEqual(metadata["target"], "test-linux-gnu")

    def test_archive_rejects_duplicate_notices(self):
        extract = load("extract-release").extract
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, checksum = self.make_archive(root, [("agent-instance", b"fake"), ("build-info.json", b"{}"), ("LICENSE", b"first"), ("LICENSE", b"second")])
            with self.assertRaisesRegex(ValueError, "Unexpected files"):
                extract(archive, checksum, root / "rejected")


if __name__ == "__main__":
    unittest.main()
