#!/usr/bin/env python3
"""Offline checkpoint checks with synthetic agents; no live services are changed."""
import contextlib
import getpass
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("checkpoint", ROOT / "scripts/checkpoint.py")
checkpoint = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(checkpoint)


class Checkpoints(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.folder = Path(self.temporary.name).resolve()
        self.home = self.folder / "source"
        self.home.mkdir()
        self.instance_id = str(uuid.uuid4())
        self.store_id = str(uuid.uuid4())
        self.session_id = str(uuid.uuid4())
        for name in ["secrets", "tls", "profiles/versions/one", "workspace/empty", "bin", "engine/releases/v1", "engine/accounts/chatgpt", "engine/locks"]:
            (self.home / name).mkdir(parents=True, exist_ok=True)
        self.document = {"revision":7,"config":{
            "schema_version":1,"instance_id":self.instance_id,"name":"Source individual",
            "owner":getpass.getuser(),"enabled":True,"setup_complete":True,
            "purpose":"Preserve the individual", "instructions":f"The old path {self.home}/workspace is literal discussion text.",
            "server":{"bind":"0.0.0.0","port":18451,"welcome_port":18452,"public_url":"https://source.test:18451","pam_service":"agent-instance","embed_origins":["https://fleet.test"],"certificate_file":None,"private_key_file":None},
            "backend":{"command":str(self.home / "engine/releases/v1/agent-runtime"),
                       "args":["--home",str(self.home / "engine"),"acp"],"working_directory":str(self.home / "workspace"),
                       "allowed_working_roots":[str(self.home / "workspace")],"secret_env":{"API_KEY":"provider-key"},"env":{"INLINE_SECRET":"opaque-env-credential"}},
            "limits":{"max_connections":8,"max_frame_bytes":4194304,"backend_start_timeout_seconds":30,"idle_timeout_seconds":1800,"event_retention":10000}}}
        checkpoint.save_json(self.home / "config.json", self.document)
        checkpoint.save_json(self.home / "engine/runtime.json", {
            "schema_version":1,"name":"Source individual","instructions":f"Mention {self.home} literally.",
            "workspace":str(self.home / "workspace"),"additional_workspaces":[str(self.home / "workspace/empty")],
            "mode":"workspace", "allow_client_stdio_mcp":True,"allow_client_system_prompt":True,
            "allowed_mcp_mutations":["something.write"], "trusted_mcp_readonly_urls":["https://source.test/context/mcp"],
            "provider":{"kind":"mock","model":"test","api_key_env":"API_KEY","auth":"chatgpt","chatgpt_credentials_file":str(self.home / "engine/accounts/chatgpt/credentials.json")}})
        for name, content in {
            "secrets/control-token":"original-control-credential", "secrets/provider-key":"original-provider-credential",
            "tls/root-ca.key":"original-tls-key", "tls/root-ca.crt":"original-ca",
            "engine/accounts/chatgpt/credentials.json":'{"refresh_token":"original-refresh-token"}',
            "profiles/communication.md":"Use brief reactions followed by the specific point.",
            "profiles/working-style.md":"Sketch the desired experience before architecture.",
            "profiles/identity.md":"Synthetic subject, separate from installation identity.",
            "profiles/versions/one/communication.md":"Earlier persona guidance.",
            "workspace/product.md":f"Product notes mentioning {self.home}; never rewrite this text.",
            "bin/agent-instance":"fixture binary", "engine/releases/v1/agent-runtime":"fixture engine",
            "agent-instance.service":"Do not install this job", "install.json":"old registration",
            "endpoints.json":"old endpoint", "service.stdout.log":"old operational log",
        }.items():
            (self.home / name).write_text(content)
        checkpoint.save_json(self.home / "context-providers.json", {"providers":[{"id":"slack","secret_ref":"provider-key","url":"https://slack.test/context/v1/query"}]})
        with contextlib.closing(sqlite3.connect(self.home / "state.sqlite3")) as db:
            db.executescript("""
              CREATE TABLE records(collection TEXT,id TEXT,value TEXT,updated_at INTEGER,PRIMARY KEY(collection,id));
              CREATE TABLE tokens(hash TEXT PRIMARY KEY,id TEXT,name TEXT,scopes TEXT,expires_at INTEGER,created_at INTEGER);
              CREATE TABLE browser_sessions(hash TEXT,owner TEXT,csrf TEXT,expires_at INTEGER);
              CREATE TABLE login_attempts(key TEXT,failures INTEGER,locked_until INTEGER);
              CREATE TABLE events(seq INTEGER PRIMARY KEY,kind TEXT,data TEXT);
            """)
            for collection, key, value in [
                ("context_bindings",self.session_id,{"owner":True}),
                ("native_history",self.session_id,{"store_id":self.store_id,"session_id":self.session_id}),
                ("session_access",self.session_id,{"actor":"old-token","owner":True}),
                ("sessions",self.session_id,{"cwd":str(self.home / "workspace")}),
                ("scheduler","settings",{"revision":3,"enabled":True,"paused":False,"approved_backend_sha256":"old-runtime"}),
                ("organization","current",{"schema":"agent-instance.organization/1","revision":1,"organization":{"assignments":[{"id":"project","active":True}],"relationships":[{"id":"peer","active":True,"send":True,"receive":True,"allow_details":True}]}}),
                ("work","work-one",{"goal":f"Build useful tools in {self.home}","state":"active"}),
                ("memory","memory-one",{"statement":"Use the desired experience as the starting point."}),
            ]:
                db.execute("INSERT INTO records VALUES(?,?,?,?)", (collection,key,json.dumps(value),1))
            db.execute("INSERT INTO tokens VALUES(?,?,?,?,?,?)", (hashlib.sha256(b"original-control-credential").hexdigest(),"old-control","Local installation control",'["admin"]',9999999999999,1))
            db.execute("INSERT INTO tokens VALUES(?,?,?,?,?,?)", ("remote-hash","remote-id","Slack connector",'["acp"]',9999999999999,1))
            db.execute("INSERT INTO browser_sessions VALUES('browser','owner','csrf',9999999999999)")
            db.execute("INSERT INTO events VALUES(1,'past',?)",(json.dumps({"instance_id":self.instance_id,"path":str(self.home)}),))
            db.commit()
        self.items = json.dumps([{"role":"user","content":f"Transcript mentions {self.home}/workspace; preserve it exactly."}])
        with contextlib.closing(sqlite3.connect(self.home / "engine/history.sqlite3")) as db:
            db.executescript("""
                CREATE TABLE history_identity(id INTEGER PRIMARY KEY,value TEXT);
                CREATE TABLE sessions(id TEXT PRIMARY KEY,cwd TEXT,items TEXT);
                CREATE TABLE context_checkpoints(session TEXT,data TEXT);
            """)
            db.execute("INSERT INTO history_identity VALUES(1,?)", (self.store_id,))
            db.execute("INSERT INTO sessions VALUES(?,?,?)", (self.session_id,str(self.home / "workspace"),self.items))
            db.execute("INSERT INTO context_checkpoints VALUES(?,?)",(self.session_id,'{"opaque":"provider continuation"}'))
            db.commit()

    def export(self, secret_policy="include", binaries=False):
        archive = self.folder / (str(uuid.uuid4()) + ".zip")
        result = checkpoint.export_checkpoint(self.home, archive, secret_policy, binaries)
        self.assertEqual(result["archive_sha256"], checkpoint.sha_file(archive))
        return archive

    def record(self, home, collection, key):
        with contextlib.closing(sqlite3.connect(home / "state.sqlite3")) as db:
            row = db.execute("SELECT value FROM records WHERE collection=? AND id=?",(collection,key)).fetchone()
            return json.loads(row[0]) if row else None

    def test_restore_preserves_individual_profiles_history_and_relocates_only_known_paths(self):
        archive = self.export(binaries=True)
        dest = self.folder / "restored"
        result = checkpoint.restore_checkpoint(archive,dest,"restore","include")
        config = checkpoint.load_json(dest / "config.json")["config"]
        self.assertEqual(config["instance_id"],self.instance_id)
        self.assertFalse(config["enabled"])
        self.assertEqual(config["instructions"],self.document["config"]["instructions"])
        self.assertEqual(config["backend"]["args"][1],str(dest / "engine"))
        self.assertEqual(config["backend"]["working_directory"],str(dest / "workspace"))
        self.assertEqual((dest / "secrets/control-token").read_text(),"original-control-credential")
        self.assertEqual((dest / "profiles/versions/one/communication.md").read_text(),"Earlier persona guidance.")
        self.assertTrue((dest / "workspace/empty").is_dir())
        self.assertEqual((dest / "workspace/product.md").read_bytes(),(self.home / "workspace/product.md").read_bytes())
        self.assertTrue((dest / "bin/agent-instance").is_file())
        self.assertFalse((dest / "agent-instance.service").exists())
        self.assertFalse((dest / "install.json").exists())
        self.assertFalse(result["service_registered"])
        self.assertFalse(result["service_started"])
        self.assertTrue(self.record(dest,"scheduler","settings")["paused"])
        self.assertEqual(self.record(dest,"work","work-one"),self.record(self.home,"work","work-one"))
        self.assertEqual(self.record(dest,"context_bindings",self.session_id),{"owner":True})
        with contextlib.closing(sqlite3.connect(dest / "engine/history.sqlite3")) as db:
            self.assertEqual(db.execute("SELECT value FROM history_identity").fetchone()[0],self.store_id)
            row = db.execute("SELECT cwd,items FROM sessions").fetchone()
            self.assertEqual(row,(str(dest / "workspace"),self.items))
        with contextlib.closing(sqlite3.connect(dest / "state.sqlite3")) as db:
            self.assertEqual(db.execute("SELECT count(*) FROM tokens").fetchone()[0],2)
            self.assertEqual(db.execute("SELECT count(*) FROM browser_sessions").fetchone()[0],0)

    def test_clone_has_new_identity_and_no_inherited_access_but_keeps_knowledge(self):
        dest = self.folder / "cloned"
        result = checkpoint.restore_checkpoint(self.export(),dest,"clone",name="New individual")
        self.assertNotEqual(result["instance_id"],self.instance_id)
        self.assertEqual(result["name"],"New individual")
        self.assertFalse((dest / "secrets/provider-key").exists())
        self.assertFalse((dest / "engine/accounts/chatgpt/credentials.json").exists())
        self.assertFalse((dest / "tls/root-ca.key").exists())
        self.assertNotEqual((dest / "secrets/control-token").read_text(),"original-control-credential")
        self.assertEqual(checkpoint.load_json(dest / "context-providers.json"),{"providers":[]})
        for collection in checkpoint.CONTEXT_RECORDS:
            self.assertIsNone(self.record(dest,collection,self.session_id))
        organization = self.record(dest,"organization","current")["organization"]
        self.assertFalse(organization["assignments"][0]["active"])
        self.assertFalse(organization["relationships"][0]["send"])
        native = checkpoint.load_json(dest / "engine/runtime.json")
        self.assertEqual(native["name"],"New individual")
        self.assertEqual(native["trusted_mcp_readonly_urls"],[])
        self.assertEqual(native["allowed_mcp_mutations"],[])
        self.assertFalse(native["allow_client_stdio_mcp"])
        self.assertEqual(native["mode"],"ask")
        self.assertEqual(self.record(dest,"memory","memory-one"),self.record(self.home,"memory","memory-one"))
        with contextlib.closing(sqlite3.connect(dest / "engine/history.sqlite3")) as db:
            self.assertNotEqual(db.execute("SELECT value FROM history_identity").fetchone()[0],self.store_id)
            self.assertEqual(db.execute("SELECT items FROM sessions").fetchone()[0],self.items)
        with contextlib.closing(sqlite3.connect(dest / "state.sqlite3")) as db:
            self.assertEqual(db.execute("SELECT count(*) FROM tokens").fetchone()[0],1)

    def test_excluding_managed_credentials_scrubs_state_and_does_not_capture_binaries(self):
        archive = self.export("exclude")
        with checkpoint.verified_archive(archive) as (bundle,manifest):
            self.assertEqual(manifest["managed_credentials"],"exclude")
            self.assertNotIn("payload/secrets/control-token",bundle.namelist())
            self.assertNotIn("payload/bin/agent-instance",bundle.namelist())
            document = json.loads(bundle.read("payload/config.json"))
            self.assertEqual(document["config"]["backend"]["env"],{})
            self.assertEqual(document["config"]["backend"]["secret_env"],{})
            db_file = self.folder / "checked.sqlite3"
            db_file.write_bytes(bundle.read("payload/state.sqlite3"))
        with contextlib.closing(sqlite3.connect(db_file)) as db:
            self.assertEqual(db.execute("SELECT count(*) FROM tokens").fetchone()[0],0)
        with self.assertRaisesRegex(ValueError,"excluded credentials"):
            checkpoint.restore_checkpoint(archive,self.folder / "impossible","restore","include")
        dest = self.folder / "restored-without-credentials"
        checkpoint.restore_checkpoint(archive,dest,"restore","exclude")
        self.assertTrue((dest / "secrets/control-token").is_file())
        self.assertFalse((dest / "secrets/provider-key").exists())
        self.assertEqual(checkpoint.load_json(dest / "config.json")["config"]["instance_id"],self.instance_id)

    def test_excluding_credentials_also_clears_external_oauth_reference(self):
        native = checkpoint.load_json(self.home / "engine/runtime.json")
        native["provider"]["chatgpt_credentials_file"] = str(self.folder / "external-account.json")
        native["provider"]["api_key_env"] = ""
        checkpoint.save_json(self.home / "engine/runtime.json", native)
        dest = self.folder / "clone-external"
        checkpoint.restore_checkpoint(self.export(),dest,"clone",name="Independent")
        provider = checkpoint.load_json(dest / "engine/runtime.json")["provider"]
        self.assertEqual(provider["auth"],"auto")
        self.assertIsNone(provider["chatgpt_credentials_file"])
        self.assertEqual(provider["api_key_env"],"AGENT_MODEL_API_KEY")

    def test_active_service_native_turn_or_database_writer_is_refused(self):
        for lock in [self.home / "service.lock", self.home / "engine/locks/session-one"]:
            with checkpoint.lock_file(lock,"fixture"):
                with self.assertRaisesRegex(ValueError,"running"):
                    self.export()
        with contextlib.closing(sqlite3.connect(self.home / "state.sqlite3")) as db:
            db.execute("BEGIN IMMEDIATE")
            with self.assertRaisesRegex(ValueError,"writer is active"):
                self.export()
            db.rollback()
        self.assertFalse(list(self.folder.glob("*.zip")))

    def test_wal_commits_are_in_the_sqlite_backup(self):
        with contextlib.closing(sqlite3.connect(self.home / "state.sqlite3")) as db:
            db.execute("PRAGMA journal_mode=WAL")
            db.execute("PRAGMA wal_autocheckpoint=0")
            db.execute("INSERT INTO events VALUES(2,'wal-committed','preserve latest commit')")
            db.commit()
            archive = self.export()
            self.assertTrue((self.home / "state.sqlite3-wal").exists())
        dest = self.folder / "wal-restored"
        checkpoint.restore_checkpoint(archive,dest,"restore","include")
        with contextlib.closing(sqlite3.connect(dest / "state.sqlite3")) as db:
            self.assertEqual(db.execute("SELECT data FROM events WHERE seq=2").fetchone()[0],"preserve latest commit")

    def rewrite_zip(self, archive, transform):
        bad = self.folder / (str(uuid.uuid4()) + ".zip")
        with zipfile.ZipFile(archive) as src, zipfile.ZipFile(bad,"w") as dst:
            values = {info.filename:src.read(info) for info in src.infolist()}
            transform(values)
            for name,content in values.items():
                dst.writestr(name,content)
        return bad

    def test_hash_mismatch_and_unsafe_archive_paths_never_publish(self):
        archive = self.export()
        bad = self.rewrite_zip(archive,lambda files:files.update({"payload/profiles/communication.md":b"tampered"}))
        for item in [bad]:
            with self.assertRaisesRegex(ValueError,"size|hash"):
                checkpoint.restore_checkpoint(item,self.folder / "bad","restore","include")
            self.assertFalse((self.folder / "bad").exists())
        with self.assertRaisesRegex(ValueError,"SHA-256"):
            checkpoint.restore_checkpoint(archive,self.folder / "bad","restore","include",expected_sha256="0"*64)
        for name in ["payload/../escaped", "/tmp/escaped", "payload/C:/escaped", "payload/a\\b", "payload/CON", "payload/name."]:
            malformed = self.rewrite_zip(archive,lambda files:files.update({name:b"unsafe"}))
            with self.assertRaises(ValueError):
                checkpoint.restore_checkpoint(malformed,self.folder / "bad","restore","include")
            self.assertFalse((self.folder / "bad").exists())
        linked = self.folder / "linked.zip"
        with zipfile.ZipFile(archive) as src, zipfile.ZipFile(linked,"w") as dst:
            for info in src.infolist():
                dst.writestr(info,src.read(info))
            link = zipfile.ZipInfo("payload/symlink")
            link.create_system = 3
            link.external_attr = (stat.S_IFLNK | 0o777) << 16
            dst.writestr(link,"../../outside")
        with self.assertRaisesRegex(ValueError,"link"):
            checkpoint.restore_checkpoint(linked,self.folder / "bad","restore","include")

    def test_existing_destinations_and_input_links_are_never_overwritten(self):
        archive = self.export()
        existing = self.folder / "existing"
        existing.mkdir()
        marker = existing / "keep"
        marker.write_text("unchanged")
        with self.assertRaisesRegex(ValueError,"already exists"):
            checkpoint.restore_checkpoint(archive,existing,"restore","include")
        self.assertEqual(marker.read_text(),"unchanged")
        stage = self.folder / "stage"
        stage.mkdir()
        empty = self.folder / "empty"
        empty.mkdir()
        with self.assertRaises(OSError):
            checkpoint.publish_directory(stage,empty)
        if os.name != "nt":
            (self.home / "workspace/link").symlink_to(self.home / "profiles/communication.md")
            with self.assertRaisesRegex(ValueError,"links"):
                self.export()

    def test_path_relocation_is_component_based_and_external_dependencies_are_reported(self):
        deps = []
        self.assertEqual(checkpoint.relocate_path(r"C:\Agents\Alex\workspace",r"C:\Agents\Alex",Path("/new"),deps,"workspace"),str(Path("/new") / "workspace"))
        self.assertEqual(checkpoint.relocate_path("/old-ish/file","/old",Path("/new"),deps,"command"),"/old-ish/file")
        self.assertEqual(deps[0]["path"],"/old-ish/file")
        self.assertEqual(checkpoint.relocate_path("${AGENT_HOME}/workspace","/old",Path("/new"),deps,"cwd"),"${AGENT_HOME}/workspace")

    @unittest.skipIf(os.name == "nt", "Directory-alias fixture needs unprivileged Unix symlinks")
    def test_source_parent_alias_is_retained_for_known_paths_only(self):
        alias = self.folder / "parent-alias"
        alias.symlink_to(self.folder, target_is_directory=True)
        aliased_home = alias / self.home.name
        config = checkpoint.load_json(self.home / "config.json")
        config["config"]["backend"]["args"][1] = str(aliased_home / "engine")
        config["config"]["instructions"] = f"Retain literal {aliased_home}/engine."
        checkpoint.save_json(self.home / "config.json", config)
        archive = self.folder / "aliased.zip"
        checkpoint.export_checkpoint(aliased_home, archive, "exclude")
        with checkpoint.verified_archive(archive) as (_, manifest):
            self.assertIn(str(aliased_home), manifest["source_home_aliases"])
        dest = self.folder / "restored-alias"
        checkpoint.restore_checkpoint(archive, dest, "restore", "exclude")
        restored = checkpoint.load_json(dest / "config.json")["config"]
        self.assertEqual(restored["backend"]["args"][1], str(dest / "engine"))
        self.assertEqual(restored["instructions"], config["config"]["instructions"])
        with self.assertRaisesRegex(ValueError, "outside the installation"):
            checkpoint.export_checkpoint(self.home, aliased_home / "inside.zip", "exclude")

    def test_cli_requires_explicit_credential_policy_and_outputs_no_secret_values(self):
        script = ROOT / "scripts/checkpoint.py"
        missing = subprocess.run([sys.executable,str(script),"export","--home",str(self.home),"--output",str(self.folder / "missing.zip")],capture_output=True,text=True)
        self.assertNotEqual(missing.returncode,0)
        archive = self.folder / "cli.zip"
        captured = subprocess.run([sys.executable,str(script),"export","--home",str(self.home),"--output",str(archive),"--secrets","exclude"],capture_output=True,text=True,check=True)
        value = json.loads(captured.stdout)
        self.assertEqual(value["operation"],"export")
        self.assertNotIn("original-provider-credential",captured.stdout + captured.stderr)
        inspected = subprocess.run([sys.executable,str(script),"inspect","--archive",str(archive),"--sha256",value["archive_sha256"]],capture_output=True,text=True,check=True)
        self.assertTrue(json.loads(inspected.stdout)["verified"])

    @unittest.skipUnless(os.environ.get("CHECKPOINT_INSTANCE_BINARY") and os.environ.get("CHECKPOINT_RUNTIME_BINARY"),
                         "Set CHECKPOINT_INSTANCE_BINARY and CHECKPOINT_RUNTIME_BINARY for real Rust service integration")
    def test_real_rust_service_locks_and_restored_configuration(self):
        instance = str(Path(os.environ["CHECKPOINT_INSTANCE_BINARY"]).resolve())
        runtime = str(Path(os.environ["CHECKPOINT_RUNTIME_BINARY"]).resolve())
        real_home = self.folder / "real-source"
        def run(*args):
            return subprocess.run(args,check=True,capture_output=True,text=True,timeout=30)
        run(instance,"--home",str(real_home),"init","--name","Checkpoint fixture","--bind","127.0.0.1")
        (real_home / "workspace").mkdir(exist_ok=True)
        run(runtime,"--home",str(real_home / "engine"),"init","--workspace",str(real_home / "workspace"),"--provider","mock","--name","Checkpoint fixture")
        document = checkpoint.load_json(real_home / "config.json")
        document["config"]["backend"] = {"command":runtime,"args":["--home",str(real_home / "engine"),"acp"],
                                             "env":{},"secret_env":{},"working_directory":str(real_home / "workspace"),"allowed_working_roots":[]}
        document["config"].update(setup_complete=True,enabled=False)
        checkpoint.save_json(real_home / "config.json",document)
        with tempfile.TemporaryFile() as log:
            process = subprocess.Popen([instance,"--home",str(real_home),"serve"],stdout=log,stderr=log)
            try:
                deadline = time.monotonic() + 10
                while not (real_home / "endpoints.json").exists() and process.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.05)
                self.assertIsNone(process.poll(),"Disposable fixture service failed to start")
                self.assertTrue((real_home / "endpoints.json").exists())
                with self.assertRaisesRegex(ValueError,"service is running"):
                    checkpoint.export_checkpoint(real_home,self.folder / "refused.zip","exclude")
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        archive = self.folder / "real.zip"
        checkpoint.export_checkpoint(real_home,archive,"exclude")
        for operation in ("restore","clone"):
            target = self.folder / ("real-" + operation)
            checkpoint.restore_checkpoint(archive,target,operation,name="Real clone" if operation == "clone" else None)
            run(instance,"--home",str(target),"config","validate")
            run(runtime,"--home",str(target / "engine"),"config-validate",str(target / "engine/runtime.json"))


if __name__ == "__main__":
    unittest.main()
