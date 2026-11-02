#!/usr/bin/env python3
"""Offline checkpoint/export, restore and clone of one independent installation.

No server, model, installer or startup registration is invoked. Output is JSON;
managed credentials are never printed. Python 3.10+; standard library only.
"""
import argparse
import contextlib
import csv
import ctypes
import datetime
import hashlib
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import platform
import re
import secrets
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import unicodedata
import uuid
import zipfile


SCHEMA = "agent-instance.checkpoint/1"
MAX_FILES = 100_000
MAX_BYTES = 20 * 1024**3
MAX_MANIFEST = 16 * 1024**2
SQLITE_MAGIC = b"SQLite format 3\x00"
EPHEMERAL = {"service.lock", ".installation.lock", "endpoints.json", "runtime.json",
             "agent-instance.service", "install.json", "checkpoint-result.json"}
RETIRED = {"backups", "upgrades", "logs", ".git"}
AUTH_TABLES = ("tokens", "browser_sessions", "login_attempts")
CONTEXT_RECORDS = ("context_bindings", "native_history", "session_access")


def fail(message):
    raise ValueError(message)


def current_owner():
    if os.name != "nt":
        import pwd
        return pwd.getpwuid(os.getuid()).pw_name
    from ctypes import wintypes
    size = wintypes.ULONG(32768)
    value = ctypes.create_unicode_buffer(size.value)
    operation = ctypes.WinDLL("secur32", use_last_error=True).GetUserNameExW
    operation.argtypes = [ctypes.c_int, wintypes.LPWSTR, ctypes.POINTER(wintypes.ULONG)]
    if not operation(2, value, ctypes.byref(size)):  # NameSamCompatible
        fail("Cannot identify the current Windows account")
    return value.value


def private_path(path, directory=False):
    if os.name != "nt":
        path.chmod(0o700 if directory else 0o600)
        return
    # Apply a protected owner+SYSTEM DACL before writing credentials. chmod alone
    # does not make Windows files private. No shell or user input is executed.
    from ctypes import wintypes
    whoami = Path(os.environ["SystemRoot"]) / "System32/whoami.exe"
    rows = subprocess.run([str(whoami), "/user", "/fo", "csv", "/nh"],
                          check=True, capture_output=True, text=True, timeout=10).stdout
    sid = next(csv.reader(rows.splitlines()))[1]
    if not re.fullmatch(r"S-1-[0-9-]+", sid):
        fail("Cannot resolve the current Windows account SID")
    library = ctypes.WinDLL("advapi32", use_last_error=True)
    convert = library.ConvertStringSecurityDescriptorToSecurityDescriptorW
    convert.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, ctypes.POINTER(ctypes.c_void_p), ctypes.c_void_p]
    descriptor = ctypes.c_void_p()
    if not convert(f"D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)", 1, ctypes.byref(descriptor), None):
        fail("Cannot construct private checkpoint permissions")
    try:
        apply = library.SetFileSecurityW
        apply.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, ctypes.c_void_p]
        if not apply(str(path), 0x80000004, descriptor):
            fail("Cannot make the checkpoint path private")
    finally:
        free = ctypes.WinDLL("kernel32").LocalFree
        free.argtypes = [ctypes.c_void_p]
        free(descriptor)


def load_json(path, maximum=MAX_MANIFEST):
    if path.stat().st_size > maximum:
        fail("JSON metadata exceeds the supported size")
    return json.loads(path.read_text(encoding="utf-8"))


def save_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    content = (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()
    with path.open("wb") as out:
        out.write(content)
        out.flush()
        os.fsync(out.fileno())
    path.chmod(0o600)


def safe_name(name):
    """Use the intersection of ordinary relative Linux/macOS/Windows paths."""
    if not isinstance(name, str) or not name or len(name.encode()) > 2048:
        fail("Archive contains an invalid path")
    if "\\" in name or name.startswith("/") or any(ord(c) < 32 for c in name):
        fail("Archive contains an unsafe path")
    parts = name.split("/")
    for part in parts:
        if not part or part in (".", "..") or part.endswith((" ", ".")) or any(c in part for c in ':<>"|?*'):
            fail("Archive contains an unsafe path")
        if re.fullmatch(r"(?i)(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?", part):
            fail("Archive path is reserved on Windows")
    return name


def under(name, root):
    return name == root or name.startswith(root + "/")


def archive_key(name):
    return unicodedata.normalize("NFC", name).casefold()


def stat_key(path):
    value = path.lstat()
    return (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_mode)


def no_link(path):
    value = path.lstat()
    if stat.S_ISLNK(value.st_mode) or getattr(value, "st_file_attributes", 0) & 0x400:
        fail("Symbolic links and Windows reparse points are not checkpoint payloads")
    return value


@contextlib.contextmanager
def lock_file(path, message):
    if path.exists():
        no_link(path)
    descriptor = os.open(path, os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0), 0o600)
    with os.fdopen(descriptor, "r+b") as handle:
        try:
            if os.name == "nt":
                import msvcrt
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            raise ValueError(message) from error
        try:
            yield
        finally:
            if os.name == "nt":
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle, fcntl.LOCK_UN)


