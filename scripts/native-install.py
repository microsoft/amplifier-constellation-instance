#!/usr/bin/env python3
"""Install and switch one native instance. No fleet or adapter state is consulted.

The only stdout is the JSON result. Build/prerequisite discovery belongs to the
platform entry scripts; this helper takes an explicit verified local executable.
"""
import argparse
import base64
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid
import webbrowser


def run(argv, **kwargs):
    return subprocess.run([str(x) for x in argv], check=True, capture_output=True, text=True, **kwargs).stdout.strip()


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent, delete=False) as target:
        json.dump(value, target, indent=2)
        target.write("\n")
        target.flush()
        os.fsync(target.fileno())
        name = target.name
    os.replace(name, path)


def binary_name():
    return "agent-instance.exe" if os.name == "nt" else "agent-instance"


def validate_home(home):
    if any(ord(c) < 32 or c == '"' for c in str(home)):
        raise ValueError("Instance paths cannot contain control characters or double quotes")
    if home.is_symlink():
        raise ValueError("Use an ordinary instance directory, not a symbolic link")


def release_info(binary):
    if not binary.is_file() or binary.is_symlink():
        raise ValueError("Supply an ordinary native executable with --binary")
    version = run([binary, "--version"], timeout=15)
    match = re.fullmatch(r"agent-instance (\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)", version)
    if not match:
        raise ValueError("Executable did not identify itself as agent-instance")
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    return {"id": match[1] + "-" + digest[:12], "version": match[1], "sha256": digest}


def stage(home, binary):
    info = release_info(binary)
    folder = home / "releases" / info["id"]
    target = folder / binary_name()
    if folder.exists():
        if not target.is_file() or hashlib.sha256(target.read_bytes()).hexdigest() != info["sha256"]:
            raise ValueError("Existing release files differ; they were preserved")
    else:
        folder.mkdir(parents=True, mode=0o700)
        shutil.copyfile(binary, target)
        target.chmod(0o700)
        save(folder / "release.json", info)
    for name in ("LICENSE", "THIRD-PARTY-NOTICES.txt"):
        source = binary.parent / name
        if not source.is_file():
            source = Path(__file__).resolve().parent.parent / name
        destination = folder / name
        if not destination.exists():
            shutil.copyfile(source, destination)
            destination.chmod(0o600)
    return info


def activate(home, info):
    source = home / "releases" / info["id"] / binary_name()
    if hashlib.sha256(source.read_bytes()).hexdigest() != info["sha256"]:
        raise ValueError("Selected release checksum does not match its manifest")
    destination = home / "bin" / binary_name()
    destination.parent.mkdir(mode=0o700, exist_ok=True)
    temporary = destination.with_name(binary_name() + "." + uuid.uuid4().hex + ".tmp")
    try:
        shutil.copyfile(source, temporary)
        temporary.chmod(0o700)
        os.replace(temporary, destination)
    finally:
        temporary.unlink(missing_ok=True)


def powershell(code, *args):
    # Encode code and data independently. Paths and names never become PS syntax.
    data = base64.b64encode(json.dumps(args).encode()).decode()
    script = "$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; $d=ConvertFrom-Json ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('" + data + "'))); " + code
    encoded = base64.b64encode(script.encode("utf-16-le")).decode()
    return run(["powershell.exe", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded], timeout=90)


