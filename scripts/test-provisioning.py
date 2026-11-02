#!/usr/bin/env python3
"""Default runtime boundary checks, without downloads or real provider credentials."""
import base64
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("provision", ROOT / "scripts/provision-backend.py")
provision = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provision)
native_spec = importlib.util.spec_from_file_location("native_provision", ROOT / "scripts/provision-native.py")
native = importlib.util.module_from_spec(native_spec)
native_spec.loader.exec_module(native)


class NativeDownloadTests(unittest.TestCase):
    def fixture(self):
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w:gz") as tar:
            entry = tarfile.TarInfo("agent-runtime")
            entry.size = len(b"fixture")
            tar.addfile(entry, io.BytesIO(b"fixture"))
        data = archive.getvalue()
        lock = {"repository": "example/runtime", "version": "0.7.0", "assets": [{
            "platform": "linux-aarch64", "asset": "runtime.tar.gz",
            "asset_sha256": hashlib.sha256(data).hexdigest(),
            "binary_sha256": hashlib.sha256(b"fixture").hexdigest(),
        }]}
        return data, lock

    def test_public_release_does_not_require_github_cli_authentication(self):
        data, lock = self.fixture()
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(native, "platform_key", return_value="linux-aarch64"), \
                mock.patch.object(native.shutil, "which", return_value="gh"), \
                mock.patch.object(native.subprocess, "run") as command, \
                mock.patch.object(native.urllib.request, "urlopen", return_value=io.BytesIO(data)):
            binary = native.obtain(lock, Path(directory), "agent-runtime")
            self.assertEqual(binary.read_bytes(), b"fixture")
            command.assert_not_called()

    def test_authenticated_fallback_still_requires_the_pinned_checksum(self):
        _, lock = self.fixture()
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(native, "platform_key", return_value="linux-aarch64"), \
                mock.patch.object(native.shutil, "which", return_value="gh"), \
                mock.patch.object(native.urllib.request, "urlopen", side_effect=native.urllib.error.HTTPError(
                    "https://github.com/example/runtime", 404, "Not found", {}, None)), \
                mock.patch.object(native.subprocess, "run", side_effect=lambda *a, **kw:
                    (Path(directory) / "runtime.tar.gz").write_bytes(b"corrupt")):
            with self.assertRaisesRegex(ValueError, "archive checksum mismatch"):
                native.obtain(lock, Path(directory), "agent-runtime")