def instance_document(home):
    no_link(home / "config.json")
    value = load_json(home / "config.json")
    config = value.get("config", {})
    uuid.UUID(config.get("instance_id", ""))
    if config.get("schema_version") != 1 or not isinstance(value.get("revision"), int):
        fail("This helper requires an agent-instance schema 1 installation")
    return value


def relative_inside(value, home):
    if not isinstance(value, str):
        return None
    value = value.replace("${AGENT_HOME}", str(home))
    path = Path(value)
    if not path.is_absolute():
        return None
    try:
        # System directory aliases (for example /tmp -> /private/tmp on macOS)
        # must not turn an installation's own credential file into an external path.
        relative = path.resolve().relative_to(home.resolve())
        return relative.as_posix() if relative.parts else None
    except ValueError:
        return None


def native_layout(home, document):
    roots = set()
    backend = document["config"].get("backend") or {}
    if Path(backend.get("command", "")).name in ("agent-runtime", "agent-runtime.exe"):
        args = backend.get("args", [])
        for index, value in enumerate(args):
            target = args[index + 1] if value == "--home" and index + 1 < len(args) else value[7:] if value.startswith("--home=") else None
            if target:
                relative = relative_inside(target, home)
                if relative and (home / relative / "runtime.json").is_file():
                    roots.add(safe_name(relative))
    if (home / "engine/runtime.json").is_file():
        roots.add("engine")
    credential_paths = {"secrets", "tls", "backend/provider/auth.json", "backend/provider/accounts",
                        "backend/provider/secrets", "backend/provider/.credentials.json"}
    config = document["config"]
    for field in ("private_key_file", "certificate_file"):
        path = relative_inside(config.get("server", {}).get(field), home)
        if path:
            credential_paths.add(safe_name(path))
    for root in roots:
        credential_paths.update((root + "/accounts", root + "/secrets"))
        provider = load_json(home / root / "runtime.json").get("provider", {})
        path = relative_inside(provider.get("chatgpt_credentials_file"), home)
        if path:
            credential_paths.add(safe_name(path))
    return sorted(roots), sorted(credential_paths)


def excluded(name, roots, credential_paths, include_secrets, binaries):
    if name in EPHEMERAL or name.split("/")[0] in RETIRED or name.startswith(".checkpoint-"):
        return True
    if "/" not in name and (name.endswith((".log", ".pid", ".plist", ".service")) or name.startswith(".native-config-")):
        return True
    if any(under(name, root + "/locks") for root in roots):
        return True
    if not include_secrets and any(under(name, root) for root in credential_paths):
        return True
    binary_roots = ["bin", "releases", "backend/runtime", "backend/node_modules"]
    binary_roots += [root + suffix for root in roots for suffix in ("/bin", "/releases")]
    return not binaries and any(under(name, root) for root in binary_roots)


def inventory(home, roots, credential_paths, include_secrets, binaries):
    files, directories = {}, []
    seen = set()

    def visit(folder):
        for path in sorted(folder.iterdir()):
            name = path.relative_to(home).as_posix()
            if excluded(name, roots, credential_paths, include_secrets, binaries):
                continue
            safe_name(name)
            key = archive_key(name)
            if key in seen:
                fail("Installation has paths that collide on a case-insensitive filesystem")
            seen.add(key)
            value = no_link(path)
            if stat.S_ISDIR(value.st_mode):
                directories.append(name)
                visit(path)
            elif stat.S_ISREG(value.st_mode):
                if value.st_nlink > 1:
                    fail("Hard-linked files must be copied to independent ordinary files before checkpointing")
                files[name] = stat_key(path)
            else:
                fail("Stop the agent and remove transient sockets or device files before checkpointing")
    visit(home)
    if len(files) + len(directories) > MAX_FILES or sum(v[2] for v in files.values()) > MAX_BYTES:
        fail("Installation exceeds checkpoint limits (100000 paths / 20 GiB)")
    return files, sorted(directories)


