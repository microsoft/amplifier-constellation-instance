"""Real subprocess transport with an isolated synthetic Incus guest.

No Incus daemon, system service, production home, provider credential or model is
used. The fixture executes the actual guest program, substituting only systemd
and /proc observations at the synthetic Incus process boundary.
"""
import base64
from concurrent.futures import ThreadPoolExecutor
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from unittest.mock import patch
import uuid

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('incus_provider', ROOT / 'incus_provider.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class IncusProviderTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / 'guest' / 'home'
        self.home.mkdir(parents=True)
        self.binary = self.root / 'guest' / 'bin' / 'agent-instance'
        self.binary.parent.mkdir()
        self.identity = str(uuid.uuid4())
        self.guest_uuid = str(uuid.uuid4())
        self.write('guest-id', self.guest_uuid)
        self.write('guest/home/config.json', {'config': {'instance_id': self.identity}})
        self.write('guest/home/sessions.json', {'original': 'unchanged'})
        self.services = ['anchor-instance.service', 'anchor-slack.service', 'paused.service']
        self.write('services.json', {n: {'active': i < 2, 'pid': 123 + i if i < 2 else 0}
                                     for i, n in enumerate(self.services)})
        self.old = self.binary_source('0.22.2')
        self.new = self.binary_source('0.22.5')
        self.binary.write_bytes(self.old)
        self.binary.chmod(0o751)
        self.write('proc/123/exe', self.old)
        self.service_command = self.script('systemctl', self.systemctl_source())
        self.incus = self.script('incus', self.incus_source())
        self.config = {'schema': 1, 'incus': str(self.incus), 'project': 'default',
                       'guest': 'anchor', 'guest_uuid': self.guest_uuid, 'home': str(self.home), 'binary': str(self.binary),
                       'guest_python': sys.executable, 'instance_id': self.identity,
                       'services': self.services, 'instance_service': self.services[0],
                       'state': str(self.root / 'host' / 'incus')}
        self.config_path = self.root / 'provider.json'
        self.config_path.write_text(json.dumps(self.config))
        self.provider = module.Provider(self.config_path)
        self.canonical = {p.relative_to(self.home).as_posix(): p.read_bytes() for p in self.home.rglob('*') if p.is_file()}

    def write(self, name, value):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(value, bytes):
            path.write_bytes(value)
        else:
            path.write_text(value if isinstance(value, str) else json.dumps(value))
        return path

    def script(self, name, source):
        path = self.write(name, '#!' + sys.executable + '\n' + source)
        path.chmod(0o700)
        return path

    def binary_source(self, version):
        return ('#!' + sys.executable + '\n' + f'''
import base64,json,sys
from pathlib import Path
root=Path({str(self.root)!r});version={version!r};args=sys.argv[1:]
if args==['--version']:print('agent-instance '+version);raise SystemExit
if args and args[0].startswith('export-update-'):
    if args[0]=='export-update-helper':Path(args[2]).write_text('# exported worker\\n')
    else:
        folder=Path(args[2]);(folder/'container').mkdir(exist_ok=True)
        for name in ['engine_provider.py','container_provider.py','incus_provider.py','provision-backend.py','container/install.py','container/bridge.py','container/launch-service.py','container/seccomp.json']:(folder/name).write_text('# exported support\\n')
    raise SystemExit
home=Path(args[1]);method=args[3];path=args[4]
services=json.loads((root/'services.json').read_text())
if not services['anchor-instance.service']['active']:raise SystemExit(1)
identity=json.loads((home/'config.json').read_text())['config']['instance_id']
if (root/'api-id').exists():identity=(root/'api-id').read_text()
if path=='status':print(json.dumps({{'instance_id':identity,'uptime_ms':int((root/'api-uptime').read_text()) if (root/'api-uptime').exists() else 20}}))
elif method=='GET':print(json.dumps({{'policy':{{'revision':0}},'request':None}}))
else:
    body=json.load(sys.stdin);(root/'last-report.json').write_text(json.dumps(body));print(json.dumps({{'policy':{{'revision':0}},'active_prompts':0,'request':None}}))
''').encode()

    def systemctl_source(self):
        return f'''
import json,shutil,sys
from pathlib import Path
root=Path({str(self.root)!r});path=root/'services.json';services=json.loads(path.read_text());args=sys.argv[1:];action=args[0];name=args[1];value=services[name]
if action=='show':print('ActiveState='+('active' if value['active'] else 'inactive'));print('MainPID='+str(value['pid']))
else:
    with (root/'service-log').open('a') as out:out.write(action+' '+name+'\\n')
    value.update(active=action=='start',pid=123 if action=='start' else 0);path.write_text(json.dumps(services))
    if action=='start' and name=='anchor-instance.service':shutil.copyfile(root/'guest/bin/agent-instance',root/'proc/123/exe');(root/'api-uptime').write_text('0')
'''

    def incus_source(self):
        return f'''
import json,subprocess,sys
from pathlib import Path
root=Path({str(self.root)!r});args=sys.argv[1:]
assert args[0]=='--force-local';args=args[1:]
if args==['config','get','--project','default','anchor','volatile.uuid']:print((root/'guest-id').read_text());raise SystemExit
if ('-T' in args or '-t' in args) and any(a.startswith('--mode') for a in args):raise SystemExit(2)
assert args[:6]==['exec','--mode=non-interactive','--project','default','anchor','--']
argv=args[6:];assert argv[:3]==[{sys.executable!r},'-I','-c']
argv[3]=argv[3].replace("SYSTEMCTL = '/bin/systemctl'",'SYSTEMCTL = '+repr(str(root/'systemctl'))).replace("PROC = '/proc'",'PROC = '+repr(str(root/'proc')))
if (root/'short-lock-timeout').exists():argv[3]=argv[3].replace('time.monotonic()+10','time.monotonic()+.1')
data=sys.stdin.buffer.read();request=json.loads(data)
with (root/'transport-log').open('a') as out:out.write(request['action']+'\\n')
failure=json.loads((root/'failure.json').read_text()) if (root/'failure.json').exists() else {{}}
matches=failure.get('action')==request['action'] and failure.get('remaining',0)>0
if matches:
    failure['remaining']-=1;(root/'failure.json').write_text(json.dumps(failure))
    if not failure.get('after'):raise SystemExit(1)
result=subprocess.run(argv,input=data,stdout=sys.stdout.buffer,stderr=sys.stderr.buffer)
if matches and failure.get('after'):raise SystemExit(1)
raise SystemExit(result.returncode)
'''

    def frozen_plan(self):
        before = self.provider.probe()
        identifier = str(uuid.uuid4())
        folder = Path(self.config['state']).parent / 'plans' / identifier
        folder.mkdir(parents=True)
        candidate = folder / 'instance.candidate'
        candidate.write_bytes(self.new)
        value = {'available': True, 'version': 'agent-instance 0.22.5',
                 'config_sha256': self.provider.config_digest, 'guest_uuid': self.guest_uuid,
                 'instance_id': self.identity, 'before': before['binary'],
                 'candidate': {'sha256': hashlib.sha256(self.new).hexdigest()},
                 'candidate_file': str(candidate)}
        wrapper = {'id': identifier, 'component': 'instance', 'plan': value}
        path = folder / 'instance.provider.json'
        path.write_text(json.dumps(wrapper))
        context = patch.dict(os.environ, {'CONSTELLATION_UPDATE_PLAN': str(path)})
        context.start()
        self.addCleanup(context.stop)
        return wrapper

    def unchanged_home(self):
        self.assertEqual(self.canonical, {p.relative_to(self.home).as_posix(): p.read_bytes()
                                         for p in self.home.rglob('*') if p.is_file()})

    def actual_worker(self, wrapper):
        worker=module.load_worker();state=Path(self.config['state']).parent
        prefix=[sys.executable,str(ROOT/'incus_provider.py'),'--config',str(self.config_path),'--action']
        registration={'schema':1,'instance_id':self.identity,'home':str(self.home),'state':str(state),
                      'management':{'protocol':1,'command':prefix+['management']},
                      'components':[{'id':'instance','provider':'command','protocol':2,
                                     'commands':{a:prefix+[a] for a in ['status','plan','apply','rollback','verify']},
                                     'restart_scope':{'services':[],'instance':True,'slack_homes':[]}}],
                      'services':[],'slack_homes':[]}
        path=state/'registration.json';worker.write(path,registration);obj=worker.Worker(path)
        folder=state/'plans'/wrapper['id']
        plan={'id':wrapper['id'],'policy_revision':0,'channel':'stable',
              'components':[{'id':'instance','available':True,'provider':'command',
                             'version':'agent-instance 0.22.2','target_version':'agent-instance 0.22.5',
                             'provider_plan_sha256':worker.digest(folder/'instance.provider.json')}],
              'participants':[],'registration_sha256':obj.registration_digest()}
        plan['restart_scope']=obj.restart_scope(plan);worker.write(folder/'plan.json',plan)
        return obj,plan

    def test_actual_cli_probe_and_management_reuse_guest_api(self):
        command = [sys.executable, str(ROOT / 'incus_provider.py'), '--config', str(self.config_path)]
        actual = json.loads(subprocess.check_output(command + ['--action', 'probe']))
        self.assertEqual(actual['instance_id'], self.identity)
        self.assertEqual(actual['binary']['sha256'], hashlib.sha256(self.old).hexdigest())
        request = {'schema': 1, 'instance_id': self.identity, 'action': 'api', 'path': 'updates/report', 'body': {'status': {'state': 'idle'}}}
        result = json.loads(subprocess.check_output(command + ['--action', 'management'], input=json.dumps(request).encode()))
        self.assertEqual(result['instance_id'], self.identity)
        self.assertEqual(json.loads((self.root / 'last-report.json').read_text()), request['body'])
        self.unchanged_home()

    def test_apply_verify_rollback_preserve_original_state_and_inactive_services(self):
        self.frozen_plan()
        self.assertTrue(self.provider.apply()['applied'])
        self.assertTrue(self.provider.verify()['verified'])
        self.assertEqual(self.binary.read_bytes(), self.new)
        self.assertEqual(self.binary.stat().st_mode & 0o777, 0o751)
        self.assertTrue(self.provider.rollback()['rolled_back'])
        self.assertTrue(self.provider.rollback()['verified'])
        self.assertEqual(self.binary.read_bytes(), self.old)
        log=(self.root/'service-log').read_text().splitlines()
        self.assertEqual(log, ['stop anchor-slack.service', 'stop anchor-instance.service',
                              'start anchor-instance.service', 'start anchor-slack.service'] * 2)
        self.unchanged_home()

    def test_lost_apply_ack_does_not_repeat_effect(self):
        self.frozen_plan(); self.provider.apply()
        first=(self.root/'service-log').read_bytes()
        self.assertTrue(self.provider.apply()['verified'])
        self.assertEqual((self.root/'service-log').read_bytes(), first)

    def test_interrupted_apply_before_and_after_swap_rolls_back(self):
        self.frozen_plan()
        self.write('failure.json', {'action': 'replace', 'after': True, 'remaining': 1})
        with self.assertRaisesRegex(RuntimeError, 'incus_command_failed'):
            self.provider.apply()
        self.assertEqual(self.binary.read_bytes(), self.new)
        with self.assertRaisesRegex(ValueError, 'requires_rollback'):
            self.provider.apply()
        self.assertTrue(self.provider.rollback()['rolled_back'])
        self.assertEqual(self.binary.read_bytes(), self.old)
        self.unchanged_home()

    def test_replacement_failure_before_effect_restores_services(self):
        self.frozen_plan()
        self.write('failure.json', {'action': 'replace', 'after': False, 'remaining': 1})
        with self.assertRaises(RuntimeError): self.provider.apply()
        self.assertEqual(self.binary.read_bytes(), self.old)
        self.assertTrue(self.provider.rollback()['rolled_back'])
        self.unchanged_home()

    def test_changed_guest_refused_before_mutation(self):
        self.frozen_plan(); self.write('guest-id', str(uuid.uuid4()))
        with self.assertRaisesRegex(ValueError, 'identity_changed'): self.provider.apply()
        self.assertFalse((self.root/'service-log').exists())

    def test_wrong_live_identity_refused_before_api_write(self):
        self.write('api-id', str(uuid.uuid4()))
        request={'schema':1,'instance_id':self.identity,'action':'api','path':'updates/report','body':{'status':{}}}
        with self.assertRaises(RuntimeError): self.provider.management(request)
        self.assertFalse((self.root/'last-report.json').exists())

    def test_wrong_config_identity_refused_before_guest_actions(self):
        self.write('guest/home/config.json', {'config': {'instance_id': str(uuid.uuid4())}})
        with self.assertRaises(RuntimeError): self.provider.probe()
        self.assertFalse((self.root/'service-log').exists())

    def test_changed_candidate_config_and_backup_refused(self):
        wrapper=self.frozen_plan(); candidate=Path(wrapper['plan']['candidate_file'])
        candidate.write_bytes(b'tampered')
        with self.assertRaisesRegex(ValueError,'candidate_digest'):self.provider.apply()
        candidate.write_bytes(self.new); self.provider.apply()
        backup=Path(self.config['state']).parent/'plans'/wrapper['id']/'instance.incus.previous';backup.write_bytes(b'tampered')
        with self.assertRaisesRegex(ValueError,'backup_changed'):self.provider.rollback()
        self.config_path.write_text(json.dumps({**self.config,'guest':'other'}))
        with self.assertRaisesRegex(ValueError,'enrollment_changed'):self.provider.verify()

    def test_unexpected_installed_binary_prevents_rollback(self):
        self.frozen_plan(); self.provider.apply(); self.binary.write_bytes(b'unknown')
        with self.assertRaisesRegex(ValueError,'installed_binary_changed'):self.provider.rollback()
        self.assertEqual(self.binary.read_bytes(),b'unknown')

    def test_management_does_not_accept_unenrolled_methods_resources_or_fields(self):
        base={'schema':1,'instance_id':self.identity,'action':'api','path':'updates','body':None}
        for change in ({'path':'config'},{'path':'updates','body':{}},{'path':'status','body':{}},
                       {'path':'https://example.com'},{'extra':'command'},{'instance_id':str(uuid.uuid4())}):
            with self.subTest(change=change),self.assertRaises(ValueError):self.provider.management({**base,**change})
        self.assertFalse((self.root/'transport-log').exists())

    def test_export_old_version_preserves_newer_host_helper(self):
        result=self.provider.management({'schema':1,'instance_id':self.identity,'action':'export'})['result']
        self.assertFalse(result['supported']);self.assertEqual(result['files'],{})
        self.assertEqual(result['binary_sha256'],hashlib.sha256(self.old).hexdigest())

    def test_export_current_binary_embedded_support_and_cleans_staging(self):
        self.binary.write_bytes(self.new)
        result=self.provider.management({'schema':1,'instance_id':self.identity,'action':'export'})['result']
        self.assertTrue(result['supported']);self.assertEqual(len(result['files']),9)
        self.assertEqual(base64.b64decode(result['files']['worker.py']),b'# exported worker\n')
        self.assertEqual(sorted(p.name for p in self.binary.parent.iterdir()),['agent-instance'])
        self.unchanged_home()

    def test_known_binary_export_returns_metadata_without_reexport(self):
        self.binary.write_bytes(self.new)
        result=self.provider.management({'schema':1,'instance_id':self.identity,'action':'export',
                                         'known_sha256':hashlib.sha256(self.new).hexdigest()})['result']
        self.assertTrue(result['supported']);self.assertEqual(result['files'],{})
        self.assertEqual(result['binary_sha256'],hashlib.sha256(self.new).hexdigest())
        with self.assertRaisesRegex(ValueError,'invalid_known_binary_digest'):
            self.provider.management({'schema':1,'instance_id':self.identity,'action':'export','known_sha256':'bad'})

    def test_same_name_cloned_guest_refused_before_management(self):
        self.write('guest-id',str(uuid.uuid4()))
        with self.assertRaisesRegex(ValueError,'incus_identity_changed'):
            self.provider.management({'schema':1,'instance_id':self.identity,'action':'api','path':'updates/report','body':{}})
        self.assertFalse((self.root/'transport-log').exists())

    def test_existing_candidate_logic_stages_and_inspects_only_in_guest(self):
        worker=module.load_worker();platform=self.provider.probe()['platform'];archive=io.BytesIO()
        with tarfile.open(fileobj=archive,mode='w:gz') as tar:
            info=tarfile.TarInfo('agent-instance');info.size=len(self.new);tar.addfile(info,io.BytesIO(self.new))
        data=archive.getvalue();sha='sha256:'+hashlib.sha256(data).hexdigest()
        release={'tag_name':'v0.22.5','html_url':'https://github.com/microsoft/amplifier-constellation-instance/releases/tag/v0.22.5',
                 'assets':[{'name':'agent-instance-'+platform[2]+'.tar.gz','browser_download_url':'https://github.com/microsoft/fixture/archive','digest':sha}]}
        def download(url,destination,expected):
            self.assertEqual(expected,sha);Path(destination).write_bytes(data)
        folder=Path(self.config['state']).parent/'plans'/str(uuid.uuid4());folder.mkdir(parents=True)
        with patch.dict(os.environ,{'CONSTELLATION_UPDATE_PLAN':str(folder/'instance.provider.json')}),patch.object(module,'load_worker',return_value=worker),patch.object(worker,'gh',return_value=release),patch.object(worker,'download',side_effect=download),patch.object(worker,'run',side_effect=AssertionError('host binary execution forbidden')):
            plan=self.provider.plan()
        self.assertTrue(plan['available']);self.assertEqual(plan['version'],'agent-instance 0.22.5')
        self.assertEqual(Path(plan['candidate_file']).read_bytes(),self.new)
        self.assertEqual(self.binary.read_bytes(),self.old)

    def test_failed_before_effect_rollback_and_verify_remain_truthfully_unchanged(self):
        wrapper=self.frozen_plan();Path(wrapper['plan']['candidate_file']).write_bytes(b'bad')
        with self.assertRaisesRegex(ValueError,'candidate_digest_mismatch'):self.provider.apply()
        self.assertTrue(self.provider.rollback()['untouched'])
        self.assertTrue(self.provider.verify()['verified'])
        self.assertFalse((self.root/'service-log').exists());self.unchanged_home()

    def test_missing_candidate_does_not_block_verified_backup_rollback(self):
        wrapper=self.frozen_plan();self.provider.apply();Path(wrapper['plan']['candidate_file']).unlink()
        self.assertTrue(self.provider.rollback()['rolled_back'])
        self.assertEqual(self.binary.read_bytes(),self.old)

    def test_plan_must_use_canonical_helper_retention_directory(self):
        wrong=self.root/'other'/str(uuid.uuid4())/'instance.provider.json';wrong.parent.mkdir(parents=True)
        with patch.dict(os.environ,{'CONSTELLATION_UPDATE_PLAN':str(wrong)}),self.assertRaisesRegex(ValueError,'invalid_provider_plan_path'):
            self.provider.plan()

    def test_actual_worker_provider_apply_and_explicit_rollback(self):
        wrapper=self.frozen_plan();obj,plan=self.actual_worker(wrapper)
        obj.apply(plan,{'revision':0})
        self.assertEqual(obj.state['state'],'succeeded');self.assertEqual(self.binary.read_bytes(),self.new)
        obj.apply(plan,{'revision':0},rollback=True)
        self.assertEqual(obj.state['state'],'rolled_back');self.assertEqual(self.binary.read_bytes(),self.old)
        self.unchanged_home()

    def test_actual_worker_pre_effect_failure_does_not_strand_old_process(self):
        wrapper=self.frozen_plan();obj,plan=self.actual_worker(wrapper)
        Path(wrapper['plan']['candidate_file']).write_bytes(b'changed')
        self.write('api-uptime','200000')
        with self.assertRaises(RuntimeError):obj.apply(plan,{'revision':0})
        self.assertEqual(obj.state['state'],'rolled_back');self.assertEqual(self.binary.read_bytes(),self.old)
        self.assertFalse((self.root/'service-log').exists());self.unchanged_home()

    def test_unsettled_guest_operation_prevents_overlapping_rollback(self):
        self.frozen_plan();self.provider.apply();self.write('short-lock-timeout','yes')
        lock=self.binary.parent/'.constellation-update.lock'
        source="import fcntl,sys;f=open(sys.argv[1],'a+b');fcntl.flock(f,fcntl.LOCK_EX);print('ready',flush=True);sys.stdin.read()"
        process=subprocess.Popen([sys.executable,'-c',source,str(lock)],stdin=subprocess.PIPE,stdout=subprocess.PIPE)
        try:
            self.assertEqual(process.stdout.readline(),b'ready\n')
            prior=(self.root/'service-log').read_bytes()
            with self.assertRaises(RuntimeError):self.provider.rollback()
            self.assertEqual((self.root/'service-log').read_bytes(),prior)
            self.assertEqual(self.binary.read_bytes(),self.new)
        finally:
            process.communicate(timeout=5)
        self.assertTrue(self.provider.rollback()['rolled_back'])

    def test_pending_rename_settles_while_rollback_waits_for_guest_lock(self):
        wrapper=self.frozen_plan()
        self.write('failure.json',{'action':'replace','after':False,'remaining':1})
        with self.assertRaises(RuntimeError):self.provider.apply()
        lock=self.binary.parent/'.constellation-update.lock'
        source="""import fcntl,os,shutil,stat,sys
from pathlib import Path
lock=Path(sys.argv[1]);binary=Path(sys.argv[2]);candidate=Path(sys.argv[3])
with lock.open('a+b') as f:
 fcntl.flock(f,fcntl.LOCK_EX);print('ready',flush=True);sys.stdin.read()
 old=binary.stat();pending=binary.with_name('.pending-rename');shutil.copyfile(candidate,pending);os.chmod(pending,stat.S_IMODE(old.st_mode));os.chown(pending,old.st_uid,old.st_gid);os.replace(pending,binary)
"""
        process=subprocess.Popen([sys.executable,'-c',source,str(lock),str(self.binary),wrapper['plan']['candidate_file']],stdin=subprocess.PIPE,stdout=subprocess.PIPE)
        baseline=(self.root/'transport-log').read_text().splitlines().count('service')
        try:
            self.assertEqual(process.stdout.readline(),b'ready\n')
            with ThreadPoolExecutor(max_workers=1) as pool,patch.object(module,'HEALTH_TIMEOUT',0):
                future=pool.submit(self.provider.rollback)
                deadline=time.monotonic()+5
                while (self.root/'transport-log').read_text().splitlines().count('service')==baseline:
                    self.assertLess(time.monotonic(),deadline,'rollback did not reach the held guest lock')
                    time.sleep(.02)
                process.communicate(timeout=5)
                self.assertTrue(future.result(timeout=10)['rolled_back'])
        finally:
            if process.poll() is None:process.communicate(timeout=5)
        self.assertEqual(self.binary.read_bytes(),self.old)
        self.assertTrue(self.provider.verify()['verified'])

    def test_candidate_changed_during_service_stop_never_reaches_guest_binary(self):
        wrapper=self.frozen_plan();candidate=Path(wrapper['plan']['candidate_file']);original=self.provider.services
        def services(active,operation):
            original(active,operation)
            if operation=='stop':candidate.write_bytes(self.binary_source('0.99.0'))
        with patch.object(self.provider,'services',side_effect=services),patch.object(module,'HEALTH_TIMEOUT',0):
            with self.assertRaisesRegex(ValueError,'replacement_source_digest_mismatch'):self.provider.apply()
        self.assertEqual(self.binary.read_bytes(),self.old)
        self.assertNotIn('replace',(self.root/'transport-log').read_text().splitlines())
        self.assertTrue(self.provider.rollback()['rolled_back'])

    def test_backup_and_intent_directory_entries_durable_before_guest_effect(self):
        wrapper=self.frozen_plan();parent=Path(wrapper['plan']['candidate_file']).parent
        synced=[];original_fsync=os.fsync;original_guest=self.provider.guest;original_write=module.write
        def fsync(fd):
            if stat.S_ISDIR(os.fstat(fd).st_mode):synced.append(Path(os.readlink('/proc/self/fd/'+str(fd))))
            return original_fsync(fd)
        def write(path,value):
            if value.get('state')=='applying':self.assertIn(parent,synced,'backup directory entry was not synced before intent')
            return original_write(path,value)
        def guest(action,**values):
            if action=='service':self.assertGreaterEqual(synced.count(parent),2,'backup and intent names must both be durable before service effects')
            return original_guest(action,**values)
        with patch.object(module.os,'fsync',side_effect=fsync),patch.object(module,'write',side_effect=write),patch.object(self.provider,'guest',side_effect=guest):
            self.assertTrue(self.provider.apply()['applied'])

    def test_running_binary_must_match_installed_binary(self):
        self.frozen_plan();self.provider.apply();self.write('proc/123/exe',self.old)
        with patch.object(module,'HEALTH_TIMEOUT',0),self.assertRaises(RuntimeError):self.provider.verify()

    def test_candidate_version_inspected_in_guest_disposable_directory(self):
        result=self.provider.guest('inspect',data=base64.b64encode(self.new).decode(),sha256=hashlib.sha256(self.new).hexdigest())
        self.assertEqual(result['version'],'agent-instance 0.22.5')
        self.assertEqual(self.binary.read_bytes(),self.old)
        self.assertEqual(sorted(p.name for p in self.binary.parent.iterdir()),['agent-instance'])

    def test_no_arbitrary_remote_names_or_agent_owned_binary(self):
        for change in ({'guest':'remote:anchor'},{'project':'--project=other'},
                       {'binary':str(self.home/'binary')},{'services':['evil;command.service']},
                       {'state':str(self.home/'helper')}):
            self.config_path.write_text(json.dumps({**self.config,**change}))
            with self.subTest(change=change),self.assertRaises(ValueError):module.Provider(self.config_path)


if __name__ == '__main__':
    unittest.main()