def make_service(home, config):
    identifier = str(uuid.UUID(config["instance_id"]))
    executable = home / "bin" / binary_name()
    if sys.platform == "linux":
        name = "agent-instance-" + identifier + ".service"
        destination = Path.home() / ".config/systemd/user" / name
        if destination.exists():
            raise ValueError("A startup job already exists for this identity; it was preserved")
        def quote(value, command=False):
            value = str(value).replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%')
            return '"' + (value.replace('$', '$$') if command else value) + '"'
        destination.parent.mkdir(parents=True, exist_ok=True)
        with destination.open('x') as output:
            output.write('[Unit]\nDescription=One Constellation agent\nAfter=network-online.target\n'
                         '[Service]\nExecStart=' + ' '.join(quote(x, command=True) for x in [executable, 'serve', '--home', home]) +
                         '\nWorkingDirectory=' + quote(home) + '\nRestart=on-failure\nRestartSec=5\nUMask=0077\n'
                         '[Install]\nWantedBy=default.target\n')
        destination.chmod(0o600)
        run(['systemctl', '--user', 'daemon-reload'], timeout=30)
        run(['systemctl', '--user', 'enable', name], timeout=30)
        return {'kind': 'systemd-user', 'name': name}
    if sys.platform == "darwin":
        label = "io.agent-instance." + identifier
        destination = Path.home() / "Library/LaunchAgents" / (label + ".plist")
        if destination.exists():
            raise ValueError("A startup job already exists for this identity; it was preserved")
        destination.parent.mkdir(parents=True, exist_ok=True)
        value = {"Label": label, "ProgramArguments": [str(executable), "serve", "--home", str(home)],
                 "WorkingDirectory": str(home), "RunAtLoad": True, "KeepAlive": {"SuccessfulExit": False},
                 "ThrottleInterval": 5, "Umask": 0o077,
                 "EnvironmentVariables": {"PATH": str(Path(sys.executable).parent) + ":/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"},
                 "StandardOutPath": str(home / "service.stdout.log"), "StandardErrorPath": str(home / "service.stderr.log")}
        with destination.open("xb") as out:
            plistlib.dump(value, out)
        destination.chmod(0o600)
        return {"kind": "launchd", "name": label, "file": str(destination), "domain": "gui/" + str(os.getuid())}
    if os.name == "nt":
        name = "AgentInstance-" + identifier
        # Run only as the logged-in owner. No account password is stored in a task.
        powershell(r'''
            $name,$exe,$folder,$owner=$d
            if (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) { throw 'A task with this instance identity already exists' }
            $action=New-ScheduledTaskAction -Execute $exe -Argument ('serve --home "'+$folder+'" --log-directory "'+$folder+'"') -WorkingDirectory $folder
            $trigger=New-ScheduledTaskTrigger -AtLogOn -User $owner
            $principal=New-ScheduledTaskPrincipal -UserId $owner -LogonType Interactive -RunLevel Limited
            $settings=New-ScheduledTaskSettingsSet -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
            Register-ScheduledTask -TaskName $name -Action $action -Trigger $trigger -Principal $principal -Settings $settings | Out-Null
        ''', name, str(executable), str(home), config["owner"])
        return {"kind": "scheduled-task", "name": name}
    raise ValueError("Native startup integration is available on macOS and Windows; use install.sh for Linux")


def service_command(service, action):
    if service["kind"] == "systemd-user":
        run(['systemctl', '--user', action, service['name']], timeout=30)
    elif service["kind"] == "launchd":
        if action == "start":
            run(["launchctl", "bootstrap", service["domain"], service["file"]], timeout=30)
        elif action == "stop":
            result = subprocess.run(["launchctl", "bootout", service["domain"] + "/" + service["name"]], capture_output=True, text=True)
            if result.returncode and "No such process" not in result.stderr and "Could not find service" not in result.stderr:
                raise ValueError("Cannot stop this instance's startup job: " + result.stderr.strip())
    elif service["kind"] == "scheduled-task":
        verb = {"start": "Start-ScheduledTask", "stop": "Stop-ScheduledTask"}[action]
        powershell(verb + " -TaskName $d[0]", service["name"])
    else:
        raise ValueError("Unknown startup integration")


@contextlib.contextmanager
def install_lock(home):
    path = home / ".installation.lock"
    descriptor = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    with os.fdopen(descriptor, "r+b") as lock:
        if os.name == "nt":
            import msvcrt
            lock.write(b"0"); lock.flush(); lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield


@contextlib.contextmanager
def stopped_lock(home):
    """Refuse updates until the instance lock is free, including manual servers."""
    with (home / "service.lock").open("a+b") as lock:
        deadline = time.monotonic() + 30
        while True:
            try:
                if os.name == "nt":
                    import msvcrt
                    # fs2 uses LockFileEx across the complete file. This byte overlaps it.
                    lock.seek(0)
                    msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except (BlockingIOError, OSError):
                if time.monotonic() >= deadline:
                    raise ValueError("The agent is still running. Stop this instance before changing its version.")
                time.sleep(.2)
        yield


