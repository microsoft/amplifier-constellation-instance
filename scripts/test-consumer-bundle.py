#!/usr/bin/env python3
"""Private-runtime extraction, integrity and installer continuity boundaries."""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


package = load('package-consumer')
consumer = load('consumer-bundle')


class BundleTests(unittest.TestCase):
    def test_shared_support_is_vendored_from_one_exact_umbrella_revision(self):
        folder = ROOT / 'scripts/vendor/python'
        receipt = json.loads((folder / 'vendor.json').read_text())
        self.assertEqual(receipt['repository'], 'microsoft/amplifier-constellation')
        self.assertRegex(receipt['commit'], r'^[a-f0-9]{40}$')
        self.assertEqual(receipt['path'], 'packaging/python')
        for name, expected in receipt['files'].items():
            self.assertEqual(hashlib.sha256((folder / name).read_bytes()).hexdigest(), expected, name)

    def archive(self, root, entries):
        path = root / 'python.tar.gz'
        with tarfile.open(path, 'w:gz') as out:
            for name, content, kind in entries:
                entry = tarfile.TarInfo(name)
                entry.mode = 0o755
                if kind:
                    entry.type = kind
                    entry.linkname = content
                    out.addfile(entry)
                else:
                    entry.size = len(content)
                    out.addfile(entry, io.BytesIO(content))
        return path

    def fixture(self, root):
        lock = json.loads((ROOT / 'scripts/vendor/python/lock.json').read_text())
        asset = next(a for a in lock['assets'] if a['target'] == 'x86_64-pc-windows-msvc')
        files = {'agent-instance.exe': b'instance', 'setup/native-install.py': b'fixed installer',
                 'setup/consumer-bundle.py': b'fixed staging',
                 'setup/private-python-lock.json': json.dumps(lock).encode(),
                 f"support/{lock['directory']}/{asset['executable']}": b'private python',
                 f"support/{lock['directory']}/LICENSE.txt": b'retained license'}
        for name, content in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        manifest = {'schema': 1, 'kind': 'instance-consumer-bundle', 'target': asset['target'],
                    'python_directory': lock['directory'], 'python_executable': asset['executable'],
                    'python_archive_sha256': asset['sha256'], 'binary': 'agent-instance.exe',
                    'files': {k: hashlib.sha256(v).hexdigest() for k, v in files.items()}}
        (root / 'consumer-bundle.json').write_text(json.dumps(manifest))
        return manifest

    def test_distributor_internal_alias_is_copied_as_regular_file(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            archive = self.archive(root, [('python/bin/python3.13', b'python', None),
                                          ('python/bin/python3', 'python3.13', tarfile.SYMTYPE)])
            package.unpack_python(archive, root / 'out')
            path = root / 'out/bin/python3'
            self.assertEqual(path.read_bytes(), b'python')
            self.assertFalse(path.is_symlink())

    def test_traversal_absolute_and_cross_root_links_rejected(self):
        for name, content, kind in [('python/../../escape', b'no', None),
                                    ('python/bin/a', '../../../escape', tarfile.SYMTYPE),
                                    ('python/bin/a', '/etc/passwd', tarfile.SYMTYPE),
                                    ('python/bin/a', 'C:\\secret', tarfile.SYMTYPE),
                                    ('different/file', b'no', None)]:
            with self.subTest(name=name, content=content), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                archive = self.archive(root, [(name, content, kind)])
                with self.assertRaises(ValueError):
                    package.unpack_python(archive, root / 'out')
                self.assertFalse((root / 'escape').exists())

    def test_cyclic_and_duplicate_archives_rejected(self):
        variants = [[('python/a', 'b', tarfile.SYMTYPE), ('python/b', 'a', tarfile.SYMTYPE)],
                    [('python/a', b'one', None), ('python/a', b'two', None)]]
        for entries in variants:
            with tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                with self.assertRaises(ValueError):
                    package.unpack_python(self.archive(root, entries), root / 'out')

    def test_pinned_archive_corruption_rejected_before_extraction(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'python.tar.gz').write_bytes(b'bad')
            with self.assertRaisesRegex(ValueError, 'checksum'):
                package.download({'filename': 'python.tar.gz', 'size': 3,
                                  'sha256': '0' * 64}, root)

    def test_bundle_validates_every_required_file_and_platform_lock(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            manifest = self.fixture(root)
            self.assertEqual(consumer.validate(root, root / 'agent-instance.exe'), manifest)
            (root / 'setup/native-install.py').write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'checksum'):
                consumer.validate(root, root / 'agent-instance.exe')

    def test_wrong_python_platform_refuses_without_installation_write(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            manifest = self.fixture(root)
            manifest['target'] = 'aarch64-pc-windows-msvc'
            (root / 'consumer-bundle.json').write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, 'platform lock'):
                consumer.validate(root, root / 'agent-instance.exe')
            self.assertFalse((root / 'home').exists())

    def test_independent_installs_and_same_bundle_retry_preserve_data(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = root / 'bundle'; bundle.mkdir()
            manifest = self.fixture(bundle)
            homes = [root / 'one', root / 'two']
            for home in homes:
                home.mkdir(); (home / 'config.json').write_bytes(b'original selected engine')
                (home / 'unknown-receipts').write_bytes(b'never replay')
                target = consumer.stage(home, bundle, manifest)
                before = consumer.sha(target / 'python.exe')
                self.assertEqual(consumer.stage(home, bundle, manifest), target)
                self.assertEqual(consumer.sha(target / 'python.exe'), before)
                self.assertEqual((home / 'config.json').read_bytes(), b'original selected engine')
                self.assertEqual((home / 'unknown-receipts').read_bytes(), b'never replay')
            self.assertNotEqual((homes[0] / 'support' / manifest['python_directory'] / 'python.exe').stat().st_ino,
                                (homes[1] / 'support' / manifest['python_directory'] / 'python.exe').stat().st_ino)

    def test_modified_installed_runtime_is_preserved_and_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / 'bundle'; bundle.mkdir()
            manifest = self.fixture(bundle); home = root / 'home'; home.mkdir()
            target = consumer.stage(home, bundle, manifest)
            (target / 'python.exe').write_bytes(b'owner changed')
            with self.assertRaisesRegex(ValueError, 'preserved'):
                consumer.stage(home, bundle, manifest)
            self.assertEqual((target / 'python.exe').read_bytes(), b'owner changed')

    def test_unlisted_bundle_file_is_not_copied_into_private_runtime(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / 'bundle'; bundle.mkdir()
            manifest = self.fixture(bundle); home = root / 'home'; home.mkdir()
            (bundle / 'support' / manifest['python_directory'] / 'unexpected.py').write_text('unrelated')
            target = consumer.stage(home, bundle, manifest)
            self.assertFalse((target / 'unexpected.py').exists())

    def native_fixture(self, root, manifest):
        name = 'agent-runtime.exe' if os.name == 'nt' else 'agent-runtime'
        folder = root / 'engine/native'; folder.mkdir(parents=True)
        (folder / name).write_bytes(b'verified fixture engine')
        (folder / 'release.zip').write_bytes(b'verified complete published archive with notices')
        runtime = {'platform': 'fixture-platform', 'version': '0.15.4', 'binary': 'engine/native/' + name,
                   'archive': 'engine/native/release.zip', 'binary_sha256': consumer.sha(folder / name),
                   'archive_sha256': consumer.sha(folder / 'release.zip')}
        manifest['native_runtime'] = runtime
        for field, digest in [('binary', 'binary_sha256'), ('archive', 'archive_sha256')]:
            manifest['files'][runtime[field]] = runtime[digest]
        (root / 'setup/native-runtime-lock.json').write_text(json.dumps({'version': '0.15.4', 'assets': [{
            'platform': 'fixture-platform', 'asset': 'release.zip', 'binary_sha256': runtime['binary_sha256'],
            'asset_sha256': runtime['archive_sha256']}]}))
        return runtime

    def test_native_engine_and_original_release_archive_are_private_and_idempotent(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / 'bundle'; bundle.mkdir()
            manifest = self.fixture(bundle); runtime = self.native_fixture(bundle, manifest)
            home = root / 'home'; home.mkdir(); (home / 'config.json').write_bytes(b'existing selected engine')
            target = consumer.stage_native(home, bundle, manifest)
            self.assertEqual(consumer.sha(target), runtime['binary_sha256'])
            self.assertEqual(consumer.sha(target.parent / 'release.zip'), runtime['archive_sha256'])
            self.assertEqual(consumer.stage_native(home, bundle, manifest), target)
            self.assertEqual((home / 'config.json').read_bytes(), b'existing selected engine')

    def test_native_digest_path_escape_and_changed_existing_copy_are_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / 'bundle'; bundle.mkdir()
            manifest = self.fixture(bundle); runtime = self.native_fixture(bundle, manifest)
            home = root / 'home'; home.mkdir()
            target = consumer.stage_native(home, bundle, manifest)
            target.write_bytes(b'changed owner bytes')
            with self.assertRaisesRegex(ValueError, 'preserved'):
                consumer.stage_native(home, bundle, manifest)
            self.assertEqual(target.read_bytes(), b'changed owner bytes')
            runtime['binary_sha256'] = '../escape'
            lock = json.loads((bundle / 'setup/native-runtime-lock.json').read_text())
            lock['assets'][0]['binary_sha256'] = '../escape'
            (bundle / 'setup/native-runtime-lock.json').write_text(json.dumps(lock))
            with self.assertRaisesRegex(ValueError, 'digest'):
                consumer.stage_native(home, bundle, manifest)
            self.assertFalse((home / 'escape').exists())

    @unittest.skipIf(os.name == 'nt', 'Creating links requires Windows test-user privileges')
    def test_private_directory_links_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / 'bundle'; bundle.mkdir()
            manifest = self.fixture(bundle); home = root / 'home'; home.mkdir()
            other = root / 'other'; other.mkdir(); (home / 'support').symlink_to(other)
            with self.assertRaisesRegex(ValueError, 'ordinary'):
                consumer.stage(home, bundle, manifest)
            self.assertEqual(list(other.iterdir()), [])


if __name__ == '__main__':
    unittest.main()
