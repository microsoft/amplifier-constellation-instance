#!/usr/bin/env python3
"""Build an offline setup bundle from a native executable and pinned Python.

This is release tooling. Consumers run the bundled executable's setup command;
they do not run this script or install Python. All distributor notices survive.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import zipfile
import importlib.util

ROOT = Path(__file__).resolve().parent.parent


support_spec = importlib.util.spec_from_file_location('python_support', ROOT / 'scripts/vendor/python/python_support.py')
support = importlib.util.module_from_spec(support_spec)
support_spec.loader.exec_module(support)
digest = support.digest
safe_name = support.safe_name
unpack_python = support.unpack_python
download = support.download


def package(binary, output, target, commit, cache):
    if not re.fullmatch(r'[a-f0-9]{40}', commit):
        raise ValueError('An exact source commit is required')
    lock = json.loads((ROOT / 'scripts/vendor/python/lock.json').read_text())
    asset = next((a for a in lock['assets'] if a['target'] == target), None)
    if asset is None:
        raise ValueError('No pinned private Python for this platform')
    binary = binary.resolve()
    guard_spec = importlib.util.spec_from_file_location('native_binary', ROOT / 'scripts/native_binary.py')
    guard = importlib.util.module_from_spec(guard_spec)
    guard_spec.loader.exec_module(guard)
    family = 'windows' if 'windows' in target else ('macos' if 'apple' in target else 'linux')
    native_metadata = guard.inspect(binary.read_bytes(), family + '-' + target.split('-')[0])
    if family == 'macos':
        lines = subprocess.check_output(['/usr/bin/otool', '-L', str(binary)], text=True).splitlines()[1:]
        libraries = [line.strip().split(' (', 1)[0] for line in lines if line.strip()]
        if not libraries or any(not name.startswith(('/usr/lib/', '/System/Library/')) for name in libraries):
            raise ValueError('Native executable depends on non-system Mac libraries')
        native_metadata['system_libraries'] = libraries
    version = subprocess.check_output([str(binary), '--version'], text=True).strip()
    if not re.fullmatch(r'agent-instance \d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?', version):
        raise ValueError('Unexpected native executable identity')
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        python = root / 'support' / lock['directory']
        support.prepare(target, cache, python)
        executable = 'agent-instance.exe' if 'windows' in target else 'agent-instance'
        (root / executable).write_bytes(binary.read_bytes())
        (root / executable).chmod(0o755)
        runtime_lock = json.loads((ROOT / 'scripts/native-runtime-lock.json').read_text())
        runtime_platform = family + '-' + target.split('-')[0]
        runtime_asset = next((a for a in runtime_lock['assets'] if a['platform'] == runtime_platform), None)
        runtime = None
        if runtime_asset is not None:
            spec = importlib.util.spec_from_file_location('provision_native', ROOT / 'scripts/provision-native.py')
            provision = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(provision)
            if provision.platform_key() != runtime_platform:
                raise ValueError('Default engine packaging requires the matching native builder')
            folder = root / 'engine/native'; folder.mkdir(parents=True)
            name = 'agent-runtime.exe' if family == 'windows' else 'agent-runtime'
            engine = provision.obtain(runtime_lock, folder, name)
            actual = subprocess.check_output([str(engine), '--version'], text=True).strip()
            if actual != 'agent-runtime ' + runtime_lock['version']:
                raise ValueError('Pinned default engine version mismatch')
            guard.inspect(engine.read_bytes(), runtime_platform)
            # Retain the entire exact published archive, including its notices.
            runtime = {'version': runtime_lock['version'], 'platform': runtime_platform,
                       'binary': 'engine/native/' + name, 'binary_sha256': runtime_asset['binary_sha256'],
                       'archive': 'engine/native/' + runtime_asset['asset'], 'archive_sha256': runtime_asset['asset_sha256']}
        for name in ('LICENSE', 'THIRD-PARTY-NOTICES.txt'):
            (root / name).write_bytes((ROOT / name).read_bytes())
        for name in ('native-install.py', 'consumer-bundle.py', 'native-runtime-lock.json'):
            dst = root / 'setup' / name
            dst.parent.mkdir(exist_ok=True)
            dst.write_bytes((ROOT / 'scripts' / name).read_bytes())
        (root / 'setup/private-python-lock.json').write_bytes((ROOT / 'scripts/vendor/python/lock.json').read_bytes())
        files = {str(p.relative_to(root)).replace(os.sep, '/'): digest(p)
                 for p in root.rglob('*') if p.is_file()}
        manifest = {'schema': 1, 'kind': 'instance-consumer-bundle', 'version': version.split()[1],
                    'target': target, 'source': commit, 'binary': executable,
                    'native_binary': native_metadata,
                    'native_runtime': runtime,
                    'python_directory': lock['directory'], 'python_executable': asset['executable'],
                    'python_archive_sha256': asset['sha256'], 'files': files}
        (root / 'consumer-bundle.json').write_text(json.dumps(manifest, indent=2) + '\n')
        (root / 'READ-ME.txt').write_text(
            'Constellation Instance\n\nKeep this folder together. Python is private to the package.\n'
            f'Create an installation: {executable} setup --home PATH --name NAME\n'
            f'Update an existing installation: {executable} setup update --home PATH\n'
            'Setup uses published, checksummed engine releases; it never builds source.\n'
            'Existing installations retain their chosen engine, accounts and history.\n')
        output.mkdir(parents=True, exist_ok=True)
        destination = output / f'agent-instance-consumer-{target}.zip'
        with zipfile.ZipFile(destination, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=6) as bundle:
            for path in sorted(root.rglob('*')):
                if not path.is_file():
                    continue
                entry = zipfile.ZipInfo(str(path.relative_to(root)).replace(os.sep, '/'), (1980, 1, 1, 0, 0, 0))
                entry.create_system = 3
                entry.external_attr = (0o100755 if path.stat().st_mode & 0o111 else 0o100644) << 16
                entry.compress_type = zipfile.ZIP_DEFLATED
                bundle.writestr(entry, path.read_bytes())
        destination.with_suffix('.zip.sha256').write_text(digest(destination) + '  ' + destination.name + '\n')
        return destination


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--target', required=True)
    p.add_argument('--commit', required=True)
    p.add_argument('--cache', type=Path, required=True)
    a = p.parse_args()
    print(package(a.binary, a.output, a.target, a.commit, a.cache))
