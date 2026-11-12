"""Verified, private setup support for native-install/1. No engine selection."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import uuid


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def ordinary(root, relative):
    p = PurePosixPath(relative)
    if not p.parts or p.is_absolute() or '..' in p.parts or '\\' in relative or ':' in relative:
        raise ValueError('Invalid consumer bundle path')
    current = root
    for part in p.parts:
        current = current / part
        if current.is_symlink():
            raise ValueError('Consumer bundle links are not accepted')
    if not current.is_file():
        raise ValueError('Consumer bundle is incomplete')
    return current


def validate(root, binary):
    if root.is_symlink():
        raise ValueError('Use an ordinary consumer bundle folder')
    manifest = json.loads(ordinary(root, 'consumer-bundle.json').read_text())
    if manifest.get('schema') != 1 or manifest.get('kind') != 'instance-consumer-bundle':
        raise ValueError('Unsupported consumer bundle')
    if ordinary(root, manifest['binary']).resolve() != binary.resolve():
        raise ValueError('Consumer bundle belongs to another executable')
    lock = json.loads(ordinary(root, 'setup/private-python-lock.json').read_text())
    if manifest['python_directory'] != lock['directory']:
        raise ValueError('Consumer Python identity differs from its lock')
    asset = next((a for a in lock['assets'] if a['target'] == manifest['target']), None)
    if not asset or asset['sha256'] != manifest['python_archive_sha256'] or asset['executable'] != manifest['python_executable']:
        raise ValueError('Consumer Python does not match its exact platform lock')
    files = manifest.get('files')
    if not isinstance(files, dict) or not files:
        raise ValueError('Consumer bundle has no file inventory')
    for relative, expected in files.items():
        if sha(ordinary(root, relative)) != expected:
            raise ValueError('Consumer bundle checksum mismatch: ' + relative)
    for required in (manifest['binary'], 'setup/native-install.py', 'setup/private-python-lock.json',
                     'setup/consumer-bundle.py', f"support/{lock['directory']}/{asset['executable']}"):
        if required not in files:
            raise ValueError('Consumer bundle file inventory is incomplete')
    return manifest


def stage(home, root, manifest):
    """Install an immutable per-installation interpreter before backend setup."""
    name = manifest['python_directory']
    source = root / 'support' / name
    support = home / 'support'
    support.mkdir(mode=0o700, exist_ok=True)
    if support.is_symlink():
        raise ValueError('Installation support must be an ordinary directory')
    target = support / name
    prefix = f'support/{name}/'
    files = {p[len(prefix):]: h for p, h in manifest['files'].items() if p.startswith(prefix)}
    def verified(folder):
        if folder.is_symlink():
            raise ValueError('Private interpreter directory is a link')
        for path, expected in files.items():
            if sha(ordinary(folder, path)) != expected:
                raise ValueError('Private interpreter differs; existing files were preserved')
    if target.exists():
        verified(target)
        return target
    temporary = support / ('.python-' + uuid.uuid4().hex)
    try:
        # Copy only the closed inventory. Unrelated files beside a downloaded
        # bundle never become importable installation support.
        temporary.mkdir(mode=0o700)
        for relative in files:
            incoming = ordinary(source, relative)
            destination = temporary / relative
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            shutil.copyfile(incoming, destination)
            destination.chmod(0o700 if incoming.stat().st_mode & 0o111 else 0o600)
        verified(temporary)
        for path in temporary.rglob('*'):
            path.chmod(0o700 if path.is_dir() or path.stat().st_mode & 0o111 else 0o600)
        temporary.chmod(0o700)
        os.rename(temporary, target)
    finally:
        if temporary.exists():
            shutil.rmtree(temporary)
    return target