def sqlite_file(path):
    with path.open("rb") as handle:
        return handle.read(16) == SQLITE_MAGIC


def db_tables(db):
    return {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}


def clear_auth(db):
    for table in AUTH_TABLES:
        if table in db_tables(db):
            db.execute(f"DELETE FROM {table}")


def sanitize_staged_credentials(stage, native_roots):
    document = load_json(stage / "config.json")
    config = document["config"]
    if config.get("backend"):
        config["backend"]["env"] = {}
        config["backend"]["secret_env"] = {}
    config["server"]["certificate_file"] = None
    config["server"]["private_key_file"] = None
    save_json(stage / "config.json", document)
    # Removing an OAuth file must also remove its authority-bearing reference,
    # including a credential file located outside this installation.
    for root in native_roots:
        path = stage / root / "runtime.json"
        if path.exists():
            native = load_json(path)
            provider = native.get("provider", {})
            if provider.get("auth") == "chatgpt" or provider.get("chatgpt_credentials_file"):
                provider["auth"] = "auto"
                provider["chatgpt_credentials_file"] = None
                if not provider.get("api_key_env"):
                    provider["api_key_env"] = "AGENT_MODEL_API_KEY"
                save_json(path, native)
    with contextlib.closing(sqlite3.connect(stage / "state.sqlite3")) as db:
        clear_auth(db)
        db.commit()
        db.execute("VACUUM")