class ProvisioningTests(unittest.TestCase):
    @unittest.skipIf(os.name == "nt", "Windows requires explicit account connection")
    def test_owner_account_is_copied_once_without_shared_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            owner = root / "owner"
            provider = root / "instance/backend/provider"
            provision.private_dir(owner / ".codex")
            provision.private_dir(provider)
            source = owner / ".codex/auth.json"
            source.write_text(json.dumps({"tokens": {"access_token": "test-only-original"}}))
            (owner / ".codex/config.toml").write_text("private_owner_configuration = true")
            self.assertTrue(provision.copy_owner_account(provider, True, owner))
            target = provider / "auth.json"
            self.assertNotEqual(source.stat().st_ino, target.stat().st_ino)
            self.assertEqual(target.stat().st_mode & 0o777, 0o600)
            self.assertFalse((provider / "config.toml").exists())
            target.write_text(json.dumps({"tokens": {"access_token": "test-only-refreshed"}}))
            self.assertFalse(provision.copy_owner_account(provider, True, owner))
            self.assertIn("test-only-original", source.read_text())
            self.assertIn("test-only-refreshed", target.read_text())
            self.assertNotIn("test-only", (provider / "account-source.json").read_text())

    @unittest.skipIf(os.name == "nt", "Windows requires explicit account connection")
    def test_account_reuse_opt_out_and_source_symlinks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            owner = root / "owner"
            provider = root / "provider"
            provision.private_dir(owner / ".codex")
            provision.private_dir(provider)
            external = root / "other-account.json"
            external.write_text('{"OPENAI_API_KEY":"test-only"}')
            (owner / ".codex/auth.json").symlink_to(external)
            self.assertFalse(provision.copy_owner_account(provider, True, owner))
            (owner / ".codex/auth.json").unlink()
            (owner / ".codex/auth.json").write_text(external.read_text())
            self.assertFalse(provision.copy_owner_account(provider, False, owner))
            self.assertFalse((provider / "auth.json").exists())

    def test_rejects_unrecognized_or_oversized_account_cache(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            provision.private_dir(root / ".codex")
            provision.private_dir(root / "provider")
            source = root / ".codex/auth.json"
            for content in ('{"tokens":{}}', '{invalid', '"not-a-cache"', 'x' * (1024 * 1024 + 1)):
                source.write_text(content)
                self.assertFalse(provision.copy_owner_account(root / "provider", True, root))

    def archive(self, root, name, kind=None):
        archive = root / "package.tgz"
        with tarfile.open(archive, "w:gz") as target:
            entry = tarfile.TarInfo(name)
            if kind:
                entry.type = kind
                entry.linkname = "/tmp/elsewhere"
                target.addfile(entry)
            else:
                entry.size = 7
                entry.mode = 0o7777
                target.addfile(entry, io.BytesIO(b"fixture"))
        return archive

    def test_extraction_rejects_path_traversal_links_and_privileged_modes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name, kind in (("../escaped", None), ("/absolute", None), ("link", tarfile.SYMTYPE), ("hardlink", tarfile.LNKTYPE)):
                with self.assertRaises(ValueError):
                    provision.extract_files(self.archive(root, name, kind), root / "output", lambda name: name)
            provision.extract_files(self.archive(root, "safe"), root / "output", lambda name: name)
            if os.name != "nt":
                self.assertEqual((root / "output/safe").stat().st_mode & 0o7777, 0o700)

    def test_pins_approve_only_expected_publishers_and_platforms(self):
        lock = json.loads((ROOT / "scripts/backend-lock.json").read_text())
        self.assertEqual(set(lock["platforms"]), {"linux-arm64", "linux-x64", "darwin-arm64", "darwin-x64", "win32-arm64", "win32-x64"})
        for package in [lock["acp"]] + [platform[name] for platform in lock["platforms"].values() for name in ("codex", "node")]:
            self.assertTrue(package["url"].startswith(("https://nodejs.org/", "https://registry.npmjs.org/")))
            algorithm, checksum = package["integrity"].split("-", 1)
            decoded = bytes.fromhex(checksum) if algorithm == "sha256" else base64.b64decode(checksum)
            self.assertEqual(len(decoded), hashlib.new(algorithm).digest_size)

    def test_windows_zip_extraction_and_platform_detection(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "node.zip"
            with zipfile.ZipFile(archive, "w") as source:
                source.writestr("node/node.exe", b"fixture")
            provision.extract_zip(archive, root / "out", lambda name: "bin/node.exe")
            self.assertEqual((root / "out/bin/node.exe").read_bytes(), b"fixture")
            for name in ("../escape", "C:/escape", "folder\\escape"):
                with zipfile.ZipFile(archive, "w") as source:
                    entry = zipfile.ZipInfo(name)
                    entry.filename = name  # preserve raw malicious separators on Windows
                    source.writestr(entry, b"bad")
                with self.assertRaises(ValueError):
                    provision.extract_zip(archive, root / "out", lambda name: name)
        for system, machine, expected in (("Windows", "AMD64", "win32-x64"), ("Darwin", "arm64", "darwin-arm64"), ("Linux", "aarch64", "linux-arm64")):
            with mock.patch.object(provision.platform, "system", return_value=system), mock.patch.object(provision.platform, "machine", return_value=machine):
                self.assertEqual(provision.platform_key(), expected)

    def test_download_rejects_corruption_and_other_hosts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaises(ValueError):
                provision.download({"url": "https://other.example/runtime", "integrity": "sha256-no"}, root / "one")
            response = io.BytesIO(b"changed-runtime")
            response.url = "https://nodejs.org/fixed-release"
            with mock.patch.object(provision.urllib.request, "urlopen", return_value=response):
                with self.assertRaises(ValueError):
                    provision.download({"url": response.url, "integrity": "sha256-" + "0" * 64}, root / "two")


if __name__ == "__main__":
    unittest.main()
