#!/usr/bin/env python3
"""Provision Constellation into one instance; output a backend, never activate it.

The compiled release lock arrives on stdin. Explicit local binaries are an
operator override, recorded by digest. No accounts are copied or contacted.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as out:
        json.dump(value, out, indent=2)
        out.write('\n')
        out.flush()
        os.fsync(out.fileno())
        temporary = Path(out.name)
    temporary.chmod(0o600)
    temporary.replace(path)


def platform_key():
    system = {'Linux': 'linux', 'Darwin': 'macos', 'Windows': 'windows'}[platform.system()]
    machine = platform.machine().lower()
    arch = {'arm64': 'aarch64', 'aarch64': 'aarch64', 'amd64': 'x86_64', 'x86_64': 'x86_64'}[machine]
    return system + '-' + arch


def unpack(archive, destination, name):
    """Read exactly one regular executable. Never extract archive paths or links."""
    with zipfile.ZipFile(archive) if archive.suffix == '.zip' else tarfile.open(archive) as src:
        entries = src.infolist() if isinstance(src, zipfile.ZipFile) else src.getmembers()
        matches = []
        for entry in entries:
            filename = entry.filename if isinstance(src, zipfile.ZipFile) else entry.name
            path = PurePosixPath(filename)
            if path.is_absolute() or '..' in path.parts or '\\' in filename:
                raise ValueError('Unsafe runtime archive path')
            if path.name == name:
                if isinstance(src, zipfile.ZipFile):
                    if entry.is_dir() or entry.file_size > 256 * 1024 * 1024 or (entry.external_attr >> 16) & 0o170000 == 0o120000:
                        raise ValueError('Unsafe runtime archive entry')
                elif not entry.isfile() or entry.size > 256 * 1024 * 1024:
                    raise ValueError('Unsafe runtime archive entry')
                matches.append(entry)
        if len(matches) != 1:
            raise ValueError('Runtime archive must contain exactly one executable')
        stream = src.open(matches[0]) if isinstance(src, zipfile.ZipFile) else src.extractfile(matches[0])
        with stream, destination.open('xb') as out:
            shutil.copyfileobj(stream, out)
    destination.chmod(0o700)


def obtain(lock, directory, name):
    release = next((a for a in lock['assets'] if a['platform'] == platform_key()), None)
    if release is None:
        raise ValueError('No pinned runtime for this platform. Supply --runtime-binary from a native build.')
    repository = lock['repository']
    tag = 'v' + lock['version']
    asset = release['asset']
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository) or '/' in asset or '\\' in asset:
        raise ValueError('Invalid compiled release lock')
    archive = directory / asset
    if shutil.which('gh'):
        # gh supplies private-repository authentication without exposing a token.
        subprocess.run(['gh', 'release', 'download', tag, '--repo', repository,
                        '--pattern', asset, '--output', str(archive)], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=240)
    else:
        url = f'https://github.com/{repository}/releases/download/{tag}/{asset}'
        try:
            with urllib.request.urlopen(url, timeout=60) as response, archive.open('xb') as out:
                total = 0
                while chunk := response.read(1024 * 1024):
                    total += len(chunk)
                    if total > 256 * 1024 * 1024:
                        raise ValueError('Runtime download exceeds its size limit')
                    out.write(chunk)
        except Exception as error:
            raise ValueError('Could not download the pinned runtime. For private repos, sign in with GitHub CLI, or supply --runtime-binary.') from error
    if digest(archive) != release['asset_sha256']:
        raise ValueError('Runtime archive checksum mismatch')
    binary = directory / name
    unpack(archive, binary, name)
    if digest(binary) != release['binary_sha256']:
        raise ValueError('Runtime executable checksum mismatch')
    return binary


def provision(home, binary):
    config = json.loads((home / 'config.json').read_text())['config']
    version = subprocess.check_output([str(binary), '--version'], text=True, timeout=15).strip()
    match = re.fullmatch(r'agent-runtime (\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)', version)
    if not match:
        raise ValueError('Expected a native agent-runtime executable')
    checksum = digest(binary)
    engine = home / 'engine'
    target = engine / 'releases' / (match[1] + '-' + checksum[:16]) / binary.name
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    if target.exists():
        if digest(target) != checksum:
            raise ValueError('Existing runtime release differs; preserved')
    else:
        shutil.copy2(binary, target)
        target.chmod(0o700)
    runtime_config = engine / 'runtime.json'
    if not runtime_config.exists():
        subprocess.run([str(target), '--home', str(engine), 'init', '--workspace', str(home / 'workspace'),
                        '--name', config['name'], '--provider', 'openai'], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=30)
        c = json.loads(runtime_config.read_text())
        c['provider']['api_key_env'] = 'AGENT_MODEL_API_KEY'
        if config['server'].get('public_url'):
            c['trusted_mcp_readonly_urls'] = [config['server']['public_url'].rstrip('/') + '/context/mcp']
        save(runtime_config, c)
    c = json.loads(runtime_config.read_text())
    key_env = c['provider']['api_key_env']
    backend = {'command': '${AGENT_HOME}/' + target.relative_to(home).as_posix(),
               'args': ['--home', '${AGENT_HOME}/engine', 'acp'],
               'working_directory': '${AGENT_HOME}/workspace', 'allowed_working_roots': ['${AGENT_HOME}/workspace'],
               'env': {}, 'secret_env': {key_env: 'native-model-key'} if (home / 'secrets/native-model-key').is_file() else {}}
    if (config.get('backend') or {}).get('command', '').rsplit('/', 1)[-1] in ['agent-runtime', 'agent-runtime.exe']:
        backend['secret_env'] = config['backend'].get('secret_env', backend['secret_env'])
    receipt = {'implementation': 'Constellation', 'version': match[1], 'sha256': checksum,
               'engine_home': str(engine), 'credentials_copied': False, 'backend': backend}
    save(engine / 'installation.json', receipt)
    return receipt


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--home', type=Path, required=True)
    parser.add_argument('--runtime-binary', type=Path)
    args = parser.parse_args()
    lock = json.load(sys.stdin)
    if args.home.is_symlink() or not (args.home / 'config.json').is_file():
        raise ValueError('Initialize an ordinary instance folder first')
    with tempfile.TemporaryDirectory(prefix='constellation-install-') as directory:
        name = 'agent-runtime.exe' if os.name == 'nt' else 'agent-runtime'
        binary = args.runtime_binary.resolve() if args.runtime_binary else obtain(lock, Path(directory), name)
        if not binary.is_file() or binary.name != name:
            raise ValueError('Supply the native agent-runtime executable')
        print(json.dumps(provision(args.home.resolve(), binary)))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        # Never print process environments, provider files or HTTP credentials.
        print(str(error) if isinstance(error, ValueError) else 'Native runtime installation failed; verify the selected release and platform.', file=sys.stderr)
        raise SystemExit(1)