def sha_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def export_checkpoint(home, archive, secret_policy, binaries=False):
    home, archive = Path(home).absolute(), Path(archive).absolute()
    no_link(home)
    supplied_home = str(home)
    home = home.resolve(strict=True)
    if archive.exists() or archive.is_symlink():
        fail("Checkpoint output already exists; choose a new path")
    if archive.resolve().is_relative_to(home):
        fail("Store the checkpoint outside the installation being captured")
    if secret_policy not in ("include", "exclude"):
        fail("Choose an explicit managed-credential policy: include or exclude")
    with contextlib.ExitStack() as stack:
        stack.enter_context(lock_file(home / ".installation.lock", "An installation or update is in progress"))
        stack.enter_context(lock_file(home / "service.lock", "The agent service is running. Stop it before checkpointing"))
        document = instance_document(home)
        roots, credential_paths = native_layout(home, document)
        for root in roots:
            locks = home / root / "locks"
            if locks.exists():
                no_link(locks)
                for path in sorted(locks.iterdir()):
                    stack.enter_context(lock_file(path, "A native session is running. Stop all attached runtime processes before checkpointing"))
        files, directories = inventory(home, roots, credential_paths, secret_policy == "include", binaries)
        if "state.sqlite3" not in files:
            fail("Installation state.sqlite3 is missing")
        databases = [name for name in files if sqlite_file(home / name)]
        # Hold a reserved writer lock across every SQLite snapshot and file copy.
        # The service lock blocks normal starts; these locks also reject ad-hoc writers.
        for name in sorted(databases):
            db = stack.enter_context(contextlib.closing(sqlite3.connect(home / name, timeout=0, isolation_level=None)))
            try:
                db.execute("BEGIN IMMEDIATE")
            except sqlite3.OperationalError as error:
                raise ValueError("A database writer is active; stop it before checkpointing") from error
            stack.callback(db.rollback)
        scratch = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="agent-checkpoint-")))
        private_path(scratch, directory=True)
        stage = scratch / "payload"
        stage.mkdir(mode=0o700)
        db_sidecars = {name + suffix for name in databases for suffix in ("-wal", "-shm", "-journal")}
        for directory in directories:
            (stage / directory).mkdir(parents=True, exist_ok=True, mode=0o700)
        for name in files:
            if name in db_sidecars:
                continue
            source, target = home / name, stage / name
            target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            if name in databases:
                uri = source.as_uri() + "?mode=ro"
                with contextlib.closing(sqlite3.connect(uri, uri=True, timeout=1)) as src, contextlib.closing(sqlite3.connect(target)) as dst:
                    src.backup(dst)
                    if dst.execute("PRAGMA quick_check").fetchone()[0] != "ok":
                        fail("SQLite snapshot integrity check failed")
            else:
                with source.open("rb") as src, target.open("xb") as dst:
                    shutil.copyfileobj(src, dst, 1024 * 1024)
            target.chmod(0o700 if files[name][4] & 0o111 else 0o600)
        # SQLite lock acquisition may create/update WAL support files. They are not payload.
        after_files, after_directories = inventory(home, roots, credential_paths, secret_policy == "include", binaries)
        before_stable = {k: v for k, v in files.items() if k not in databases and k not in db_sidecars}
        after_stable = {k: v for k, v in after_files.items() if k not in databases and k not in db_sidecars}
        if before_stable != after_stable or directories != after_directories:
            fail("Installation files changed during capture; stop external writers and retry")
        if secret_policy == "exclude":
            sanitize_staged_credentials(stage, roots)
        entries = {}
        for path in sorted(stage.rglob("*")):
            if path.is_file():
                name = path.relative_to(stage).as_posix()
                entries[name] = {"sha256": sha_file(path), "size": path.stat().st_size,
                                 "mode": 0o700 if path.stat().st_mode & 0o111 else 0o600,
                                 "sqlite": name in databases}
        manifest = {
            "schema": SCHEMA, "checkpoint_id": str(uuid.uuid4()),
            "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "instance_id": document["config"]["instance_id"], "name": document["config"]["name"],
            "source_home": str(home), "source_platform": sys.platform, "source_architecture": platform.machine(),
            "source_home_aliases": [supplied_home] if supplied_home != str(home) else [],
            "managed_credentials": secret_policy, "binaries_included": binaries,
            "credential_paths": credential_paths, "native_roots": roots,
            "files": entries, "directories": directories,
            "scope": "installation_directory_only", "service_registration_included": False,
            "excluded": ["runtime endpoints, locks, logs, service registration, prior backup/update copies",
                         "managed binaries" if not binaries else "", "managed credentials" if secret_policy == "exclude" else ""],
        }
        archive.parent.mkdir(parents=True, exist_ok=True)
        temporary = archive.with_name("." + archive.name + "." + uuid.uuid4().hex + ".tmp")
        try:
            fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "wb") as out:
                private_path(temporary)
                with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as bundle:
                    bundle.writestr("manifest.json", json.dumps(manifest, indent=2))
                    for name in entries:
                        bundle.write(stage / name, "payload/" + name)
            # A hard link publishes only if the output did not appear during capture.
            os.link(temporary, archive)
        finally:
            temporary.unlink(missing_ok=True)
    return {"schema": SCHEMA, "operation": "export", "archive": str(archive),
            "archive_sha256": sha_file(archive), "instance_id": manifest["instance_id"],
            "files": len(entries), "managed_credentials": secret_policy, "binaries_included": binaries,
            "scope": manifest["scope"], "service_started": False}


