#!/usr/bin/env python3
"""Provision one private pinned Codex runtime. Embedded in the Rust executable.

No npm lifecycle scripts, shell downloads, global runtime state, config updates,
or changes to an existing provider account. The only stdout is a JSON result.
The compiled lock is supplied on stdin; the CLI cannot substitute download URLs.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.parse
import urllib.request
import zipfile

if os.name == "nt":
    import msvcrt
else:
    import fcntl
    import pwd


def private_dir(path):
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    if path.is_symlink() or not path.is_dir():
        raise ValueError("Runtime directories must be private ordinary folders")
    path.chmod(0o700)


def write_private(path, content):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as temporary:
        temporary.write(content)
        temporary.flush()
        os.fsync(temporary.fileno())
        temporary_name = temporary.name
    os.replace(temporary_name, path)
    path.chmod(0o600)


def download(item, destination):
    url = urllib.parse.urlsplit(item["url"])
    if url.scheme != "https" or url.netloc not in ("nodejs.org", "registry.npmjs.org"):
        raise ValueError("Unrecognized runtime publisher")
    algorithm, expected = item["integrity"].split("-", 1)
    if algorithm not in ("sha256", "sha512"):
        raise ValueError("Unsupported runtime checksum")
    digest = hashlib.new(algorithm)
    size = 0
    with urllib.request.urlopen(item["url"], timeout=45) as response, destination.open("xb") as target:
        final = urllib.parse.urlsplit(response.url)
        if final.scheme != "https" or final.netloc != url.netloc:
            raise ValueError("Runtime download redirected away from its publisher")
        while chunk := response.read(1024 * 1024):
            size += len(chunk)
            if size > 512 * 1024 * 1024:
                raise ValueError("Runtime archive exceeds its size limit")
            digest.update(chunk)
            target.write(chunk)
    actual = digest.hexdigest() if algorithm == "sha256" else base64.b64encode(digest.digest()).decode()
    if actual != expected:
        raise ValueError("Runtime archive did not match the pinned publisher checksum")


def extract_files(archive, destination, select):
    """Extract selected regular files only, never links or archive permissions."""
    written = set()
    total = 0
    with tarfile.open(archive, "r:gz") as source:
        for member in source:
            name = PurePosixPath(member.name)
            if name.is_absolute() or ".." in name.parts or "\\" in member.name or ":" in member.name:
                raise ValueError("Unsafe archive member")
            relative = select(member.name)
            if relative is None or member.isdir():
                continue
            relative = PurePosixPath(relative)
            if not member.isfile() or relative.is_absolute() or ".." in relative.parts or relative in written:
                raise ValueError("Runtime archive contains an unsafe or duplicate file")
            written.add(relative)
            total += member.size
            if total > 1024 * 1024 * 1024 or member.size > 512 * 1024 * 1024:
                raise ValueError("Runtime archive exceeds its expanded size limit")
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            stream = source.extractfile(member)
            if stream is None:
                raise ValueError("Unreadable archive member")
            with target.open("xb") as output:
                shutil.copyfileobj(stream, output)
            target.chmod(0o700 if member.mode & 0o111 else 0o600)
    return written


def extract_zip(archive, destination, select):
    """Node's Windows archive, with the same regular-file-only extraction policy."""
    written = set()
    total = 0
    with zipfile.ZipFile(archive) as source:
        for member in source.infolist():
            # Windows ZipInfo normalizes backslashes in filename; inspect the raw
            # archive name too so ambiguous path separators never pass the check.
            raw_name = member.orig_filename
            name = PurePosixPath(raw_name)
            if name.is_absolute() or ".." in name.parts or "\\" in raw_name or ":" in raw_name:
                raise ValueError("Unsafe archive member")
            relative = select(member.filename)
            if relative is None or member.is_dir():
                continue
            relative = PurePosixPath(relative)
            mode = member.external_attr >> 16
            if stat.S_ISLNK(mode) or relative.is_absolute() or ".." in relative.parts or relative in written:
                raise ValueError("Runtime archive contains an unsafe or duplicate file")
            total += member.file_size
            if total > 1024 * 1024 * 1024 or member.file_size > 512 * 1024 * 1024:
                raise ValueError("Runtime archive exceeds its expanded size limit")
            written.add(relative)
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            with source.open(member) as stream, target.open("xb") as output:
                shutil.copyfileobj(stream, output)
    return written


