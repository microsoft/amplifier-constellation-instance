"""Model-free tests for the optional Linux container administration helpers."""
import copy
import importlib.util
import json
import pathlib
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parent

def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / (name + '.py'))
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value

install = module('install')
firewall = module('firewall')

class ContainerContracts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.manifest = {'container': 'example-agent', 'state': self.temp.name,
                         'ip': '172.30.240.10', 'mounts': [{'path': self.temp.name}],
                         'ports': [{'host': 41000, 'container': 41000}],
                         'services': {'example.service': {'argv': ['/bin/true'], 'cwd': self.temp.name}},
                         'routes': [{'listen': 41001, 'target': ['127.0.0.1', 41001]}]}

    def test_scoped_installation_is_valid(self):
        install.validate_manifest(self.manifest)

    def test_admin_is_explicit_and_keeps_host_capabilities_absent(self):
        default = install.isolation_arguments()
        self.assertIn('--read-only', default)
        self.assertIn('no-new-privileges', default)
        self.assertNotIn('--cap-add', default)
        self.manifest['container_admin'] = True
        install.validate_manifest(self.manifest)
        admin = install.isolation_arguments(True)
        self.assertNotIn('--privileged', admin)
        self.assertNotIn('--read-only', admin)
        self.assertNotIn('no-new-privileges', admin)
        self.assertEqual(admin[:2], ['--cap-drop', 'ALL'])
        for denied in ['SYS_ADMIN', 'NET_ADMIN', 'SYS_PTRACE', 'MKNOD', 'NET_RAW']:
            self.assertNotIn(denied, admin)
        for invalid in ['true', 1, None]:
            self.manifest['container_admin'] = invalid
            with self.assertRaises(ValueError): install.validate_manifest(self.manifest)

    def test_admin_image_identity_is_checked_before_state_or_services(self):
        self.manifest.update(container_admin=True, image='unprepared-image')
        with patch.object(install.subprocess, 'run') as process, patch.object(install, 'run', return_value='{}'):
            process.return_value.returncode = 1
            with self.assertRaisesRegex(ValueError, 'Dockerfile.admin'):
                install.install(self.manifest)
        self.assertEqual(list(pathlib.Path(self.temp.name).iterdir()), [])

    def test_broad_and_control_mounts_are_rejected(self):
        for source in ['/', str(pathlib.Path.home()), '/etc', '/run/docker.sock', '/proc', '/sys', str(pathlib.Path.home()/'.ssh')]:
            with self.subTest(source=source):
                m = copy.deepcopy(self.manifest)
                m['mounts'] = [{'path': source}]
                with self.assertRaises(ValueError): install.validate_manifest(m)

    def test_symlink_cannot_hide_control_mount(self):
        link = pathlib.Path(self.temp.name)/'socket'
        link.symlink_to('/run/docker.sock')
        self.manifest['mounts'] = [{'path': str(link)}]
        with self.assertRaises(ValueError): install.validate_manifest(self.manifest)

    def test_unit_and_network_inputs_fail_before_installation(self):
        for key, value in [('container', 'bad\nname'), ('state', '/tmp/bad%name'), ('ip', '192.0.2.10')]:
            with self.subTest(key=key):
                m = copy.deepcopy(self.manifest); m[key] = value
                with self.assertRaises(ValueError): install.validate_manifest(m)
        self.manifest['services'] = {'agent.service\nWants=other.service': {'argv': ['/bin/true'], 'cwd': '/tmp'}}
        with self.assertRaises(ValueError): install.validate_manifest(self.manifest)

    def test_firewall_replaces_rules_with_guard_and_restricts_private_egress(self):
        config = pathlib.Path(self.temp.name)/'firewall.json'
        config.write_text(json.dumps({'agents': [{'ip': '172.30.240.10', 'allow': [['192.0.2.10', 443]]}]}))
        calls = []
        def ip(*args, **kwargs):
            calls.append(args)
            return 1 if args[0] == '-C' or args[:1] == ('-D',) and args[-1] == '2' else 0
        with patch.object(firewall, 'ip', ip): firewall.main(str(config))
        for chain in ['CONSTELLATION-OUT', 'CONSTELLATION-HOST']:
            self.assertLess(calls.index(('-I', chain, '1', '-j', 'DROP')), calls.index(('-D', chain, '2')))
            self.assertIn(('-D', chain, '1'), calls)
        self.assertIn(('-A', 'CONSTELLATION-OUT', '-d', '169.254.0.0/16', '-j', 'REJECT'), calls)
        self.assertIn(('-A', 'CONSTELLATION-OUT', '-d', '172.16.0.0/12', '-j', 'REJECT'), calls)
        self.assertIn(('-A', 'CONSTELLATION-HOST', '-j', 'REJECT'), calls)
        self.assertIn(('-I', 'INPUT', '1', '-s', '172.30.240.0/24', '-j', 'CONSTELLATION-HOST'), calls)

    def test_service_stops_when_host_control_pipe_closes(self):
        marker = pathlib.Path(self.temp.name)/'ready'
        child = f"from pathlib import Path; import time; Path({str(marker)!r}).touch(); time.sleep(60)"
        process = subprocess.Popen([sys.executable, str(ROOT/'service.py'), sys.executable, '-c', child], stdin=subprocess.PIPE)
        self.addCleanup(lambda: process.kill() if process.poll() is None else None)
        deadline = time.monotonic() + 5
        while not marker.exists() and time.monotonic() < deadline: time.sleep(.02)
        self.assertTrue(marker.exists())
        process.stdin.close()
        self.assertNotEqual(process.wait(timeout=5), 0)

if __name__ == '__main__': unittest.main()