@contextlib.contextmanager
def verified_archive(archive, expected_sha256=None):
    archive = Path(archive)
    no_link(archive)
    descriptor = os.open(archive, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(descriptor, "rb") as archive_stream:
        digest = hashlib.sha256()
        for chunk in iter(lambda: archive_stream.read(1024 * 1024), b""):
            digest.update(chunk)
        archive_digest = digest.hexdigest()
        if expected_sha256 and (not re.fullmatch(r"[0-9a-f]{64}", expected_sha256) or archive_digest != expected_sha256):
            fail("Checkpoint archive SHA-256 does not match")
        archive_stream.seek(0)
        with zipfile.ZipFile(archive_stream) as bundle:
            bundle.verified_sha256 = archive_digest
            infos = bundle.infolist()
            if len(infos) > MAX_FILES + 1:
                fail("Checkpoint has too many entries")
            names, folded = {}, set()
            for info in infos:
                safe_name(info.filename)
                mode = (info.external_attr >> 16) & 0xFFFF
                if info.is_dir() or stat.S_IFMT(mode) not in (0, stat.S_IFREG) or info.flag_bits & 1:
                    fail("Checkpoint contains a link, special file, directory entry or encrypted entry")
                key = archive_key(info.filename)
                if info.filename in names or key in folded:
                    fail("Checkpoint contains duplicate or colliding paths")
                names[info.filename] = info
                folded.add(key)
            if "manifest.json" not in names or names["manifest.json"].file_size > MAX_MANIFEST:
                fail("Checkpoint manifest is missing or too large")
            manifest = json.loads(bundle.read("manifest.json"))
            if not isinstance(manifest, dict) or manifest.get("schema") != SCHEMA or not isinstance(manifest.get("files"), dict):
                fail("Unsupported checkpoint manifest")
            uuid.UUID(manifest.get("instance_id", ""))
            uuid.UUID(manifest.get("checkpoint_id", ""))
            for field in ("name", "source_platform", "source_architecture"):
                if not isinstance(manifest.get(field), str) or not manifest[field]:
                    fail("Checkpoint identity or platform metadata is missing")
            if not isinstance(manifest.get("binaries_included"), bool):
                fail("Invalid binary policy in checkpoint")
            if manifest.get("managed_credentials") not in ("include", "exclude"):
                fail("Invalid credential policy in checkpoint")
            if not isinstance(manifest.get("source_home"), str) or not (PureWindowsPath(manifest["source_home"]).is_absolute() or PurePosixPath(manifest["source_home"]).is_absolute()):
                fail("Checkpoint source home must be absolute")
            aliases = manifest.get("source_home_aliases", [])
            if not isinstance(aliases, list) or len(aliases) > 16 or any(not isinstance(value, str) or not (PureWindowsPath(value).is_absolute() or PurePosixPath(value).is_absolute()) for value in aliases):
                fail("Invalid checkpoint source-home aliases")
            files = manifest["files"]
            if set(names) != {"manifest.json", *("payload/" + n for n in files)}:
                fail("Archive payload differs from its manifest")
            directories = manifest.get("directories", [])
            for field in ("directories", "credential_paths", "native_roots"):
                if not isinstance(manifest.get(field), list) or len(manifest[field]) > MAX_FILES:
                    fail("Invalid checkpoint path list")
                for name in manifest[field]:
                    safe_name(name)
            all_paths = list(files) + directories
            if len(set(map(archive_key, all_paths))) != len(all_paths):
                fail("Checkpoint files and directories collide")
            total = 0
            for name, entry in files.items():
                safe_name(name)
                if not isinstance(entry, dict) or not isinstance(entry.get("size"), int) or entry["size"] < 0 or entry.get("mode") not in (0o600, 0o700) or not re.fullmatch(r"[0-9a-f]{64}", str(entry.get("sha256", ""))):
                    fail("Invalid checkpoint file metadata")
                if any(parent.as_posix() in files for parent in PurePosixPath(name).parents if parent.as_posix() != "."):
                    fail("Checkpoint has a file where a parent directory belongs")
                if names["payload/" + name].file_size != entry["size"]:
                    fail("Checkpoint file size differs from its manifest")
                total += entry["size"]
            if total > MAX_BYTES or len(all_paths) > MAX_FILES or "config.json" not in files or "state.sqlite3" not in files:
                fail("Checkpoint exceeds limits or lacks core instance files")
            # Verify each stream without zipfile.extract; no archive-controlled filesystem writes.
            for name, entry in files.items():
                digest, count = hashlib.sha256(), 0
                with bundle.open("payload/" + name) as content:
                    for chunk in iter(lambda: content.read(1024 * 1024), b""):
                        count += len(chunk)
                        if count > entry["size"]:
                            fail("Checkpoint entry exceeds its declared size")
                        digest.update(chunk)
                if count != entry["size"] or digest.hexdigest() != entry["sha256"]:
                    fail("Checkpoint content hash does not match")
            yield bundle, manifest


def relocate_path(value, old_home, new_home, dependencies, field):
    if not isinstance(value, str) or not value or "${AGENT_HOME}" in value:
        return value
    homes = [old_home] if isinstance(old_home, str) else old_home
    absolute = False
    for home in homes:
        kind = PureWindowsPath if PureWindowsPath(home).drive else PurePosixPath
        path, base = kind(value), kind(home)
        if not path.is_absolute():
            continue
        absolute = True
        try:
            relative = path.relative_to(base)
        except ValueError:
            continue
        if ".." in relative.parts:
            fail("A configured path traverses outside its installation")
        return str(Path(new_home).joinpath(*relative.parts))
    if absolute:
        dependencies.append({"field": field, "path": value, "action": "Reconnect or install this external dependency before enabling the agent"})
    return value


def source_homes(manifest):
    return [manifest["source_home"], *manifest.get("source_home_aliases", [])]


def relocate_config(document, old_home, new_home, dependencies):
    config = document["config"]
    backend = config.get("backend") or {}
    for field in ("command", "working_directory"):
        if field in backend:
            backend[field] = relocate_path(backend[field], old_home, new_home, dependencies, "backend." + field)
    backend["allowed_working_roots"] = [relocate_path(v, old_home, new_home, dependencies, "backend.allowed_working_roots") for v in backend.get("allowed_working_roots", [])]
    args = backend.get("args", [])
    for i, value in enumerate(args):
        if i and args[i - 1] in ("--home", "--cwd", "--workspace", "--working-directory"):
            args[i] = relocate_path(value, old_home, new_home, dependencies, "backend.args." + args[i - 1])
        elif any(value.startswith(flag + "=") for flag in ("--home", "--cwd", "--workspace", "--working-directory")):
            key, content = value.split("=", 1)
            args[i] = key + "=" + relocate_path(content, old_home, new_home, dependencies, "backend.args." + key)
    for key in ("HOME", "CODEX_HOME", "AGENT_HOME"):
        if key in backend.get("env", {}):
            backend["env"][key] = relocate_path(backend["env"][key], old_home, new_home, dependencies, "backend.env." + key)
    for key in ("certificate_file", "private_key_file"):
        config["server"][key] = relocate_path(config["server"].get(key), old_home, new_home, dependencies, "server." + key)


def relocate_native(config, old_home, new_home, dependencies, clone):
    config["workspace"] = relocate_path(config.get("workspace"), old_home, new_home, dependencies, "native.workspace")
    if "additional_workspaces" in config:
        config["additional_workspaces"] = [relocate_path(v, old_home, new_home, dependencies, "native.additional_workspaces") for v in config["additional_workspaces"]]
    provider = config.get("provider", {})
    for key in ("tls_ca_file", "chatgpt_credentials_file"):
        if key in provider:
            provider[key] = relocate_path(provider[key], old_home, new_home, dependencies, "native.provider." + key)
    bridge = provider.get("copilot") or {}
    for key in ("python", "script"):
        if key in bridge:
            bridge[key] = relocate_path(bridge[key], old_home, new_home, dependencies, "native.provider.copilot." + key)
    for backend in config.get("decisions", []):
        if "tls_ca_file" in backend:
            backend["tls_ca_file"] = relocate_path(backend["tls_ca_file"], old_home, new_home, dependencies, "native.decisions.tls_ca_file")
    if clone:
        for key in ("allowed_mcp_mutations", "trusted_mcp_readonly_urls"):
            config[key] = []
        config.update(mode="ask", allow_client_stdio_mcp=False, allow_client_system_prompt=False)


def update_state(stage, manifest, destination, clone, clear_credentials):
    state = stage / "state.sqlite3"
    now = int(time.time() * 1000)
    with contextlib.closing(sqlite3.connect(state)) as db:
        tables = db_tables(db)
        if clear_credentials:
            clear_auth(db)
        else:
            for table in ("browser_sessions", "login_attempts"):
                if table in tables:
                    db.execute(f"DELETE FROM {table}")
        if "records" in tables:
            if clone:
                db.executemany("DELETE FROM records WHERE collection=?", [(name,) for name in CONTEXT_RECORDS])
            for collection, key, raw in db.execute("SELECT collection,id,value FROM records WHERE collection IN ('scheduler','organization','sessions')").fetchall():
                value = json.loads(raw)
                if collection == "scheduler":
                    value.update(enabled=False, paused=True, approved_backend_sha256=None)
                    value["revision"] = value.get("revision", 0) + 1
                elif clone and collection == "organization":
                    organization = value.get("organization", value)
                    for assignment in organization.get("assignments", []):
                        assignment["active"] = False
                    for relationship in organization.get("relationships", []):
                        relationship.update(active=False, send=False, receive=False, allow_details=False)
                    value["revision"] = value.get("revision", 0) + 1
                elif collection == "sessions" and "cwd" in value:
                    value["cwd"] = relocate_path(value["cwd"], source_homes(manifest), destination, [], "session.cwd")
                else:
                    continue
                db.execute("UPDATE records SET value=?,updated_at=? WHERE collection=? AND id=?", (json.dumps(value), now, collection, key))
        if clear_credentials or not (stage / "secrets/control-token").is_file():
            token = secrets.token_hex(32)
            if "tokens" not in tables:
                fail("Instance token table is missing")
            db.execute("DELETE FROM tokens WHERE name='Local installation control'")
            db.execute("INSERT INTO tokens(hash,id,name,scopes,expires_at,created_at) VALUES(?,?,?,?,?,?)",
                       (hashlib.sha256(token.encode()).hexdigest(), str(uuid.uuid4()), "Local installation control", '["admin"]', now + 366 * 86400 * 1000, now))
            (stage / "secrets").mkdir(exist_ok=True, mode=0o700)
            (stage / "secrets/control-token").write_text(token, encoding="utf-8")
            (stage / "secrets/control-token").chmod(0o600)
        db.commit()
        db.execute("VACUUM")
    for root in manifest["native_roots"]:
        history = stage / root / "history.sqlite3"
        if not history.exists():
            continue
        with contextlib.closing(sqlite3.connect(history)) as db:
            tables = db_tables(db)
            if clone and "history_identity" in tables:
                db.execute("UPDATE history_identity SET value=? WHERE id=1", (str(uuid.uuid4()),))
            if "sessions" in tables:
                for sid, cwd in db.execute("SELECT id,cwd FROM sessions").fetchall():
                    relocated = relocate_path(cwd, source_homes(manifest), destination, [], "native.session.cwd")
                    if relocated != cwd:
                        db.execute("UPDATE sessions SET cwd=? WHERE id=?", (relocated, sid))
            db.commit()


def publish_directory(source, destination):
    """Atomically publish without replacing even an empty concurrently-created directory."""
    if os.name == "nt":
        os.rename(source, destination)  # Windows rename never replaces a directory.
        return
    library = ctypes.CDLL(None, use_errno=True)
    if sys.platform == "darwin":
        operation = getattr(library, "renamex_np", None)
        if operation is None:
            fail("Atomic no-overwrite publication is unavailable on this platform")
        operation.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
        result = operation(os.fsencode(source), os.fsencode(destination), 0x4)  # RENAME_EXCL
    elif sys.platform.startswith("linux"):
        operation = getattr(library, "renameat2", None)
        if operation is None:
            fail("Atomic no-overwrite publication requires a platform with renameat2 support")
        operation.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        result = operation(-100, os.fsencode(source), -100, os.fsencode(destination), 1)  # RENAME_NOREPLACE
    else:
        fail("This platform lacks a supported atomic no-overwrite directory operation")
    if result:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))


