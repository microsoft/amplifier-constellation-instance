"""Owner-fixed management transport and helper publication, without a live guest.

The command boundary is an actual child process. Incus, credentials, services,
provider networks and model calls are deliberately outside this fixture.
"""
import base64
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import uuid
import zipfile
from unittest.mock import patch

WORKER = Path(os.environ.get('CONSTELLATION_UPDATE_WORKER', Path(__file__).with_name('worker.py')))
spec = importlib.util.spec_from_file_location('management_worker', WORKER)
w = importlib.util.module_from_spec(spec)
spec.loader.exec_module(w)

COMMAND = r'''
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
config = json.loads((root / 'command-config.json').read_text())
def log(value):
    with (root / 'calls.jsonl').open('a') as out:
        out.write(json.dumps(value) + '\n')
if sys.argv[2] == 'provider':
    action = sys.argv[3]
    log({'kind': 'provider', 'action': action})
    if action == 'rollback':
        config['mode'] = 'ok'
        (root / 'command-config.json').write_text(json.dumps(config))
    print('{}')
    sys.exit(0)
request = json.load(sys.stdin)
log({'kind': 'management', 'argv': sys.argv[2:], 'request': request})
mode = config.get('mode', 'ok')
if mode == 'error':
    print('guest-secret-canary', file=sys.stderr)
    sys.exit(3)
if mode == 'malformed':
    print('not JSON')
    sys.exit(0)
if mode == 'oversize':
    sys.stdout.write('x' * (8 * 1024 * 1024 + 1))
    sys.exit(0)
identity = config['identity']
if request['action'] == 'export':
    result = config['export']
elif request['path'] == 'status':
    result = {'instance_id': identity, 'uptime_ms': 0}
    if mode == 'wrong-status-identity':
        result['instance_id'] = 'different-instance'
else:
    result = {'policy': {'revision': 0, 'agent_initiated': True,
        'check_minutes': 0, 'channel': 'stable', 'install': 'on_request'},
        'active_prompts': 0}
value = {'schema': 1, 'instance_id': identity, 'result': result}
if mode == 'wrong-identity': value['instance_id'] = 'different-instance'
if mode == 'wrong-schema': value['schema'] = 2
if mode == 'extra-envelope-field': value['secret'] = 'must not be accepted'
if mode == 'non-object': value = []
if mode == 'non-object-result': value['result'] = []
print(json.dumps(value))
'''


class ManagementTransportTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.state = self.root / 'owner'
        self.state.mkdir()
        self.home = self.root / 'guest-only-home-does-not-exist'
        self.command = self.root / 'fixed-command.py'
        self.command.write_text(COMMAND)
        self.identity = str(uuid.uuid4())
        self.command_config = {'identity': self.identity, 'mode': 'ok'}
        self.set_command()
        self.registration = {
            'schema': 1, 'instance_id': self.identity, 'home': str(self.home),
            'state': str(self.state), 'services': [], 'slack_homes': [],
            'management': {'protocol': 1, 'command': [sys.executable,
                str(self.command), str(self.root), 'management', 'fixed ; argument']},
            'components': [{'id': 'instance', 'provider': 'command', 'protocol': 2,
                'commands': {a: [sys.executable, str(self.command), str(self.root),
                    'provider', a] for a in ('status', 'plan', 'apply', 'verify', 'rollback')}}],
        }
        self.path = self.state / 'registration.json'
        w.write(self.path, self.registration)

    def set_command(self, **values):
        self.command_config.update(values)
        (self.root / 'command-config.json').write_text(json.dumps(self.command_config))

    def calls(self):
        path = self.root / 'calls.jsonl'
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def worker(self):
        # No host-readable guest home, config, credential, CA, or binary exists.
        with patch.object(w.ssl, 'create_default_context', side_effect=AssertionError('host CA read')):
            return w.Worker(self.path)

    def test_guest_management_needs_no_host_home_or_credentials_and_argv_is_fixed(self):
        obj = self.worker()
        self.assertEqual(self.calls(), [])
        self.assertEqual(obj.api('status')['instance_id'], self.identity)
        body = {'status': {'message': 'quotes " ; $(touch nope)\n../elsewhere',
                           'request_id': 'not a command argument'}, 'drain_seconds': 0}
        obj.api('updates/report', body)
        calls = self.calls()
        self.assertEqual([c['argv'] for c in calls], [['management', 'fixed ; argument']] * 2)
        self.assertEqual(calls[1]['request'], {'schema': 1, 'instance_id': self.identity,
            'action': 'api', 'path': 'updates/report', 'body': body})
        self.assertFalse(self.home.exists())
        self.assertEqual(set(self.root.iterdir()), {self.state, self.command,
            self.root / 'command-config.json', self.root / 'calls.jsonl'})

    def test_bad_transport_results_never_become_success(self):
        obj = self.worker()
        for mode in ('error', 'malformed', 'oversize', 'wrong-identity', 'wrong-status-identity',
                     'wrong-schema', 'extra-envelope-field', 'non-object', 'non-object-result'):
            with self.subTest(mode=mode):
                self.set_command(mode=mode)
                with self.assertRaises(ValueError) as caught:
                    obj.api('status')
                self.assertNotIn('guest-secret-canary', str(caught.exception))
        self.assertEqual(len(self.calls()), 9)

    def test_timeout_is_not_retried_or_acknowledged(self):
        obj = self.worker()
        with patch.object(w.subprocess, 'run', side_effect=subprocess.TimeoutExpired('fixed', 60)) as call:
            with self.assertRaises(subprocess.TimeoutExpired):
                obj.api('updates/report', {'finished_request': 'original'})
        self.assertEqual(call.call_count, 1)
        self.assertNotIn('completed_requests', obj.state)

    def test_only_existing_update_api_shapes_reach_transport(self):
        obj = self.worker()
        for path, body in [('config', None), ('../status', None), ('https://elsewhere/status', None),
                           ('updates?method=DELETE', None), ('updates/report', None),
                           ('status', {}), ('updates', {}), ('service-connections', {})]:
            with self.subTest(path=path, body=body), self.assertRaisesRegex(ValueError, 'unsupported_update_management_request'):
                obj.api(path, body)
        self.assertEqual(self.calls(), [])
        with self.assertRaisesRegex(ValueError, 'management_request_too_large'):
            obj.api('updates/report', {'message': 'x' * (2 * 1024 * 1024)})
        self.assertEqual(self.calls(), [])

    def test_registration_rejects_ambiguous_transport_or_host_component_paths(self):
        bad = [None, {}, {'protocol': 2, 'command': [sys.executable]},
               {'protocol': 1, 'command': 'shell text'},
               {'protocol': 1, 'command': ['relative-command']},
               {'protocol': 1, 'command': [sys.executable, 'nul\x00argument']},
               {'protocol': 1, 'command': [sys.executable], 'url': 'https://elsewhere'}]
        for transport in bad:
            with self.subTest(transport=transport):
                w.write(self.path, {**self.registration, 'management': transport})
                with self.assertRaisesRegex(ValueError, 'fixed_management_transport_required'):
                    w.load_registration(self.path)
        for change in ({'slack_homes': ['/guest/slack']}, {'components': [
                {'id': 'instance', 'kind': 'instance', 'binary': '/guest/bin/agent-instance'}]}):
            w.write(self.path, {**self.registration, **change})
            with self.assertRaisesRegex(ValueError, 'remote_components_require_owner_providers'):
                w.load_registration(self.path)
        self.assertEqual(self.calls(), [])

    def test_api_down_does_not_prevent_constructor_or_provider_recovery(self):
        self.set_command(mode='error')
        plan_id = str(uuid.uuid4())
        folder = self.state / 'plans' / plan_id
        provider = folder / 'instance.provider.json'
        w.write(provider, {'id': plan_id, 'component': 'instance', 'plan': {'frozen': 'target'}})
        plan = {'id': plan_id, 'components': [{'id': 'instance', 'available': True,
            'provider': 'command', 'version': 'old', 'target_version': 'new',
            'provider_plan_sha256': w.digest(provider)}], 'provider_attempts': ['instance'],
            'previous': {}, 'restart_scope': {'services': [], 'instance': True, 'slack_homes': []}}
        w.write(folder / 'plan.json', plan)
        retained = {'state': 'swapping', 'plan_id': plan_id, 'last_check': 0,
            'unknown_receipts': [{'id': 'retained', 'settled': False}], 'work': {'spent': 8}}
        w.write(self.state / 'state.json', retained)
        obj = self.worker()
        self.assertEqual(self.calls(), [])
        obj.tick()
        self.assertEqual(self.calls()[0], {'kind': 'provider', 'action': 'rollback'})
        self.assertEqual(obj.state['state'], 'rolled_back')
        self.assertEqual(obj.state['unknown_receipts'], retained['unknown_receipts'])
        self.assertEqual(obj.state['work'], retained['work'])

    def test_local_management_still_uses_own_tls_and_control_token(self):
        self.home.mkdir()
        (self.home / 'secrets').mkdir()
        (self.home / 'secrets/control-token').write_text('local-only-token')
        w.write(self.home / 'config.json', {'config': {'server': {'bind': '::', 'port': 19400}}})
        registration = {k: v for k, v in self.registration.items() if k != 'management'}
        w.write(self.path, registration)
        with patch.object(w.ssl, 'create_default_context') as context, patch.object(w, 'http_json', return_value={'ok': True}) as http:
            obj = w.Worker(self.path)
            self.assertEqual(obj.api('status'), {'ok': True})
        context.assert_called_once_with(cafile=str(self.home / 'tls/root-ca.crt'))
        http.assert_called_once_with('https://[::1]:19400/api/v1/status', None,
            {'Authorization': 'Bearer local-only-token'}, context.return_value)
        self.assertEqual(self.calls(), [])

    def export_files(self, generation):
        return {name: base64.b64encode((('GENERATION = ' + repr(generation) + '\n')
                if name.endswith('.py') else json.dumps({'generation': generation})).encode()).decode()
                for name in sorted(w.HELPER_FILES)}

    def seed_helper_files(self, generation):
        for name, content in self.export_files(generation).items():
            path = self.state / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(base64.b64decode(content))

    def test_coherent_helper_refresh_preserves_registration_and_work(self):
        self.seed_helper_files('old')
        files = self.export_files('new')
        self.set_command(export={'supported': True, 'binary_sha256': 'a' * 64, 'files': files})
        retained = {'state': 'succeeded', 'completed_requests': ['original'],
                    'unknown_receipts': [{'settled': False}], 'work': {'id': 'same', 'spent': 8}}
        w.write(self.state / 'state.json', retained)
        before = self.path.read_bytes()
        obj = self.worker()
        with patch.object(w.os, 'execv') as execute:
            obj.refresh_helper()
        execute.assert_called_once_with(sys.executable, [sys.executable, '-I', '-X', 'utf8',
            str(self.state / 'worker.py'), '--registration', str(self.path)])
        self.assertEqual(self.path.read_bytes(), before)
        state = w.read(self.state / 'state.json')
        for key, value in retained.items(): self.assertEqual(state[key], value)
        self.assertNotIn('helper_refresh', state)
        self.assertEqual(state['helper_instance_sha256'], 'a' * 64)
        for name, content in files.items(): self.assertEqual((self.state / name).read_bytes(), base64.b64decode(content))
        with patch.object(w.os, 'execv') as execute:
            obj.refresh_helper()
        execute.assert_not_called()

    def test_bad_helper_export_leaves_working_bootstrap_and_journal_usable(self):
        self.seed_helper_files('bootstrap')
        obj=self.worker()
        obj.save(state='succeeded',completed_requests=['original'])
        before=obj.file.read_bytes()
        bad_values=['not base64!',base64.b64encode(b'invalid python !').decode(),
                    base64.b64encode(b'x'*(2*1024*1024+1)).decode()]
        for content in bad_values:
            files=self.export_files('new');files['worker.py']=content
            self.set_command(export={'supported':True,'binary_sha256':'a'*64,'files':files})
            with self.assertRaises((ValueError,SyntaxError)),patch.object(w.os,'execv') as execute:
                obj.refresh_helper()
            execute.assert_not_called()
            self.assertEqual(obj.file.read_bytes(),before)
            self.assertTrue(w.enrollment_idle(obj.state,self.state))
            self.assertIn(b'bootstrap',(self.state/'worker.py').read_bytes())

    def test_failed_exec_exits_instead_of_serving_with_mixed_helper_generations(self):
        self.seed_helper_files('old')
        files=self.export_files('new')
        self.set_command(export={'supported':True,'binary_sha256':'d'*64,'files':files})
        obj=self.worker()
        with patch.object(w.os,'execv',side_effect=OSError('cannot exec')),self.assertRaisesRegex(SystemExit,'restart_required'):
            obj.refresh_helper()
        self.assertNotIn('helper_refresh',w.read(obj.file))
        self.assertIn(b'new',(self.state/'worker.py').read_bytes())

    def test_interrupted_export_finishes_original_intent_before_any_new_guest_call(self):
        self.seed_helper_files('old')
        files = self.export_files('new')
        self.set_command(export={'supported': True, 'binary_sha256': 'b' * 64, 'files': files})
        retained = {'state': 'succeeded', 'completed_requests': ['done'], 'work': {'spent': 8}}
        w.write(self.state / 'state.json', retained)
        registration = self.path.read_bytes()
        obj = self.worker()
        replace = w.os.replace
        publications = []
        def interrupted(source, destination):
            destination = Path(destination)
            if destination != obj.file:
                publications.append(destination)
                if len(publications) == 2: raise OSError('simulated helper publication interruption')
            return replace(source, destination)
        with patch.object(w.os, 'replace', side_effect=interrupted), patch.object(w.os, 'execv') as execute:
            with self.assertRaisesRegex(OSError, 'publication interruption'): obj.refresh_helper()
        execute.assert_not_called()
        self.assertIn(b"'old'", (self.state / 'worker.py').read_bytes())
        self.assertEqual(w.read(obj.file)['helper_refresh']['files'], files)
        before_calls = self.calls()
        self.set_command(mode='error')
        with patch.object(w.os, 'execv') as execute:
            recovered = self.worker()
        self.assertEqual(self.calls(), before_calls)
        execute.assert_called_once()
        self.assertNotIn('helper_refresh', recovered.state)
        self.assertEqual(self.path.read_bytes(), registration)
        for key, value in retained.items(): self.assertEqual(recovered.state[key], value)
        for name, content in files.items(): self.assertEqual((self.state / name).read_bytes(), base64.b64decode(content))

    def test_same_process_cannot_process_work_after_partial_helper_publication(self):
        self.seed_helper_files('old')
        files=self.export_files('new')
        self.set_command(export={'supported':True,'binary_sha256':'e'*64,'files':files})
        obj=self.worker();replace=w.os.replace;published=[]
        def fail_midway(source,destination):
            if Path(destination)!=obj.file:
                published.append(destination)
                if len(published)==2:raise OSError('partial publication')
            return replace(source,destination)
        with patch.object(w.os,'replace',side_effect=fail_midway),self.assertRaises(OSError):
            obj.refresh_helper()
        calls=self.calls()
        with patch.object(w.os,'execv') as execute:
            obj.tick()
        execute.assert_called_once()
        self.assertEqual(self.calls(),calls)
        self.assertNotIn('helper_refresh',obj.state)

    def test_older_guest_without_transport_export_keeps_bootstrap_helper(self):
        self.seed_helper_files('bootstrap')
        before = {name: (self.state / name).read_bytes() for name in w.HELPER_FILES}
        self.set_command(export={'supported': False, 'binary_sha256': 'c' * 64})
        obj = self.worker()
        with patch.object(w.os, 'execv') as execute:
            obj.refresh_helper()
        execute.assert_not_called()
        self.assertEqual({name: (self.state / name).read_bytes() for name in w.HELPER_FILES}, before)
        self.assertEqual(obj.state['helper_instance_sha256'], 'c' * 64)

    def test_bad_export_validates_all_files_before_publishing_any(self):
        self.seed_helper_files('old')
        old = {name: (self.state / name).read_bytes() for name in w.HELPER_FILES}
        for mode in ('missing-file', 'extra-path', 'invalid-python', 'invalid-base64'):
            files = self.export_files('new')
            if mode == 'missing-file': files.pop('worker.py')
            if mode == 'extra-path': files['../outside.py'] = files['worker.py']
            if mode == 'invalid-python': files['worker.py'] = base64.b64encode(b'def broken(').decode()
            if mode == 'invalid-base64': files['worker.py'] = '%%not-base64%%'
            with self.subTest(mode=mode), self.assertRaises((ValueError, SyntaxError)):
                w.publish_helper_files(self.state, files)
            self.assertEqual({name: (self.state / name).read_bytes() for name in w.HELPER_FILES}, old)

    def test_candidate_uses_guest_platform_and_remote_inspection_only(self):
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, 'w') as package: package.writestr('agent-instance.exe', b'guest candidate')
        url = 'https://github.com/microsoft/amplifier-constellation-instance/releases/download/v0.22.5/windows'
        release = {'tag_name': 'v0.22.5', 'html_url': 'https://github.com/microsoft/amplifier-constellation-instance/releases/tag/v0.22.5',
                   'assets': [{'name': 'agent-instance-aarch64-pc-windows-msvc.tar.gz',
                               'browser_download_url': url, 'digest': 'sha256:' + 'd' * 64}]}
        observed = {'platform': ['Windows', 'aarch64', 'aarch64-pc-windows-msvc'],
                    'version': 'agent-instance 0.22.4', 'sha256': 'a' * 64}
        component = {'id': 'instance', 'kind': 'instance', 'binary': '/guest/not-a-host-binary'}
        inspected = []
        def inspect(path):
            inspected.append(path)
            self.assertEqual(path.read_bytes(), b'guest candidate')
            return 'agent-instance 0.22.5'
        def download(selected, destination, digest):
            self.assertEqual((selected, digest), (url, 'sha256:' + 'd' * 64))
            destination.write_bytes(archive.getvalue())
        with patch.object(w, 'platform_names', side_effect=AssertionError('host platform used')), \
             patch.object(w, 'run', side_effect=AssertionError('guest binary executed on host')), \
             patch.object(w, 'gh', return_value=release), patch.object(w, 'download', side_effect=download):
            result = w.candidate(component, 'stable', self.state, observed=observed, inspect_candidate=inspect)
        self.assertEqual(inspected, [self.state / 'instance.candidate'])
        self.assertEqual(result['target_version'], 'agent-instance 0.22.5')
        self.assertEqual(result['installed_sha256'], observed['sha256'])
        self.assertEqual(result['sha256'], w.digest(inspected[0]))
        with patch.object(w, 'gh') as fetch:
            with self.assertRaisesRegex(ValueError, 'remote_candidate_inspection_required'):
                w.candidate(component, 'stable', self.state, observed=observed)
        fetch.assert_not_called()


if __name__ == '__main__':
    unittest.main()
