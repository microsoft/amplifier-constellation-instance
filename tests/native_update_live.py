#!/usr/bin/env python3
"""Update and roll back a DISPOSABLE native installation, preserving its records."""
import argparse
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--allow-disposable-mutations", action="store_true", required=True)
    args = parser.parse_args()
    home = args.home.resolve()
    binary = home / "bin" / ("agent-instance.exe" if os.name == "nt" else "agent-instance")
    helper = Path(__file__).resolve().parent.parent / "scripts/native-install.py"
    def status():
        return json.loads(subprocess.check_output([str(binary), "status", "--home", str(home)], text=True))
    before = status()
    config = (home / "config.json").read_bytes()
    token_hash = hashlib.sha256((home / "secrets/control-token").read_bytes()).hexdigest()
    def profile_hashes():
        return {str(p.relative_to(home / "profiles")): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in (home / "profiles").rglob("*") if p.is_file()}
    profiles = profile_hashes()
    authority_hash = hashlib.sha256((home / "tls/root-ca.crt").read_bytes()).hexdigest()
    with closing(sqlite3.connect(home / "state.sqlite3")) as db:
        records = db.execute("SELECT collection,id,value FROM records ORDER BY collection,id").fetchall()
    original = json.loads((home / "install.json").read_text())["current"]
    updated = json.loads(subprocess.check_output([sys.executable, str(helper), "update", "--home", str(home), "--binary", str(args.candidate.resolve())], text=True))
    assert updated["running_verified"] and updated["release"]["id"] != original["id"]
    after_update = status()
    assert after_update["instance_id"] == before["instance_id"]
    rolled_back = json.loads(subprocess.check_output([sys.executable, str(helper), "rollback", "--home", str(home)], text=True))
    assert rolled_back["running_verified"] and rolled_back["release"] == original
    assert status()["instance_id"] == before["instance_id"]
    assert (home / "config.json").read_bytes() == config
    assert hashlib.sha256((home / "secrets/control-token").read_bytes()).hexdigest() == token_hash
    assert profile_hashes() == profiles
    assert hashlib.sha256((home / "tls/root-ca.crt").read_bytes()).hexdigest() == authority_hash
    with closing(sqlite3.connect(home / "state.sqlite3")) as db:
        assert db.execute("SELECT collection,id,value FROM records ORDER BY collection,id").fetchall() == records
    print(json.dumps({"instance_id": before["instance_id"], "from_version": before["version"],
        "updated_version": after_update["version"], "restored_version": status()["version"],
        "preserved": ["identity", "configuration", "control credential", "profiles", "recorded views and sessions", "trusted CA"],
        "startup_job_restart": "verified twice", "backups": len(list((home / "backups").glob("*/state.sqlite3")))}, indent=2))


if __name__ == "__main__":
    main()