def restore_checkpoint(archive, destination, mode, secret_policy="exclude", name=None, expected_sha256=None):
    destination = Path(destination).absolute()
    if destination.exists() or destination.is_symlink():
        fail("Destination already exists; restore and clone never overwrite an installation")
    if mode not in ("restore", "clone") or secret_policy not in ("include", "exclude"):
        fail("Invalid restore mode or credential policy")
    clone = mode == "clone"
    if clone and (not name or not name.strip() or len(name.encode()) > 200):
        fail("A clone needs its own --name (1–200 bytes)")
    if clone and secret_policy != "exclude":
        fail("Clones never inherit credentials")
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination = destination.parent.resolve() / destination.name
    with verified_archive(archive, expected_sha256) as (bundle, manifest):
        if secret_policy == "include" and manifest["managed_credentials"] != "include":
            fail("This checkpoint excluded credentials; they cannot be restored")
        stage = Path(tempfile.mkdtemp(prefix=".checkpoint-restore-", dir=destination.parent))
        try:
            private_path(stage, directory=True)
            for directory in manifest["directories"]:
                (stage / directory).mkdir(parents=True, exist_ok=True, mode=0o700)
            for path, entry in manifest["files"].items():
                if secret_policy == "exclude" and any(under(path, root) for root in manifest["credential_paths"]):
                    continue
                target = stage / path
                target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                with bundle.open("payload/" + path) as source, target.open("xb") as out:
                    shutil.copyfileobj(source, out, 1024 * 1024)
                target.chmod(entry["mode"])
                if entry.get("sqlite"):
                    with contextlib.closing(sqlite3.connect(target)) as db:
                        if db.execute("PRAGMA quick_check").fetchone()[0] != "ok":
                            fail("Restored SQLite integrity check failed")
            document = instance_document(stage)
            if document["config"]["instance_id"] != manifest["instance_id"]:
                fail("Checkpoint identity and configuration differ")
            if secret_policy == "exclude":
                sanitize_staged_credentials(stage, manifest["native_roots"])
                document = instance_document(stage)
            config = document["config"]
            previous_owner = config["owner"]
            config.update(enabled=False, owner=current_owner())
            config["server"].update(bind="127.0.0.1", port=0, welcome_port=0, public_url=None, embed_origins=[])
            if clone:
                config.update(instance_id=str(uuid.uuid4()), name=name.strip())
                save_json(stage / "context-providers.json", {"providers": []})
            dependencies = []
            relocate_config(document, source_homes(manifest), destination, dependencies)
            document["revision"] += 1
            save_json(stage / "config.json", document)
            for root in manifest["native_roots"]:
                path = stage / root / "runtime.json"
                if path.exists():
                    native = load_json(path)
                    relocate_native(native, source_homes(manifest), destination, dependencies, clone)
                    if clone:
                        native["name"] = name.strip()
                    save_json(path, native)
            update_state(stage, manifest, destination, clone, secret_policy == "exclude")
            actions = ["Install or verify the instance and runtime executables for this operating system",
                       "Review external paths and provider credentials",
                       "Configure HTTPS address, ports and experience connections",
                       "Register startup explicitly if desired; this command registers nothing",
                       "Review pending work, enable the agent, and separately reapprove background execution"]
            if not clone:
                actions.insert(0, "Keep the original installation stopped before activating this restored identity")
            else:
                actions.insert(0, "Reconnect conversation providers and grant this new individual its own access")
            result = {"schema": SCHEMA, "operation": mode, "home": str(destination),
                      "checkpoint_id": manifest["checkpoint_id"], "archive_sha256": bundle.verified_sha256, "instance_id": config["instance_id"],
                      "source_instance_id": manifest["instance_id"], "name": config["name"],
                      "owner": config["owner"], "previous_owner": previous_owner,
                      "managed_credentials": secret_policy, "binaries_included": manifest["binaries_included"],
                      "source_platform": manifest["source_platform"], "source_architecture": manifest["source_architecture"],
                      "service_started": False, "service_registered": False, "agent_enabled": False,
                      "scheduler_paused": True, "external_dependencies": dependencies, "next_actions": actions}
            save_json(stage / "checkpoint-result.json", result)
            publish_directory(stage, destination)
            return result
        finally:
            if stage.exists():
                shutil.rmtree(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    capture = commands.add_parser("export", help="Stop the agent first; capture one installation")
    capture.add_argument("--home", type=Path, required=True)
    capture.add_argument("--output", type=Path, required=True)
    capture.add_argument("--secrets", choices=("include", "exclude"), required=True, help="Explicit policy for managed credentials; workspace/history content is preserved")
    capture.add_argument("--include-binaries", action="store_true")
    for command in ("restore", "clone", "inspect"):
        sub = commands.add_parser(command)
        sub.add_argument("--archive", type=Path, required=True)
        sub.add_argument("--sha256", help="Verify the archive hash supplied out of band")
        if command != "inspect":
            sub.add_argument("--home", type=Path, required=True, help="New, nonexistent destination directory")
        if command == "restore":
            sub.add_argument("--secrets", choices=("include", "exclude"), required=True)
        if command == "clone":
            sub.add_argument("--name", required=True)
    args = parser.parse_args()
    try:
        if args.command == "export":
            result = export_checkpoint(args.home, args.output, args.secrets, args.include_binaries)
        elif args.command == "inspect":
            with verified_archive(args.archive, args.sha256) as (bundle, manifest):
                result = {"verified": True, "archive_sha256": bundle.verified_sha256, "manifest": manifest}
        else:
            result = restore_checkpoint(args.archive, args.home, args.command,
                                        getattr(args, "secrets", "exclude"), getattr(args, "name", None), args.sha256)
        print(json.dumps(result, indent=2))
    except (ValueError, OSError, sqlite3.Error, zipfile.BadZipFile, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(json.dumps({"error": str(error), "operation": args.command}), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
