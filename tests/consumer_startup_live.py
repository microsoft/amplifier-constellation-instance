#!/usr/bin/env python3
"""Actual current-user startup/update/rollback in disposable Windows/macOS CI.

Only UUID-named jobs created by this fixture are removed. No model calls or
production installation. A version-only binary is labelled as a fixture.
"""
import argparse
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
        home = root / 'same installed individual'
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
        def run(executable, *words):
            result = subprocess.run([str(executable), *map(str, words)], env=env, capture_output=True, text=True, timeout=240)
            if result.returncode:
                raise RuntimeError('Disposable native command failed: ' + result.stderr[-2500:])
            return json.loads(result.stdout)
        try:
            engine_args = [] if manifest.get('native_runtime') else ['--skip-runtime']
            installed = run(baseline / name, 'setup', '--home', home, '--name', 'Disposable startup',
                            '--bind', '127.0.0.1', '--no-browser', *engine_args)
            identifier = str(uuid.UUID(installed['instance_id']))
            assert installed['running_verified'] and installed['service']
            checks.append('actual limited current-user startup reaches authenticated identity health')
            native = home / 'bin' / name
            before = run(native, 'status', '--home', home, '--json')
            files = {str(path.relative_to(home)): hashlib.sha256(path.read_bytes()).hexdigest()
                     for path in [home / 'config.json', home / 'secrets/control-token', home / 'tls/root-ca.crt']}
            with sqlite3.connect(home / 'state.sqlite3') as db:
                db.execute('CREATE TABLE native_consumer_continuity(value TEXT)')
                db.execute('INSERT INTO native_consumer_continuity VALUES (?)', ('retained original work/unknown outcome fixture',))
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
            with sqlite3.connect(home / 'state.sqlite3') as db:
                assert db.execute('SELECT value FROM native_consumer_continuity').fetchall() == [('retained original work/unknown outcome fixture',)]
            checks.append('configuration, credential, CA and retained work data unchanged across both switches')
        finally:
            # Even a partially failed install has an owner-generated UUID in config.
            if identifier is None and (home / 'config.json').exists():
                identifier = str(uuid.UUID(json.loads((home / 'config.json').read_text())['config']['instance_id']))
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
                    shutil.rmtree(state)
                time.sleep(1)
    print(json.dumps({'passed': len(checks), 'checks': checks,
                      'baseline': 'identical source with version-only fixture label',
                      'limits': ['no model calls', 'no production state', 'native CI account policy only']}, indent=2))


if __name__ == '__main__':
    main()
