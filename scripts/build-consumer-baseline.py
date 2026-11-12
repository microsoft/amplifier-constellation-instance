#!/usr/bin/env python3
"""CI-only version fixture for actual same-home native update/rollback tests.

Build identical product source with a distinct test version; restore source and
the original release executable even on failure. This is never a release asset.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', required=True, type=Path)
    p.add_argument('--output', required=True, type=Path)
    a = p.parse_args()
    root = Path(__file__).resolve().parent.parent
    binary = a.binary.resolve()
    source = root / 'Cargo.toml'; lock = root / 'Cargo.lock'
    original_source = source.read_bytes(); original_lock = lock.read_bytes()
    original = subprocess.check_output([str(binary), '--version'], text=True).strip().split()[-1]
    fixture = original + '-consumer-fixture'
    source_text = original_source.decode()
    replacement = source_text.replace('version = "' + original + '"', 'version = "' + fixture + '"', 1)
    if replacement == source_text:
        raise ValueError('Expected workspace package version was not found')
    lock_text, count = re.subn(r'(name = "agent-instance"\nversion = ")' + re.escape(original) + '"',
                              lambda m: m.group(1) + fixture + '"', original_lock.decode(), count=1)
    if count != 1:
        raise ValueError('Expected workspace lock version was not found')
    a.output.mkdir(parents=True, exist_ok=True)
    target = a.output / binary.name
    with tempfile.TemporaryDirectory(prefix='consumer-baseline-build-') as temporary:
        backup = Path(temporary) / binary.name
        shutil.copy2(binary, backup)
        try:
            source.write_bytes(replacement.encode()); lock.write_bytes(lock_text.encode())
            subprocess.run(['cargo', '+1.93.0', 'build', '--release', '--locked'], cwd=root, check=True, timeout=1800)
            if subprocess.check_output([str(binary), '--version'], text=True).strip() != 'agent-instance ' + fixture:
                raise ValueError('Version fixture did not build at the expected native path')
            shutil.copy2(binary, target)
        finally:
            source.write_bytes(original_source); lock.write_bytes(original_lock)
            shutil.copy2(backup, binary)
        assert hashlib.sha256(binary.read_bytes()).digest() == hashlib.sha256(backup.read_bytes()).digest()
    result = {'kind': 'identical-source-version-fixture', 'version': fixture,
              'fixture_sha256': hashlib.sha256(target.read_bytes()).hexdigest(),
              'candidate_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'source_and_candidate_restored': True}
    (a.output / 'fixture.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
