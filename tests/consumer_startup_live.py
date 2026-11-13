#!/usr/bin/env python3
"""Actual current-user startup/update/rollback in disposable Windows/macOS CI.

Only UUID-named jobs created by this fixture are removed. No model calls or
production installation. A version-only binary is labelled as a fixture.
"""
import argparse
import base64
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid
import zipfile


def settled_helper(status, policy, observed_after, binary_digest):
    """An API observation only; the installer independently owns admission."""
    helper = status.get('helper', {})
    return (status.get('policy') == policy and status.get('helper_connected') is True
            and not status.get('active_prompts') and not status.get('draining')
            and not (status.get('request') or {}).get('pending')
            and helper.get('observed_at', 0) > observed_after
            and helper.get('helper_instance_sha256') == binary_digest
            and helper.get('state') in ('idle', 'available', 'up_to_date', 'check_failed', 'succeeded', 'rolled_back')
            and not helper.get('transaction') and not helper.get('active_request')
            and not helper.get('enrollment'))


def retained_helper(status):
    return {key: value for key, value in status['helper'].items()
            if key not in ('observed_at', 'drain_until')}


def remove_stopped_fixture(path, timeout=10):
    """SchTasks End is asynchronous; wait only for this verified fixture's files."""
    deadline = time.monotonic() + timeout
    while True:
        try:
            shutil.rmtree(path)
            return
        except PermissionError as error:
            if getattr(error, 'winerror', None) not in (32, 33) or time.monotonic() >= deadline:
                raise
            time.sleep(.1)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--bundle', required=True, type=Path)
    p.add_argument('--baseline', required=True, type=Path)
    a = p.parse_args()
    if os.name != 'nt' and sys.platform != 'darwin':
        raise ValueError('This fixture qualifies actual Windows/macOS per-user startup')
    checks = []
    with tempfile.TemporaryDirectory(prefix='instance-native-startup-') as temporary:
        root = Path(temporary); bundle = root / 'candidate'; bundle.mkdir()
        with zipfile.ZipFile(a.bundle) as archive:
            for member in archive.infolist():
                path = bundle / member.filename
                if not path.resolve().is_relative_to(bundle.resolve()):
                    raise ValueError('Unsafe package path')
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(archive.read(member))
                if os.name != 'nt': path.chmod((member.external_attr >> 16) & 0o777)
        name = 'agent-instance.exe' if os.name == 'nt' else 'agent-instance'
        baseline = root / 'baseline'; shutil.copytree(bundle, baseline)
        shutil.copy2(a.baseline, baseline / name)
        manifest = json.loads((baseline / 'consumer-bundle.json').read_text())
        manifest['files'][name] = hashlib.sha256((baseline / name).read_bytes()).hexdigest()
        manifest['version'] = subprocess.check_output([str(baseline / name), '--version'], text=True).strip().split()[-1]
        (baseline / 'consumer-bundle.json').write_text(json.dumps(manifest))
        home = root / 'same installed individual café 日本'
        env = os.environ.copy()
        if os.name == 'nt':
            system = Path(os.environ['SystemRoot']) / 'System32'
            env['PATH'] = str(system) + os.pathsep + str(system / 'WindowsPowerShell/v1.0')
        else:
            tools = root / 'os-tools'; tools.mkdir()
            for tool in ('id', 'hostname', 'launchctl'):
                path = shutil.which(tool)
                if not path: raise ValueError('Native OS tool missing: ' + tool)
                (tools / tool).symlink_to(path)
            env['PATH'] = str(tools)
        env['PYTHONHOME'] = str(root / 'not-python'); env['PYTHONPATH'] = str(root / 'not-python')
        assert all(shutil.which(tool, path=env['PATH']) is None for tool in ('python', 'python3', 'node', 'cargo', 'git'))
        identifier = None
        def run(executable, *words, timeout=240):
            result = subprocess.run([str(executable), *map(str, words)], env=env, capture_output=True, text=True, timeout=timeout)
            if result.returncode:
                raise RuntimeError('Disposable native command failed: ' + result.stderr[-2500:])
            return json.loads(result.stdout)
        reenrollment = None
        try:
            engine_args = [] if manifest.get('native_runtime') else ['--skip-runtime']
            installed = run(baseline / name, 'setup', '--home', home, '--name', 'Startup café 日本',
                            '--bind', '127.0.0.1', '--no-browser', *engine_args)
            identifier = str(uuid.UUID(installed['instance_id']))
            assert installed['running_verified'] and installed['service']
            checks.append('actual limited current-user startup reaches authenticated identity health')
            if os.name == 'nt':
                # Hosted runners can be elevated. Inspect the actual registered
                # jobs as well, so a global logon trigger cannot pass unnoticed.
                script = r'''$ErrorActionPreference='Stop';
                  $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;
                  function Resolve-Sid($value) { if($value -match '^S-1-') { return (New-Object Security.Principal.SecurityIdentifier($value)).Value }; return (New-Object Security.Principal.NTAccount($value)).Translate([Security.Principal.SecurityIdentifier]).Value };
                  $scheduler=New-Object -ComObject Schedule.Service; $scheduler.Connect();
                  foreach($name in @('AgentInstance-ID','constellation-updates-ID')) {
                    $definition=$scheduler.GetFolder('\').GetTask($name).Definition;
                    $owner=Resolve-Sid $definition.Principal.UserId;
                    if($owner -ne $sid -or $definition.Principal.LogonType -ne 3 -or $definition.Principal.RunLevel -ne 0) { throw 'Startup principal is not the interactive limited owner' };
                    if($definition.Triggers.Count -ne 1) { throw 'Expected one owner logon trigger' };
                    $trigger=$definition.Triggers.Item(1);
                    if($trigger.Type -ne 9 -or -not $trigger.UserId) { throw 'Logon trigger is not owner-scoped' };
                    $triggerOwner=Resolve-Sid $trigger.UserId;
                    if($triggerOwner -ne $sid) { throw 'Logon trigger belongs to another owner' };
                  }; Write-Output 'owner-scoped startup verified'
                '''.replace('ID', identifier)
                powershell = Path(os.environ['SystemRoot']) / 'System32/WindowsPowerShell/v1.0/powershell.exe'
                verified = subprocess.run([str(powershell), '-NoProfile', '-NonInteractive', '-EncodedCommand',
                    base64.b64encode(script.encode('utf-16-le')).decode()], env=env, capture_output=True, text=True, timeout=30)
                assert verified.returncode == 0, verified.stderr
                checks.append('both actual Windows tasks use the limited owner and owner-only logon trigger')
            native = home / 'bin' / name
            # The initial setup may already have admitted its default hourly
            # metadata check. Disable future checks through the supported API;
            # observe the existing check to settlement rather than interrupt it.
            update_status = run(native, 'updates', 'status', '--home', home)
            policy = dict(update_status['policy'], check_minutes=0, install='on_request', agent_initiated=False)
            policy_file = root / 'idle-update-policy.json'; policy_file.write_text(json.dumps(policy))
            policy = run(native, 'updates', 'configure', '--home', home, '--file', policy_file)
            assert policy['check_minutes'] == 0 and policy['install'] == 'on_request' and not policy['agent_initiated']
            binary_digest = hashlib.sha256(native.read_bytes()).hexdigest()
            def await_settled(observed_after):
                deadline = time.monotonic() + 60
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise RuntimeError('Disposable helper did not reach a fresh settled observation; no reenrollment retry')
                    value = run(native, 'updates', 'status', '--home', home, timeout=min(10, remaining))
                    if settled_helper(value, policy, observed_after, binary_digest):
                        return value
                    time.sleep(min(.25, max(0, deadline - time.monotonic())))
            helper_before = await_settled(update_status.get('helper', {}).get('observed_at', 0))
            before = run(native, 'status', '--home', home, '--json')
            files = {str(path.relative_to(home)): hashlib.sha256(path.read_bytes()).hexdigest()
                     for path in [home / 'config.json', home / 'secrets/control-token', home / 'tls/root-ca.crt']}
            with closing(sqlite3.connect(home / 'state.sqlite3')) as db:
                db.execute('CREATE TABLE native_consumer_continuity(value TEXT)')
                db.execute('INSERT INTO native_consumer_continuity VALUES (?)', ('retained original work/unknown outcome fixture',))
                db.commit()
            state = Path.home() / '.local/share/constellation-updates' / identifier
            registration_bytes = (state / 'registration.json').read_bytes()
            registration = json.loads(registration_bytes)
            assert registration['instance_id'] == identifier and Path(registration['home']).resolve() == home.resolve()
            private = json.loads((home / 'install.json').read_text())['private_python']
            interpreter = home / 'support' / private['directory'] / private['executable']
            def helper_owned():
                # Real flock/msvcrt contention from the packaged interpreter;
                # no PID-only inference and no journal read outside ownership.
                code = """import importlib.util,json,pathlib,sys
spec=importlib.util.spec_from_file_location('fixture_worker',sys.argv[1]);w=importlib.util.module_from_spec(spec);spec.loader.exec_module(w)
try:
 with w.single_worker(pathlib.Path(sys.argv[2])):raise RuntimeError('Helper lifetime lock was not owned')
except w.LockBusy:print(json.dumps({'owned':True}))
"""
                assert run(interpreter, '-I', '-c', code, state / 'worker.py', state, timeout=15)['owned'] is True
            helper_owned()
            started = time.monotonic()
            enrolled = run(native, 'install-updates', '--home', home, timeout=90)
            assert enrolled['installed'] and enrolled['service'] == 'constellation-updates-' + identifier
            helper_after = await_settled(int(time.time() * 1000))
            helper_owned()
            assert (state / 'registration.json').read_bytes() == registration_bytes
            assert retained_helper(helper_after) == retained_helper(helper_before)
            assert run(native, 'status', '--home', home, '--json')['instance_id'] == before['instance_id']
            reenrollment = {'dispatches': 1, 'elapsed_seconds': round(time.monotonic() - started, 3),
                'service': enrolled['service'], 'registration_sha256': hashlib.sha256(registration_bytes).hexdigest(),
                'policy': policy, 'helper_lock_owned_before_and_after': True,
                'retained_helper_state_equal': True, 'initial_setup_check_may_have_run': True}
            checks.append('running idle helper reenrolls once through installed CLI with retained state, policy and lifetime lock')
            updated = run(bundle / name, 'setup', 'update', '--home', home, '--no-browser')
            assert updated['running_verified'] and updated['previous'] == installed['release']
            assert updated['release']['id'] != installed['release']['id']
            checks.append('actual startup job stops and starts the same home after executable update')
            rolled = run(bundle / name, 'setup', 'rollback', '--home', home, '--no-browser')
            assert rolled['running_verified'] and rolled['release'] == installed['release']
            assert run(native, 'status', '--home', home, '--json')['instance_id'] == before['instance_id']
            checks.append('actual previous executable rollback restores the same authenticated individual')
            for relative, expected in files.items():
                assert hashlib.sha256((home / relative).read_bytes()).hexdigest() == expected
            with closing(sqlite3.connect(home / 'state.sqlite3')) as db:
                assert db.execute('SELECT value FROM native_consumer_continuity').fetchall() == [('retained original work/unknown outcome fixture',)]
            checks.append('configuration, credential, CA and retained work data unchanged across both switches')
        finally:
            # Even a partially failed install has an owner-generated UUID in config.
            if identifier is None and (home / 'config.json').exists():
                identifier = str(uuid.UUID(json.loads((home / 'config.json').read_text(encoding='utf-8'))['config']['instance_id']))
            if identifier:
                if os.name == 'nt':
                    tool = str(Path(os.environ['SystemRoot']) / 'System32/schtasks.exe')
                    for job in ('AgentInstance-' + identifier, 'constellation-updates-' + identifier):
                        subprocess.run([tool, '/End', '/TN', job], capture_output=True, timeout=30)
                        subprocess.run([tool, '/Delete', '/TN', job, '/F'], capture_output=True, timeout=30)
                else:
                    domain = 'gui/' + str(os.getuid())
                    for label in ('io.agent-instance.' + identifier, 'io.constellation-updates-' + identifier):
                        subprocess.run(['/bin/launchctl', 'bootout', domain + '/' + label], capture_output=True, timeout=30)
                        (Path.home() / 'Library/LaunchAgents' / (label + '.plist')).unlink(missing_ok=True)
                state = Path.home() / '.local/share/constellation-updates' / identifier
                if state.exists():
                    registration = json.loads((state / 'registration.json').read_text())
                    if registration['instance_id'] != identifier or Path(registration['home']).resolve() != home.resolve():
                        raise ValueError('Disposable helper identity differs; files preserved')
                    remove_stopped_fixture(state)
                time.sleep(1)
    print(json.dumps({'passed': len(checks), 'checks': checks,
                      'reenrollment': reenrollment,
                      'baseline': 'identical source with version-only fixture label',
                      'limits': ['no model calls', 'no production state', 'native CI account policy only']}, indent=2))


if __name__ == '__main__':
    main()
