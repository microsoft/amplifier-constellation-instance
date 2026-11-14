#!/usr/bin/env python3
"""An existing Incus installation, managed by its independent host updater.

Only owner-enrolled paths, services and guest identity enter command arguments.
The existing Instance API handles authentication inside the guest. No credential,
session, configuration backup, container replacement or model invocation occurs.
"""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import uuid

MAX_BINARY = 256 * 1024 * 1024
MAX_EXPORT = 8 * 1024 * 1024
HEALTH_TIMEOUT = 45
API_PATHS = {'status': False, 'updates': False, 'service-connections': False,
             'updates/report': True, 'service-connections/report': True}

# This is fixed source, not caller-supplied code. Values arrive as JSON on stdin.
# Guest file writes are limited to disposable staging and the registered binary.
GUEST = r'''
import base64, fcntl, hashlib, json, os, platform, stat, subprocess, sys, tempfile, time
from pathlib import Path
SYSTEMCTL = '/bin/systemctl'
PROC = '/proc'
MAX_BINARY = 256 * 1024 * 1024
p=json.load(sys.stdin); c=p['config']; binary=Path(c['binary']); home=Path(c['home'])
config=json.loads((home/'config.json').read_text())['config']
if config['instance_id']!=c['instance_id']:raise ValueError('instance_identity_mismatch')
if binary.resolve().is_relative_to(home.resolve()):raise ValueError('binary_must_be_outside_agent_home')
# A lost Incus reply does not prove the guest operation exited. Serialize any
# subsequent service or binary mutation with that still-running guest process.
# Failure to acquire is unresolved, never permission to replay its mutation.
operation_lock=None
if p['action'] in ('service','replace'):
    lock_path=binary.parent/'.constellation-update.lock'
    if lock_path.is_symlink():raise ValueError('guest_lock_symlink')
    operation_lock=lock_path.open('a+b');deadline=time.monotonic()+10
    while True:
        try:fcntl.flock(operation_lock,fcntl.LOCK_EX|fcntl.LOCK_NB);break
        except BlockingIOError:
            if time.monotonic()>=deadline:raise ValueError('guest_operation_unsettled')
            time.sleep(.05)
def digest(path):
    result=hashlib.sha256()
    with Path(path).open('rb') as stream:
        while block:=stream.read(1024*1024):result.update(block)
    return result.hexdigest()
def ordinary():
    s=binary.lstat()
    if not stat.S_ISREG(s.st_mode) or binary.resolve()!=binary or not 0<s.st_size<=MAX_BINARY:raise ValueError('ordinary_binary_required')
    return {'sha256':digest(binary),'size':s.st_size,'mode':stat.S_IMODE(s.st_mode),'uid':s.st_uid,'gid':s.st_gid}
def call(argv,data=None):
    r=subprocess.run(argv,input=data,capture_output=True,timeout=60)
    if r.returncode:raise RuntimeError('guest_command_failed')
    if len(r.stdout)>8*1024*1024:raise ValueError('guest_response_limit')
    return r.stdout
def api(path,body=None):
    argv=[str(binary),'--home',str(home),'api','GET' if body is None else 'POST',path]
    if body is not None:argv+=['--file','/dev/stdin']
    return json.loads(call(argv,None if body is None else json.dumps(body).encode()))
def live():
    value=api('status')
    if value.get('instance_id')!=c['instance_id']:raise ValueError('live_instance_identity_mismatch')
    return value
def service(name):
    if name not in c['services']:raise ValueError('unenrolled_service')
    raw=call([SYSTEMCTL,'show',name,'--property=ActiveState','--property=MainPID']).decode()
    values=dict(line.split('=',1) for line in raw.splitlines() if '=' in line)
    if values.get('ActiveState') not in ('active','inactive','failed'):raise ValueError('service_transition_in_progress')
    return {'active':values['ActiveState']=='active','pid':int(values.get('MainPID','0'))}
action=p['action']; result=None
if action=='probe':
    meta=ordinary(); value=live(); version=call([str(binary),'--version']).decode().strip()
    arch={'arm64':'aarch64','aarch64':'aarch64','x86_64':'x86_64','amd64':'x86_64'}[platform.machine().lower()]
    if platform.system()!='Linux':raise ValueError('linux_guest_required')
    result={'instance_id':c['instance_id'],'version':version,'binary':meta,'platform':['Linux',arch,arch+'-unknown-linux-gnu'],'services':{n:service(n) for n in c['services']}}
elif action=='metadata':result=ordinary()
elif action=='api':
    live(); result=api(p['path'],p['body'])
elif action=='read_binary':
    before=ordinary()
    if before!=p['expected']:raise ValueError('binary_changed_before_backup')
    data=binary.read_bytes()
    if hashlib.sha256(data).hexdigest()!=before['sha256'] or ordinary()!=before:raise ValueError('binary_changed_during_backup')
    result={'metadata':before,'data':base64.b64encode(data).decode()}
elif action=='inspect':
    live(); data=base64.b64decode(p['data'],validate=True)
    if not 0<len(data)<=MAX_BINARY or hashlib.sha256(data).hexdigest()!=p['sha256']:raise ValueError('candidate_digest_mismatch')
    with tempfile.TemporaryDirectory(prefix='.instance-inspect-',dir=binary.parent) as root:
        path=Path(root)/'agent-instance';path.write_bytes(data);path.chmod(0o700)
        result={'version':call([str(path),'--version']).decode().strip()}
elif action=='service':
    name=p['name']; previous=service(name); requested=p['operation']
    if requested not in ('start','stop','status'):raise ValueError('unsupported_service_operation')
    if requested=='stop' and previous['active']:call([SYSTEMCTL,'stop',name])
    if requested=='start' and not previous['active']:call([SYSTEMCTL,'start',name])
    result=service(name)
    if requested!='status' and result['active']!=(requested=='start'):raise ValueError('service_operation_unsettled')
elif action=='replace':
    current=ordinary(); expected=p['expected']; data=base64.b64decode(p['data'],validate=True)
    if current!=expected:raise ValueError('binary_changed_before_replace')
    if not 0<len(data)<=MAX_BINARY or hashlib.sha256(data).hexdigest()!=p['sha256']:raise ValueError('replacement_digest_mismatch')
    for name in c['services']:
        if service(name)['active']:raise ValueError('service_still_active')
    fd,name=tempfile.mkstemp(prefix='.'+binary.name+'-update-',dir=binary.parent)
    try:
        with os.fdopen(fd,'wb') as output:output.write(data);output.flush();os.fsync(output.fileno())
        os.chown(name,expected['uid'],expected['gid']);os.chmod(name,expected['mode'])
        if ordinary()!=expected:raise ValueError('binary_changed_during_replace')
        os.replace(name,binary)
        parent=os.open(binary.parent,os.O_RDONLY)
        try:os.fsync(parent)
        finally:os.close(parent)
    finally:
        if os.path.exists(name):os.unlink(name)
    result=ordinary()
elif action=='verify':
    meta=ordinary()
    if meta['sha256']!=p['expected_sha256']:raise ValueError('installed_binary_mismatch')
    value=live(); states={n:service(n) for n in c['services']}
    if {n:s['active'] for n,s in states.items()}!=p['active']:raise ValueError('service_state_mismatch')
    pid=states[c['instance_service']]['pid']
    if not states[c['instance_service']]['active'] or pid<=0 or digest(Path(PROC)/str(pid)/'exe')!=p['expected_sha256']:raise ValueError('running_binary_mismatch')
    result={'verified':True,'binary_sha256':meta['sha256'],'instance_id':value['instance_id']}
elif action=='export':
    ordinary();live();version=call([str(binary),'--version']).decode().strip()
    import re
    match=re.search(r'\b(\d+)\.(\d+)\.(\d+)\b',version)
    if not match:raise ValueError('version_unavailable')
    supported=tuple(map(int,match.groups()))>=(0,22,5)
    result={'binary_sha256':digest(binary),'version':version,'supported':supported,'files':{}}
    if supported and p.get('known_sha256')!=result['binary_sha256']:
        with tempfile.TemporaryDirectory(prefix='.instance-export-',dir=binary.parent) as root:
            root=Path(root);call([str(binary),'export-update-helper','--path',str(root/'worker.py')]);call([str(binary),'export-update-support','--directory',str(root)])
            total=0
            for path in sorted(root.rglob('*')):
                if path.is_symlink():raise ValueError('helper_export_symlink')
                if path.is_file():
                    data=path.read_bytes();total+=len(data)
                    if total>4*1024*1024:raise ValueError('helper_export_limit')
                    result['files'][path.relative_to(root).as_posix()]=base64.b64encode(data).decode()
        if digest(binary)!=result['binary_sha256']:raise ValueError('binary_changed_during_export')
else:raise ValueError('unknown_guest_operation')
print(json.dumps(result,separators=(',',':')))
'''