def credential_present(value):
    if not isinstance(value, dict):
        return False
    key = value.get("OPENAI_API_KEY")
    tokens = value.get("tokens")
    return (isinstance(key, str) and bool(key.strip())) or (
        isinstance(tokens, dict) and isinstance(tokens.get("access_token"), str)
        and bool(tokens["access_token"].strip())
    )


def copy_owner_account(provider, reuse_owner_account, owner_home=None):
    """Copy only a regular, owned auth cache once; never share a mutable file."""
    target = provider / "auth.json"
    # Native Windows accounts are connected explicitly; do not guess another
    # profile/keychain credential location or silently reuse domain credentials.
    if os.name == "nt" or target.exists() or target.is_symlink() or not reuse_owner_account:
        return False
    source = (owner_home or Path(pwd.getpwuid(os.getuid()).pw_dir)) / ".codex" / "auth.json"
    try:
        fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW)
    except (FileNotFoundError, OSError):
        return False
    with os.fdopen(fd, "rb") as source_file:
        info = os.fstat(source_file.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_size > 1024 * 1024:
            return False
        data = source_file.read(1024 * 1024 + 1)
    try:
        if not credential_present(json.loads(data)):
            return False
    except (ValueError, UnicodeDecodeError):
        return False
    # O_EXCL protects an account connected concurrently by this instance.
    try:
        fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    except FileExistsError:
        return False
    with os.fdopen(fd, "wb") as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    write_private(provider / "account-source.json", json.dumps({
        "source": "owner_account", "at": int(time.time() * 1000),
        "description": "Copied once from the installing OS owner's Codex account; independent cache thereafter",
    }).encode())
    return True


def runtime_paths(folder):
    suffix = ".exe" if os.name == "nt" else ""
    return [folder / ("node/bin/node" + suffix), folder / ("codex/bin/codex" + suffix), folder / ("codex/bin/codex-code-mode-host" + suffix), folder / "acp/dist/index.js"]


def platform_key():
    machine = {"aarch64": "arm64", "arm64": "arm64", "x86_64": "x64", "amd64": "x64"}.get(platform.machine().lower())
    system = {"Linux": "linux", "Darwin": "darwin", "Windows": "win32"}.get(platform.system())
    if machine is None or system is None:
        raise ValueError("The default runtime supports x86_64 and ARM64 Linux, macOS and Windows")
    return system + "-" + machine


def lock_installation(mutex):
    if os.name == "nt":
        mutex.seek(0)
        mutex.write(b"0")
        mutex.flush()
        mutex.seek(0)
        msvcrt.locking(mutex.fileno(), msvcrt.LK_NBLCK, 1)
    else:
        fcntl.flock(mutex, fcntl.LOCK_EX | fcntl.LOCK_NB)



def provision(home, lock, reuse_owner_account):
    if not (home / "config.json").is_file():
        raise ValueError("Initialize this instance before installing its runtime")
    backend = home / "backend"
    private_dir(backend)
    lock_fd = os.open(backend / "provision.lock", os.O_CREAT | os.O_RDWR | getattr(os, "O_NOFOLLOW", 0), 0o600)
    with os.fdopen(lock_fd, "r+b") as mutex:
        try:
            lock_installation(mutex)
        except OSError as error:
            raise ValueError("An installation is already in progress for this agent") from error
        key = platform_key()
        selected = lock["platforms"][key]
        runtime = backend / "runtime" / lock["bundle"]
        private_dir(runtime.parent)
        marker = runtime / "runtime.json"
        complete = False
        if marker.is_file():
            manifest = json.loads(marker.read_text())
            complete = manifest.get("bundle") == lock["bundle"] and all(x.is_file() for x in runtime_paths(runtime))
            # Detect accidental modification before deciding an install is a no-op.
            complete = complete and all(hashlib.sha256(path.read_bytes()).hexdigest() == manifest.get("sha256", {}).get(str(path.relative_to(runtime))) for path in runtime_paths(runtime))
        if not complete:
            if runtime.exists():
                raise ValueError("The existing runtime is incomplete or modified; preserve it and reinstall into a new instance, or restore the pinned files")
            with tempfile.TemporaryDirectory(prefix=".provision-", dir=runtime.parent) as scratch:
                scratch = Path(scratch)
                build = scratch / "build"
                private_dir(build)
                for name, package in (("node", selected["node"]), ("codex", selected["codex"]), ("acp", lock["acp"])):
                    archive = scratch / (name + ".tgz")
                    download(package, archive)
                    if name == "node":
                        prefix = "node-v" + lock["node_version"] + "-" + key.replace("win32", "win") + "/"
                        if os.name == "nt":
                            extract_zip(archive, build / name, lambda x: "bin/node.exe" if x == prefix + "node.exe" else ("LICENSE" if x == prefix + "LICENSE" else None))
                        else:
                            extract_files(archive, build / name, lambda x: x[len(prefix):] if x in (prefix + "bin/node", prefix + "LICENSE") else None)
                    elif name == "codex":
                        prefix = "package/vendor/" + selected["target"] + "/"
                        extract_files(archive, build / name, lambda x: x[len(prefix):] if x.startswith(prefix) else None)
                    else:
                        extract_files(archive, build / name, lambda x: x[8:] if x in ("package/dist/index.js", "package/package.json", "package/LICENSE", "package/README.md") else None)
                if not all(x.is_file() for x in runtime_paths(build)):
                    raise ValueError("Publisher runtime archive is missing required files")
                # Version checks execute verified publishers' code with no credentials.
                clean_env = {"PATH": os.defpath, "HOME": str(scratch)}
                for key_name in ("SYSTEMROOT", "WINDIR", "TEMP", "TMP"):
                    if key_name in os.environ:
                        clean_env[key_name] = os.environ[key_name]
                for executable, expected in zip(runtime_paths(build)[:2], ("v" + lock["node_version"], "codex-cli " + lock["codex_version"])):
                    result = subprocess.run([str(executable), "--version"], env=clean_env, capture_output=True, text=True, timeout=15, check=True)
                    if result.stdout.strip() != expected:
                        raise ValueError("Runtime version does not match the pinned release")
                manifest = {key: lock[key] for key in ("schema_version", "bundle", "codex_version", "acp_version", "node_version")}
                manifest.update({"installed_at": int(time.time() * 1000), "platform": key,
                    "sha256": {str(path.relative_to(build)): hashlib.sha256(path.read_bytes()).hexdigest() for path in runtime_paths(build)}})
                write_private(build / "runtime.json", json.dumps(manifest, indent=2).encode())
                os.rename(build, runtime)
        provider = backend / "provider"
        private_dir(provider)
        for folder in (home / "workspace", backend / "config", backend / "data", backend / "cache"):
            private_dir(folder)
        config_path = provider / "config.toml"
        if not config_path.exists():
            write_private(config_path, b'cli_auth_credentials_store = "file"\ncheck_for_update_on_startup = false\nallow_login_shell = false\nweb_search = "disabled"\n[features]\nshell_snapshot = false\n')
        copied = copy_owner_account(provider, reuse_owner_account)
        write_private(backend / "runtime.json", json.dumps(manifest, indent=2).encode())
        prefix = "${AGENT_HOME}/backend/runtime/" + lock["bundle"]
        suffix = ".exe" if os.name == "nt" else ""
        configuration = {
            "command": prefix + "/node/bin/node" + suffix, "args": [prefix + "/acp/dist/index.js"],
            "env": {
                "CODEX_PATH": prefix + "/codex/bin/codex" + suffix,
                "CODEX_HOME": "${AGENT_HOME}/backend/provider",
                "NO_BROWSER": "1", "INITIAL_AGENT_MODE": "workspace-write",
                "PATH": os.pathsep.join([prefix + "/node/bin", prefix + "/codex/codex-path", os.environ.get("PATH", os.defpath)]),
                "CODEX_CONFIG": json.dumps({"web_search": "disabled", "allow_login_shell": False,
                    "check_for_update_on_startup": False, "features": {"shell_snapshot": False}}),
            },
            "secret_env": {}, "working_directory": "${AGENT_HOME}/workspace",
            "allowed_working_roots": ["${AGENT_HOME}/workspace"],
        }
        return {"backend": configuration, "installed": not complete, "account_copied": copied}


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser()
    parser.add_argument("--home", required=True, type=Path)
    parser.add_argument("--no-reuse-account", action="store_true")
    options = parser.parse_args()
    try:
        result = provision(options.home.resolve(), json.load(sys.stdin), not options.no_reuse_account)
    except Exception as error:
        # Auth caches are never included in exceptions; network errors include only pinned publisher URLs.
        print("Default runtime installation failed: " + str(error), file=sys.stderr)
        return 1
    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
