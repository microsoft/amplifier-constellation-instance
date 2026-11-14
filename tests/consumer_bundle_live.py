#!/usr/bin/env python3
"""Actual consumer setup/restart in a disposable home with no developer PATH.

No startup registration, model calls, production profiles or credentials.
The bundled default engine is initialized without copying an account.
The runner's Python drives the test; only the package interpreter runs setup.
"""
import argparse
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import tempfile
import time
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', required=True, type=Path)
    args = parser.parse_args()
    checks = []
    with tempfile.TemporaryDirectory(prefix='instance-consumer-') as temporary:
        root = Path(temporary)
        bundle = root / 'download with spaces'; bundle.mkdir()
        with zipfile.ZipFile(args.bundle) as archive:
            for entry in archive.infolist():
                target = bundle / entry.filename
                if target.resolve().is_relative_to(bundle.resolve()) is False:
                    raise ValueError('Unsafe bundle member')
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(archive.read(entry))
                if os.name != 'nt':
                    target.chmod((entry.external_attr >> 16) & 0o777)
        executable = bundle / ('agent-instance.exe' if os.name == 'nt' else 'agent-instance')
        manifest = json.loads((bundle / 'consumer-bundle.json').read_text())
        home = root / 'installed agent café 日本'
        env = os.environ.copy()
        env.pop('PYTHONPATH', None); env.pop('PYTHONHOME', None)
        if os.name == 'nt':
            # OS tools only, no Git/WSL/Python/Rust/Node/winget locations.
            system = Path(os.environ['SystemRoot']) / 'System32'
            env['PATH'] = str(system) + os.pathsep + str(system / 'WindowsPowerShell/v1.0')
        else:
            tools = root / 'os-tools'; tools.mkdir()
            for name in ('id', 'hostname'):
                path = shutil.which(name)
                if path:
                    (tools / name).symlink_to(path)
            env['PATH'] = str(tools)
        assert all(shutil.which(name, path=env['PATH']) is None for name in ('python', 'python3', 'node', 'cargo', 'rustc', 'git'))
        # -I must also disregard ambient Python configuration.
        env['PYTHONHOME'] = str(root / 'not-a-python')
        env['PYTHONPATH'] = str(root / 'not-a-python')
        def command(*words):
            result = subprocess.run([str(executable), *map(str, words)], env=env,
                                    capture_output=True, text=True, timeout=120)
            if result.returncode:
                raise RuntimeError('Consumer command failed: ' + result.stderr[-3000:])
            return result
        engine_args = [] if manifest.get('native_runtime') else ['--skip-runtime']
        installed = json.loads(command('setup', '--home', home, '--name', 'Disposable café 日本',
                                       '--bind', '127.0.0.1', '--no-service', '--no-browser', *engine_args).stdout)
        assert installed['running_verified'] is False
        assert json.loads((home / 'config.json').read_text(encoding='utf-8'))['config']['name'] == 'Disposable café 日本'
        checks.append('actual native setup with developer tools absent and hostile Python environment')
        private = home / 'support' / manifest['python_directory'] / manifest['python_executable']
        assert private.is_file()
        installed_support = json.loads((home / "install.json").read_text())["private_python"]
        assert installed_support == {"directory": manifest["python_directory"], "executable": manifest["python_executable"]}
        output = subprocess.check_output([str(private), '-I', '-c',
            'import hashlib,json,sqlite3,ssl,sys; print(json.dumps({"python":sys.version_info[:3],"sqlite":sqlite3.sqlite_version,"tls":ssl.OPENSSL_VERSION}))'],
            env=env, text=True)
        assert json.loads(output)['python'] == [3, 13, 18]
        checks.append('private interpreter loads actual SQLite and TLS without external Python')
        if manifest.get('native_runtime'):
            receipt = json.loads((home / 'engine/installation.json').read_text())
            assert receipt['sha256'] == manifest['native_runtime']['binary_sha256']
            assert receipt['credentials_copied'] is False
            assert json.loads((home / 'engine/runtime.json').read_text(encoding='utf-8'))['name'] == 'Disposable café 日本'
            checks.append('published default native engine installed from verified bundle without setup download')
        executable = home / 'bin' / executable.name
        before = {name: hashlib.sha256((home / name).read_bytes()).hexdigest()
                  for name in ('config.json', 'secrets/control-token', 'tls/root-ca.crt')}
        db = sqlite3.connect(home / 'state.sqlite3')
        db.execute('CREATE TABLE consumer_preservation(value TEXT NOT NULL)')
        db.execute('INSERT INTO consumer_preservation VALUES (?)', ('same session/task/unknown receipt fixture',))
        db.commit(); db.close()
        def start_and_check():
            process = subprocess.Popen([str(executable), 'serve', '--home', str(home)], env=env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 35
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError('Disposable instance stopped before readiness: ' + process.stderr.read().decode()[-1500:])
                    result = subprocess.run([str(executable), 'status', '--home', str(home), '--json'],
                                            env=env, capture_output=True, text=True, timeout=8)
                    if result.returncode == 0:
                        value = json.loads(result.stdout)
                        if value.get('instance_id') == installed['instance_id']:
                            return value
                    time.sleep(.15)
                raise RuntimeError('Disposable instance readiness timed out')
            finally:
                process.terminate()
                try: process.wait(timeout=15)
                except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=5)
                process.stderr.close()
        first = start_and_check()
        for name in ('secrets/control-token', 'tls/root-ca.crt'):
            assert hashlib.sha256((home / name).read_bytes()).hexdigest() == before[name]
        # The first real start assigns previously unspecified ports. Capture
        # restart continuity after that documented first-start configuration.
        before = {name: hashlib.sha256((home / name).read_bytes()).hexdigest() for name in before}
        second = start_and_check()
        assert first['instance_id'] == second['instance_id'] == installed['instance_id']
        assert {name: hashlib.sha256((home / name).read_bytes()).hexdigest() for name in before} == before
        with closing(sqlite3.connect(home / 'state.sqlite3')) as db:
            assert db.execute('SELECT value FROM consumer_preservation').fetchall() == [('same session/task/unknown receipt fixture',)]
        checks.append('actual same-installation restart preserves identity/config/credential/CA and retained data')
        # Installation support survives removal of the original download.
        shutil.rmtree(bundle)
        assert start_and_check()['instance_id'] == installed['instance_id']
        assert private.is_file()
        checks.append('installed service and private interpreter remain independent of download folder')
    print(json.dumps({'passed': len(checks), 'checks': checks,
                      'limits': ['no OS startup registration', 'no model calls or production state',
                                 'engine omitted only when the published lock has no matching target']}, indent=2))


if __name__ == '__main__':
    main()