def snapshot(home):
    folder = home / "backups" / (time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()) + "-" + uuid.uuid4().hex[:8])
    folder.mkdir(parents=True, mode=0o700)
    for name in ("config.json", "install.json"):
        shutil.copyfile(home / name, folder / name)
    if (home / "state.sqlite3").exists():
        with contextlib.closing(sqlite3.connect(home / "state.sqlite3")) as source, contextlib.closing(sqlite3.connect(folder / "state.sqlite3")) as target:
            source.backup(target)
    return str(folder)


def health(home, identity, version):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            result = json.loads(run([home / "bin" / binary_name(), "status", "--home", home, "--json"], timeout=5))
            if result.get("instance_id") == identity and result.get("version") == version:
                return result
        except (ValueError, OSError, subprocess.SubprocessError):
            pass
        time.sleep(.3)
    raise ValueError("The new service did not become healthy; inspect this instance's service logs")


def result(home, metadata, healthy=None):
    value = {"home": str(home), "instance_id": metadata["instance_id"], "owner": metadata.get("owner"), "release": metadata["current"],
             "previous": metadata.get("previous"), "service": metadata.get("service"),
             "running_verified": healthy is not None, "authentication": "windows_logon" if os.name == "nt" else "pam"}
    if healthy and (home / "endpoints.json").exists():
        value["endpoints"] = json.loads((home / "endpoints.json").read_text())
    return value


def install(args):
    home = args.home
    validate_home(home)
    if home.exists() and any(home.iterdir()):
        raise ValueError("Choose a new or empty instance folder; use update for an existing installation")
    if getattr(args, 'bundle', None) is not None and not args.skip_runtime and args.engine == 'native' and not args.runtime_binary:
        lock = json.loads((args.bundle / 'setup/native-runtime-lock.json').read_text())
        family = {'Linux': 'linux', 'Darwin': 'macos', 'Windows': 'windows'}.get(platform.system())
        architecture = {'aarch64': 'aarch64', 'arm64': 'aarch64', 'x86_64': 'x86_64', 'amd64': 'x86_64'}.get(platform.machine().lower())
        key = str(family) + '-' + str(architecture)
        if not any(a['platform'] == key for a in lock['assets']):
            raise ValueError('No published native engine is pinned for this platform; no installation was created. Configure a supported engine explicitly or use --skip-runtime for an unconfigured host.')
    info = release_info(args.binary)
    home.mkdir(parents=True, exist_ok=True, mode=0o700)
    # init establishes the native private directory policy before credentials exist.
    run([args.binary, "init", "--home", home, "--name", args.name, "--bind", args.bind, "--port", args.port], timeout=30)
    with install_lock(home):
        stage_support(args)
        config = json.loads((home / "config.json").read_text())["config"]
        info = stage(home, args.binary)
        activate(home, info)
        if not args.skip_runtime:
            runtime_args = [home / "bin" / binary_name(), "backend", "install", "--home", home, "--engine", args.engine, "--no-reuse-account"]
            if args.runtime_binary:
                runtime_args.extend(["--runtime-binary", args.runtime_binary])
            run(runtime_args, timeout=360)
        metadata = {"manager": "native-install/1", "schema_version": 1, "instance_id": config["instance_id"],
                    "owner": config["owner"], "platform": sys.platform, "architecture": platform.machine(),
                    "current": info, "previous": None, "service": None,
                    "installed_at": int(time.time() * 1000)}
        # Record a usable installation even if host service registration fails.
        save(home / "install.json", metadata)
        healthy = None
        if not args.no_service:
            metadata["service"] = make_service(home, config)
            save(home / "install.json", metadata)
            service_command(metadata["service"], "start")
            if "install-updates" in run([home / "bin" / binary_name(), "--help"]):
                run([home / "bin" / binary_name(), "--home", home, "install-updates"])
            healthy = health(home, config["instance_id"], info["version"])
        return result(home, metadata, healthy)