def read(path):
    return json.loads(Path(path).read_text())


def digest(path):
    result = hashlib.sha256()
    with Path(path).open('rb') as stream:
        while block := stream.read(1024 * 1024):
            result.update(block)
    return result.hexdigest()


def load_worker():
    spec = importlib.util.spec_from_file_location('incus_update_worker', Path(__file__).with_name('worker.py'))
    worker = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(worker)
    return worker


def write(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.NamedTemporaryFile(dir=path.parent, mode='w', delete=False) as stream:
        json.dump(value, stream, indent=2)
        stream.flush()
        os.fchmod(stream.fileno(), 0o600)
        os.fsync(stream.fileno())
        temporary = Path(stream.name)
    try:
        os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        temporary.unlink(missing_ok=True)


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def bounded(argv, data=None, limit=MAX_EXPORT, timeout=120):
    with tempfile.TemporaryFile() as source, tempfile.TemporaryFile() as output:
        if data is not None:
            source.write(data)
            source.seek(0)
        process = subprocess.Popen(argv, stdin=source, stdout=output, stderr=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + timeout
            while process.poll() is None:
                if os.fstat(output.fileno()).st_size > limit:
                    raise ValueError('incus_response_limit')
                if time.monotonic() >= deadline:
                    raise subprocess.TimeoutExpired(argv, timeout)
                time.sleep(.02)
            if process.returncode:
                raise RuntimeError('incus_command_failed')
            if os.fstat(output.fileno()).st_size > limit:
                raise ValueError('incus_response_limit')
            output.seek(0)
            return output.read(limit + 1)
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()


class Provider:
    def __init__(self, config):
        self.config = Path(config)
        self.config_digest = digest(self.config)
        self.c = read(config)
        required = {'schema', 'incus', 'project', 'guest', 'guest_uuid', 'home', 'binary', 'guest_python',
                    'instance_id', 'services', 'instance_service', 'state'}
        if set(self.c) - {'participants'} != required or self.c['schema'] != 1:
            raise ValueError('invalid_incus_config')
        for key in ('incus', 'home', 'binary', 'guest_python', 'state'):
            value = self.c[key]
            if not isinstance(value, str) or '\x00' in value or not Path(value).is_absolute() or '..' in Path(value).parts:
                raise ValueError('absolute_owner_paths_required')
        for key in ('project', 'guest'):
            if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,62}', self.c[key]):
                raise ValueError('fixed_local_guest_required')
        for key in ('instance_id', 'guest_uuid'):
            if str(uuid.UUID(self.c[key])) != self.c[key]:
                raise ValueError('invalid_instance_or_guest_id')
        services = self.c['services']
        if not isinstance(services, list) or not 1 <= len(services) <= 16 or len(set(services)) != len(services):
            raise ValueError('invalid_services')
        if any(not isinstance(n, str) or not re.fullmatch(r'[A-Za-z0-9_@.-]+\.service', n) for n in services):
            raise ValueError('invalid_services')
        if self.c['instance_service'] != services[0]:
            raise ValueError('instance_service_must_be_first')
        if Path(self.c['binary']).is_relative_to(Path(self.c['home'])):
            raise ValueError('binary_must_be_outside_agent_home')
        self.root = Path(self.c['state'])
        if self.root.resolve().is_relative_to(Path(self.c['home']).resolve()):
            raise ValueError('provider_state_must_be_outside_agent_home')
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)

    def exec(self, argv, data=None, limit=MAX_EXPORT):
        command = [self.c['incus'], '--force-local', 'exec', '-T', '--mode=non-interactive', '--project',
                   self.c['project'], self.c['guest'], '--', *argv]
        return bounded(command, data, limit)

    def guest_id(self):
        result = bounded([self.c['incus'], '--force-local', 'config', 'get', '--project', self.c['project'],
                          self.c['guest'], 'volatile.uuid'], limit=1024).decode().strip()
        if str(uuid.UUID(result)) != result:
            raise ValueError('guest_identity_unavailable')
        if result != self.c['guest_uuid']:
            raise ValueError('incus_identity_changed')
        return result

    def guest(self, action, **values):
        self.guest_id()
        data = {'action': action, 'config': {k: self.c[k] for k in
                ('home', 'binary', 'instance_id', 'services', 'instance_service')}, **values}
        limit = MAX_BINARY * 2 if action == 'read_binary' else MAX_EXPORT
        result = json.loads(self.exec([self.c['guest_python'], '-I', '-c', GUEST],
                                     json.dumps(data).encode(), limit))
        self.guest_id()
        return result

    def probe(self):
        identity = self.guest_id()
        result = self.guest('probe')
        if self.guest_id() != identity:
            raise ValueError('guest_changed_during_probe')
        result['guest_uuid'] = identity
        return result

    def status(self):
        p = self.probe()
        return {'version': p['version'], 'sha256': p['binary']['sha256'],
                'instance_id': p['instance_id'], 'guest_uuid': p['guest_uuid']}

    def plan(self):
        observed = self.probe()
        if not observed['services'][self.c['instance_service']]['active']:
            raise ValueError('instance_not_running')
        folder = self.plan_location().parent
        worker = load_worker()

        def inspect(path):
            return self.guest('inspect', data=base64.b64encode(Path(path).read_bytes()).decode(),
                              sha256=digest(path))['version']

        candidate = worker.candidate({'id': 'instance', 'kind': 'instance', 'binary': self.c['binary']},
                                     'stable', folder, observed={'version': observed['version'],
                                     'sha256': observed['binary']['sha256'], 'platform': observed['platform']},
                                     inspect_candidate=inspect)
        result = {'available': candidate['available'], 'version': candidate.get('target_version', observed['version']),
                  'config_sha256': self.config_digest, 'guest_uuid': observed['guest_uuid'],
                  'instance_id': self.c['instance_id'], 'before': observed['binary'],
                  'candidate': candidate}
        if result['available']:
            result['candidate_file'] = str(folder / 'instance.candidate')
        return result

    def plan_location(self):
        path = Path(os.environ['CONSTELLATION_UPDATE_PLAN'])
        identifier = str(uuid.UUID(path.parent.name))
        expected = self.root.parent / 'plans' / identifier / 'instance.provider.json'
        if path != expected or path.resolve() != expected or not path.parent.is_dir():
            raise ValueError('invalid_provider_plan_path')
        return path

    def context(self):
        location = self.plan_location()
        wrapper = read(location)
        identifier = str(uuid.UUID(wrapper['id']))
        if identifier != wrapper['id'] or identifier != location.parent.name or wrapper['component'] != 'instance':
            raise ValueError('invalid_plan_id')
        plan = wrapper['plan']
        if digest(self.config) != self.config_digest or plan['config_sha256'] != self.config_digest:
            raise ValueError('incus_enrollment_changed')
        if plan['instance_id'] != self.c['instance_id'] or self.guest_id() != plan['guest_uuid']:
            raise ValueError('incus_identity_changed')
        if not plan['available']:
            raise ValueError('plan_has_no_candidate')
        path = Path(plan['candidate_file'])
        if path != location.parent / 'instance.candidate' or path.is_symlink() or path.resolve() != path:
            raise ValueError('invalid_candidate_path')
        return plan, location.parent / 'incus-receipt.json', location.parent / 'instance.incus.previous'

    def services(self, active, operation):
        names = reversed(self.c['services']) if operation == 'stop' else self.c['services']
        for name in names:
            if active[name]:
                self.guest('service', name=name, operation=operation)

    def checked_receipt(self, path, plan):
        receipt = read(path)
        if receipt.get('state') not in ('applying', 'applied', 'rolling_back', 'rolled_back'):
            raise ValueError('provider_receipt_state_unknown')
        if receipt['config_sha256'] != self.config_digest or receipt['guest_uuid'] != plan['guest_uuid'] or receipt['before'] != plan['before'] or receipt['after_sha256'] != plan['candidate']['sha256']:
            raise ValueError('provider_receipt_changed')
        if set(receipt['active']) != set(self.c['services']) or any(not isinstance(v, bool) for v in receipt['active'].values()):
            raise ValueError('provider_service_snapshot_changed')
        return receipt

    def replace(self, path, expected, intended_digest):
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != intended_digest:
            raise ValueError('replacement_source_digest_mismatch')
        return self.guest('replace', expected=expected, data=base64.b64encode(data).decode(),
                          sha256=intended_digest)

    def apply(self):
        plan, path, backup = self.context()
        if path.exists():
            receipt = self.checked_receipt(path, plan)
            if receipt['state'] == 'applied':
                return self.verify()
            raise ValueError('interrupted_apply_requires_rollback')
        if digest(plan['candidate_file']) != plan['candidate']['sha256']:
            raise ValueError('candidate_digest_mismatch')
        current = self.probe()
        if current['guest_uuid'] != plan['guest_uuid'] or current['binary'] != plan['before']:
            raise ValueError('installation_changed_since_plan')
        active = {n: value['active'] for n, value in current['services'].items()}
        if not active[self.c['instance_service']]:
            raise ValueError('instance_not_running')
        preserved = self.guest('read_binary', expected=plan['before'])
        data = base64.b64decode(preserved['data'], validate=True)
        if hashlib.sha256(data).hexdigest() != plan['before']['sha256']:
            raise ValueError('backup_digest_mismatch')
        if backup.exists():
            if backup.is_symlink() or digest(backup) != plan['before']['sha256']:
                raise ValueError('rollback_backup_changed')
        else:
            with backup.open('xb') as output:
                output.write(data); output.flush(); os.fchmod(output.fileno(), 0o600); os.fsync(output.fileno())
        sync_directory(backup.parent)
        receipt = {'state': 'applying', 'config_sha256': self.config_digest,
                   'guest_uuid': plan['guest_uuid'], 'before': plan['before'],
                   'after_sha256': plan['candidate']['sha256'], 'active': active}
        write(path, receipt)
        self.services(active, 'stop')
        self.replace(Path(plan['candidate_file']), plan['before'], plan['candidate']['sha256'])
        self.services(active, 'start')
        self.verify_expected(receipt, receipt['after_sha256'])
        receipt['state'] = 'applied'; write(path, receipt)
        return {'applied': True, 'version': plan['version']}

    def rollback(self):
        plan, path, backup = self.context()
        if not path.exists():
            return self.verify_unchanged(plan)
        receipt = self.checked_receipt(path, plan)
        if not backup.is_file() or backup.is_symlink() or digest(backup) != plan['before']['sha256']:
            raise ValueError('rollback_backup_changed')
        current = self.guest('metadata')
        allowed = (plan['before']['sha256'], receipt['after_sha256'])
        if current['sha256'] not in allowed or any(current[k] != plan['before'][k] for k in ('mode', 'uid', 'gid')):
            raise ValueError('installed_binary_changed')
        if receipt['state'] == 'rolled_back':
            return self.verify()
        receipt['state'] = 'rolling_back'; write(path, receipt)
        self.services(receipt['active'], 'stop')
        # A timed-out remote replacement may have completed while service stop
        # waited for its guest lock. Decide from the settled bytes, not the
        # earlier observation made before acquiring that boundary.
        current = self.guest('metadata')
        if current['sha256'] not in allowed or any(current[k] != plan['before'][k] for k in ('mode', 'uid', 'gid')):
            raise ValueError('installed_binary_changed')
        if current['sha256'] != plan['before']['sha256']:
            self.replace(backup, current, plan['before']['sha256'])
        self.services(receipt['active'], 'start')
        self.verify_expected(receipt, plan['before']['sha256'])
        receipt['state'] = 'rolled_back'; write(path, receipt)
        return {'rolled_back': True}

    def verify_expected(self, receipt, expected):
        deadline = time.monotonic() + HEALTH_TIMEOUT
        while True:
            try:
                return self.guest('verify', expected_sha256=expected, active=receipt['active'])
            except (RuntimeError, subprocess.TimeoutExpired):
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.25)

    def verify(self):
        plan, path, _ = self.context()
        if not path.exists():
            return self.verify_unchanged(plan)
        receipt = self.checked_receipt(path, plan)
        if receipt['state'] not in ('applied', 'rolled_back'):
            raise ValueError('provider_not_settled')
        expected = plan['before']['sha256'] if receipt['state'] == 'rolled_back' else receipt['after_sha256']
        return self.verify_expected(receipt, expected)

    def verify_unchanged(self, plan):
        observed = self.probe()
        if observed['binary'] != plan['before']:
            raise ValueError('missing_rollback_receipt')
        active = {name: state['active'] for name, state in observed['services'].items()}
        result = self.verify_expected({'active': active}, plan['before']['sha256'])
        return {**result, 'untouched': True}

    def management(self, request):
        if not isinstance(request, dict) or request.get('schema') != 1 or request.get('instance_id') != self.c['instance_id']:
            raise ValueError('invalid_management_request')
        action = request.get('action')
        before = self.guest_id()
        if action == 'api':
            if set(request) != {'schema', 'instance_id', 'action', 'path', 'body'}:
                raise ValueError('invalid_management_request')
            path = request['path']; body = request['body']
            if path not in API_PATHS or API_PATHS[path] != (body is not None) or body is not None and not isinstance(body, dict):
                raise ValueError('unsupported_management_resource')
            result = self.guest('api', path=path, body=body)
        elif action == 'export' and set(request) <= {'schema', 'instance_id', 'action', 'known_sha256'}:
            known = request.get('known_sha256')
            if known is not None and (not isinstance(known, str) or not re.fullmatch('[0-9a-f]{64}', known)):
                raise ValueError('invalid_known_binary_digest')
            result = self.guest('export', known_sha256=known)
        else:
            raise ValueError('unsupported_management_action')
        if self.guest_id() != before:
            raise ValueError('guest_changed_during_management')
        return {'schema': 1, 'instance_id': self.c['instance_id'], 'result': result}


if __name__ == '__main__':
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True)
    parser.add_argument('--action', required=True, choices=['status', 'probe', 'plan', 'apply', 'rollback', 'verify', 'management'])
    args = parser.parse_args()
    provider = Provider(args.config)
    if args.action == 'management':
        data = sys.stdin.buffer.read(MAX_EXPORT + 1)
        if len(data) > MAX_EXPORT:raise ValueError('management_request_limit')
        result = provider.management(json.loads(data))
    else:
        result = getattr(provider, args.action)()
    print(json.dumps(result, separators=(',', ':')))
