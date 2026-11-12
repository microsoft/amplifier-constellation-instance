#!/usr/bin/env python3
"""Installer transactions and native registration contracts without real services."""
import argparse
from contextlib import closing
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("native", ROOT / "scripts/native-install.py")
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


class NativeInstallTests(unittest.TestCase):
    def test_missing_published_engine_platform_refuses_before_creating_home(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = root / 'bundle'; (bundle / 'setup').mkdir(parents=True)
            (bundle / 'setup/native-runtime-lock.json').write_text('{"assets":[]}')
            home = root / 'never-created'
            args = argparse.Namespace(home=home, bundle=bundle, skip_runtime=False,
                                      engine='native', runtime_binary=None)
            with mock.patch.object(native, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'no installation was created'):
                    native.install(args)
                run.assert_not_called()
            self.assertFalse(home.exists())

    def test_linux_user_startup_quotes_paths_without_expanding_dollar_or_percent(self):
        with tempfile.TemporaryDirectory() as temporary:
            owner = Path(temporary)
            home = owner / 'agent $literal %test'
            with mock.patch.object(native.sys, 'platform', 'linux'), mock.patch.object(native.Path, 'home', return_value=owner), mock.patch.object(native, 'run') as run:
                service = native.make_service(home, {'instance_id': '96b452bc-47f1-4b6a-8bb9-697fc6ff27ae'})
                text = (owner / '.config/systemd/user' / service['name']).read_text()
                working = next(line for line in text.splitlines() if line.startswith('WorkingDirectory='))
                command = next(line for line in text.splitlines() if line.startswith('ExecStart='))
                self.assertIn('$literal %%test', working)
                self.assertNotIn('$$literal', working)
                self.assertIn('$$literal %%test', command)
                self.assertEqual(service['kind'], 'systemd-user')
                self.assertEqual([call.args[0][2] for call in run.call_args_list], ['daemon-reload', 'enable'])

    def instance(self, root):
        home = root / "agent space $literal"
        home.mkdir()
        config = {"config": {"instance_id": "96b452bc-47f1-4b6a-8bb9-697fc6ff27ae", "owner": "test-owner"}}
        native.save(home / "config.json", config)
        (home / "profiles").mkdir()
        (home / "profiles/communication.md").write_text("Ask specific questions.")
        with closing(sqlite3.connect(home / "state.sqlite3")) as db:
            db.execute("CREATE TABLE history(value TEXT)")
            db.execute("INSERT INTO history VALUES('preserve this conversation')")
            db.commit()
        old = self.release(home, "0.7.0", b"first executable")
        new = self.release(home, "0.7.1", b"second executable")
        native.activate(home, old)
        native.save(home / "install.json", {"manager": "native-install/1", "instance_id": config["config"]["instance_id"],
                    "current": new, "previous": old, "service": None})
        return home, old, new

    def release(self, home, version, content):
        digest = hashlib.sha256(content).hexdigest()
        info = {"id": version + "-" + digest[:12], "version": version, "sha256": digest}
        folder = home / "releases" / info["id"]
        folder.mkdir(parents=True)
        (folder / native.binary_name()).write_bytes(content)
        native.save(folder / "release.json", info)
        return info

    def test_rollback_preserves_identity_history_profiles_and_saves_snapshot(self):
        with tempfile.TemporaryDirectory() as temporary:
            home, old, new = self.instance(Path(temporary))
            config_before = (home / "config.json").read_bytes()
            args = argparse.Namespace(home=home, command="rollback", release=None)
            with mock.patch.object(native, "run", return_value='{"valid":true}'):
                result = native.switch(args)
            self.assertEqual(result["release"], old)
            self.assertEqual(result["previous"], new)
            self.assertEqual((home / "config.json").read_bytes(), config_before)
            self.assertEqual((home / "profiles/communication.md").read_text(), "Ask specific questions.")
            with closing(sqlite3.connect(home / "state.sqlite3")) as db:
                self.assertEqual(db.execute("SELECT value FROM history").fetchone()[0], "preserve this conversation")
            self.assertEqual(len(list((home / "backups").glob("*/state.sqlite3"))), 1)

    def test_bad_release_is_rejected_without_replacing_executable(self):
        with tempfile.TemporaryDirectory() as temporary:
            home, old, _ = self.instance(Path(temporary))
            original = (home / "bin" / native.binary_name()).read_bytes()
            (home / "releases" / old["id"] / native.binary_name()).write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "checksum"):
                native.activate(home, old)
            self.assertEqual((home / "bin" / native.binary_name()).read_bytes(), original)

    def test_failed_health_restores_prior_binary_without_rewinding_database(self):
        with tempfile.TemporaryDirectory() as temporary:
            home, old, new = self.instance(Path(temporary))
            metadata = json.loads((home / "install.json").read_text())
            metadata.update(current=old, previous=new, service={"kind": "test", "name": "one"})
            native.save(home / "install.json", metadata)
            args = argparse.Namespace(home=home, command="rollback", release=None)
            with mock.patch.object(native, "run", return_value='{"valid":true}'), mock.patch.object(native, "service_command") as service, mock.patch.object(native, "health", side_effect=[ValueError("fixture failure"), {}]):
                with self.assertRaisesRegex(ValueError, "prior executable was restored"):
                    native.switch(args)
            self.assertEqual(json.loads((home / "install.json").read_text())["current"], old)
            self.assertEqual((home / "bin" / native.binary_name()).read_bytes(), b"first executable")
            self.assertEqual([x.args[1] for x in service.call_args_list], ["stop", "start", "stop", "start"])

    def test_foreign_installation_and_path_injection_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            home, _, _ = self.instance(Path(temporary))
            metadata = json.loads((home / "install.json").read_text())
            metadata["instance_id"] = "another-agent"
            native.save(home / "install.json", metadata)
            with self.assertRaisesRegex(ValueError, "identity differs"):
                native.switch(argparse.Namespace(home=home, command="rollback", release=None))
            for value in ['bad"path', 'bad\npath']:
                with self.assertRaises(ValueError):
                    native.validate_home(Path(value))

    def test_launchd_paths_are_structured_and_instance_specific(self):
        if os.name == "nt":
            self.skipTest("Mac registration format is checked on Unix")
        import plistlib
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            home, _, _ = self.instance(root)
            config = json.loads((home / "config.json").read_text())["config"]
            with mock.patch.object(native.sys, "platform", "darwin"), mock.patch.object(native.Path, "home", return_value=root):
                service = native.make_service(home, config)
            document = plistlib.loads(Path(service["file"]).read_bytes())
            self.assertEqual(document["ProgramArguments"], [str(home / "bin" / native.binary_name()), "serve", "--home", str(home)])
            self.assertIn(config["instance_id"], service["name"])
            self.assertEqual(document["Umask"], 0o077)


if __name__ == "__main__":
    unittest.main()