def switch(args):
    home = args.home
    validate_home(home)
    with install_lock(home):
        metadata = json.loads((home / "install.json").read_text())
        config = json.loads((home / "config.json").read_text())["config"]
        if metadata.get("manager") != "native-install/1" or metadata.get("instance_id") != config["instance_id"]:
            raise ValueError("This installation is not managed by the native installer or its identity differs")
        stage_support(args)
        old = metadata["current"]
        if args.command == "update":
            candidate = stage(home, args.binary)
        else:
            selected = args.release or (metadata.get("previous") or {}).get("id")
            if not selected or not re.fullmatch(r"[0-9A-Za-z.+-]+", selected):
                raise ValueError("No previous release is available; pass a retained --release ID")
            candidate = json.loads((home / "releases" / selected / "release.json").read_text())
            if candidate["id"] != selected:
                raise ValueError("Release ID does not match its manifest")
        if candidate["id"] == old["id"]:
            return result(home, metadata)
        candidate_binary = home / "releases" / candidate["id"] / binary_name()
        if hashlib.sha256(candidate_binary.read_bytes()).hexdigest() != candidate["sha256"]:
            raise ValueError("Selected release checksum does not match its manifest")
        run([candidate_binary, "config", "validate", "--home", home], timeout=15)
        service = metadata.get("service")
        if service:
            service_command(service, "stop")
        try:
            with stopped_lock(home):
                backup = snapshot(home)
                activate(home, candidate)
        except Exception:
            if service:
                service_command(service, "start")
                health(home, metadata["instance_id"], old["version"])
            raise
        metadata.update(current=candidate, previous=old, updated_at=int(time.time() * 1000), backup=backup)
        healthy = None
        try:
            save(home / "install.json", metadata)
            if service:
                service_command(service, "start")
                healthy = health(home, metadata["instance_id"], candidate["version"])
        except Exception:
            # Restore executable only. Restoring a database could discard new work.
            if service:
                service_command(service, "stop")
            with stopped_lock(home):
                activate(home, old)
            metadata.update(current=old, previous=candidate, failed_release=candidate)
            save(home / "install.json", metadata)
            if service:
                service_command(service, "start")
                health(home, metadata["instance_id"], old["version"])
            raise ValueError("New version failed its health check; the prior executable was restored. Data was preserved.")
        return result(home, metadata, healthy)


def stage_support(args):
    bundle = getattr(args, 'bundle', None)
    if bundle is None:
        return
    spec = importlib.util.spec_from_file_location('consumer_bundle', Path(__file__).with_name('consumer-bundle.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    manifest = module.validate(bundle, args.binary)
    module.stage(args.home, bundle, manifest)


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("install", "update", "rollback", "inspect"))
    parser.add_argument("--home", required=True, type=Path)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--name", default="New agent")
    parser.add_argument("--bind", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--release")
    parser.add_argument("--no-service", action="store_true")
    parser.add_argument("--no-browser", action="store_true")
    parser.add_argument("--skip-runtime", action="store_true")
    parser.add_argument("--engine", choices=["native", "codex"], default="native")
    parser.add_argument("--runtime-binary", type=Path)
    parser.add_argument("--bundle", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    # Reject symbolic home before canonicalization discards that information.
    validate_home(args.home)
    args.home = args.home.expanduser().absolute()
    if args.binary:
        args.binary = args.binary.expanduser().absolute()
    if args.command in ("install", "update") and args.binary is None:
        parser.error("--binary is required for install/update")
    try:
        if args.command == "install":
            value = install(args)
        elif args.command == "inspect":
            value = result(args.home, json.loads((args.home / "install.json").read_text()))
        else:
            value = switch(args)
        print(json.dumps(value, indent=2))
        if args.command == "install" and not args.no_browser and value.get("endpoints"):
            endpoints = value["endpoints"]
            webbrowser.open(endpoints.get("welcome_url") or endpoints["base_url"])
        return 0
    except Exception as error:
        # subprocess stdout may contain credential material; never print it here.
        print("Installation stopped: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
