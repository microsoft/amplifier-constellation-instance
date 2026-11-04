#!/usr/bin/env python3
"""One installation's update helper. Run OUTSIDE its container, as its owner.

The registration file is private operator input, never supplied by an agent.
The authenticated instance API supplies only action/plan IDs and policy choices.
"""
import argparse, contextlib, hashlib, io, json, os, platform, re, shutil
import sqlite3, ssl, subprocess, sys, tarfile, tempfile, time, urllib.request, uuid, zipfile
from pathlib import Path

REPOSITORIES = {'instance': ('amplifier-constellation-instance','agent-instance','ci.yml'),
                'runtime': ('amplifier-constellation-runtime','agent-runtime','check.yml'),
                'slack': ('amplifier-acp-slack','acp-slack-adapter','check.yml'),
                'teams': ('amplifier-acp-teams','acp-teams-adapter','check.yml')}
CRITICAL = {'stopping','swapping','verifying','rolling_back'}
LIMIT = 256*1024*1024

def read(path): return json.loads(Path(path).read_text())
def write(path,value):
    path=Path(path);path.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
    with tempfile.NamedTemporaryFile(dir=path.parent,mode='w',delete=False,encoding='utf-8') as f:
        json.dump(value,f,indent=2);f.flush();os.fsync(f.fileno());name=f.name
    os.chmod(name,0o600);os.replace(name,path)
def digest(path):
    result=hashlib.sha256()
    with Path(path).open('rb') as f:
        while block:=f.read(1024*1024):result.update(block)
    return result.hexdigest()
def run(argv,timeout=90,env=None):
    p=subprocess.run([str(a) for a in argv],capture_output=True,stdin=subprocess.DEVNULL,timeout=timeout,env=env)
    if p.returncode: raise RuntimeError('command_failed')
    return p.stdout

def version(text):
    m=re.search(r'\bv?(\d+)\.(\d+)\.(\d+)',text)
    if not m: raise ValueError('version_unavailable')
    return tuple(map(int,m.groups()))

def load_registration(path):
    c=read(path)
    if c.get('schema')!=1 or not c.get('components') or len(c['components'])>16: raise ValueError('invalid_registration')
    for key in ['home','state']:
        if not Path(c[key]).is_absolute(): raise ValueError('absolute_paths_required')
    # The helper state and any host credentials must not be in an agent's mounted home.
    if Path(c['state']).resolve().is_relative_to(Path(c['home']).resolve()): raise ValueError('helper_state_must_be_outside_agent_home')
    ids=set()
    for p in c['components']:
        if p['id'] in ids or not re.fullmatch('[a-z][a-z0-9-]{0,63}',p['id']):raise ValueError('invalid_component_id')
        ids.add(p['id'])
        if p.get('provider','github')=='github':
            if p['kind'] not in REPOSITORIES: raise ValueError('unsupported_component')
            if not Path(p['binary']).is_file() or Path(p['binary']).is_symlink():raise ValueError('ordinary_binary_required')
        elif p['provider']=='command':
            # Extensible owner-enrolled provider. No values from chat enter argv.
            if p.get('protocol',1) not in (1,2):raise ValueError('unsupported_command_protocol')
            actions=['status','plan','apply','rollback']+(['verify'] if p.get('protocol')==2 else [])
            for action in actions:
                a=p['commands'][action]
                if not a or not Path(a[0]).is_absolute() or not all(isinstance(x,str) for x in a):raise ValueError('fixed_absolute_provider_commands_required')
        else: raise ValueError('unsupported_provider')
    for s in c.get('services',[]):
        if s['kind'] not in ['systemd-user','launchd','scheduled-task']:raise ValueError('unsupported_service')
        if not re.fullmatch(r'[A-Za-z0-9_.@-]+',s['name']):raise ValueError('invalid_service_name')
    return c

def http_json(url,body=None,headers=None,ctx=None,method=None):
    r=urllib.request.Request(url,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json',**(headers or {})},method=method)
    with urllib.request.urlopen(r,context=ctx,timeout=15) as f:return json.loads(f.read(2*1024*1024))

def gh(path):
    # Public release metadata needs no login. An optional local gh account handles
    # authenticated Actions downloads and public-API rate limits on busy machines.
    try:return http_json('https://api.github.com/'+path,headers={'User-Agent':'Constellation-installation-updates'})
    except Exception:
        return json.loads(run(['gh','api','--hostname','github.com',path]))

def download(url,destination,expected):
    if not url.startswith('https://github.com/microsoft/') or not re.fullmatch('sha256:[0-9a-f]{64}',expected):raise ValueError('unverified_release_asset')
    with urllib.request.urlopen(url,timeout=90) as f,Path(destination).open('wb') as out:
        total=0
        while block:=f.read(1024*1024):
            total+=len(block)
            if total>LIMIT:raise ValueError('asset_too_large')
            out.write(block)
    if 'sha256:'+digest(destination)!=expected:raise ValueError('asset_digest_mismatch')

def extract(data,binary_name):
    """Select only an ordinary exact-name binary, never extract arbitrary paths."""
    if data[:2]==b'PK':
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            entries=[i for i in z.infolist() if i.filename==binary_name and not i.is_dir() and (i.external_attr>>16)&0o170000!=0o120000]
            if len(entries)==1 and entries[0].file_size<=LIMIT:return z.read(entries[0])
            archives=[i for i in z.infolist() if '/' not in i.filename and i.filename.endswith('.tar.gz')]
            if len(archives)==1 and archives[0].file_size<=LIMIT:return extract(z.read(archives[0]),binary_name)
    else:
        with tarfile.open(fileobj=io.BytesIO(data),mode='r:*') as t:
            entries=[i for i in t.getmembers() if i.name in (binary_name,'./'+binary_name) and i.isfile() and i.size<=LIMIT]
            if len(entries)==1:return t.extractfile(entries[0]).read()
    raise ValueError('unsupported_archive_layout')

def platform_names():
    osname=platform.system();arch={'arm64':'aarch64','aarch64':'aarch64','AMD64':'x86_64','x86_64':'x86_64'}[platform.machine()]
    return osname,arch,{'Linux':arch+'-unknown-linux-gnu','Darwin':arch+'-apple-darwin','Windows':arch+'-pc-windows-msvc'}[osname]

def candidate(component,channel,folder):
    kind=component['kind'];repo,binary,workflow=REPOSITORIES[kind];repo='microsoft/'+repo
    osname,arch,target=platform_names();binary+=('.exe' if osname=='Windows' else '')
    current=run([component['binary'],'--version']).decode().strip();installed=digest(component['binary'])
    if channel=='stable':
        release=gh('repos/'+repo+'/releases/latest');tag=release['tag_name']
        if version(tag)<=version(current):return {'id':component['id'],'version':current,'installed_sha256':installed,'available':False,'reason':'installed_version_is_current_or_newer'}
        names=[binary.removesuffix('.exe')+'-'+target+'.tar.gz',binary.removesuffix('.exe')+'-'+tag.lstrip('v')+'-'+target+'.tar.gz',binary.removesuffix('.exe')+'-'+tag.lstrip('v')+'-'+{'Linux':'linux','Darwin':'macos','Windows':'windows'}[osname]+'-'+arch+('.zip' if osname=='Windows' else '.tar.gz')]
        asset=next((a for a in release['assets'] if a['name'] in names),None)
        if not asset:raise ValueError('release_not_available_for_platform')
        archive=folder/(component['id']+'.archive');download(asset['browser_download_url'],archive,asset.get('digest',''))
        source={'repository':repo,'tag':tag,'asset_digest':asset['digest'],'url':release['html_url']}
    else:
        runs=gh('repos/'+repo+'/actions/workflows/'+workflow+'/runs?branch=main&event=push&status=success&per_page=20')['workflow_runs']
        build=next((r for r in runs if r['conclusion']=='success' and r['event']=='push' and r['head_branch']=='main' and r['repository']['full_name']==repo),None)
        if not build:raise ValueError('no_passing_main_build')
        prefix='candidate' if kind=='instance' else binary.removesuffix('.exe')
        name=prefix+'-'+{'Linux':'Linux','Darwin':'macOS','Windows':'Windows'}[osname]+'-'+('ARM64' if arch=='aarch64' else 'X64')+'-'+build['head_sha']
        assets=gh('repos/'+repo+'/actions/runs/'+str(build['id'])+'/artifacts?per_page=100')['artifacts']
        a=next((a for a in assets if a['name']==name and not a['expired']),None)
        if not a or a['size_in_bytes']>LIMIT or not re.fullmatch('sha256:[0-9a-f]{64}',a.get('digest','')):raise ValueError('verified_build_unavailable')
        archive=folder/(component['id']+'.archive')
        # gh needs the installation owner's account for Actions artifacts; it is
        # never injected into the agent. Bound disk use and time during streaming.
        with archive.open('wb') as out:
            proc=subprocess.Popen(['gh','api','--hostname','github.com','repos/'+repo+'/actions/artifacts/'+str(a['id'])+'/zip'],stdout=out,stderr=subprocess.DEVNULL,stdin=subprocess.DEVNULL)
            try:
                end=time.monotonic()+120
                while proc.poll() is None:
                    if time.monotonic()>end or archive.stat().st_size>LIMIT:raise ValueError('artifact_download_limit')
                    time.sleep(.1)
                if proc.returncode:raise ValueError('development_channel_requires_github_artifact_access')
            finally:
                if proc.poll() is None:proc.kill()
                proc.wait()
        if 'sha256:'+digest(archive)!=a['digest']:raise ValueError('artifact_digest_mismatch')
        source={'repository':repo,'commit':build['head_sha'],'run_id':build['id'],'url':build['html_url'],'asset_digest':a['digest']}
    target_file=folder/(component['id']+'.candidate');target_file.write_bytes(extract(archive.read_bytes(),binary));target_file.chmod(0o700)
    new_version=run([target_file,'--version']).decode().strip()
    if not new_version.startswith(binary.removesuffix('.exe')+' ') or version(new_version)<version(current):raise ValueError('candidate_identity_or_version_mismatch')
    return {'id':component['id'],'version':current,'target_version':new_version,'installed_sha256':installed,'sha256':digest(target_file),'available':digest(target_file)!=installed,**source}

class Worker:
    def __init__(self,registration):
        self.c=load_registration(registration);self.home=Path(self.c['home']);self.root=Path(self.c['state']);self.root.mkdir(parents=True,exist_ok=True,mode=0o700)
        self.file=self.root/'state.json';self.state=read(self.file) if self.file.exists() else {'state':'idle','components':[],'last_check':0}
        self.context=ssl.create_default_context(cafile=str(self.home/'tls/root-ca.crt'))
    def api(self,path='updates',body=None):
        config=read(self.home/'config.json')['config'];token=(self.home/'secrets/control-token').read_text().strip()
        return http_json('https://127.0.0.1:'+str(config['server']['port'])+'/api/v1/'+path,body,{'Authorization':'Bearer '+token},self.context)
    def save(self,**values):self.state.update(values,updated_at=int(time.time()*1000));write(self.file,self.state)
    def report(self,drain=False,finished=None):
        return self.api('updates/report',{'status':self.state,'finished_request':finished,'drain_seconds':60 if drain else 0})
    def control(self,action):
        services=self.c.get('services',[])
        for s in (reversed(services) if action=='stop' else services):
            if s['kind']=='systemd-user':run(['systemctl','--user',action,s['name']])
            elif s['kind']=='launchd':
                if action=='stop':
                    loaded=subprocess.run(['launchctl','print',s['domain']+'/'+s['name']],capture_output=True).returncode==0
                    if loaded:run(['launchctl','bootout',s['domain']+'/'+s['name']])
                else:run(['launchctl','bootstrap',s['domain'],s['file']])
            else:
                verb='Start' if action=='start' else 'Stop'
                run(['powershell.exe','-NoProfile','-NonInteractive','-Command',verb+'-ScheduledTask -TaskName "'+s['name']+'"'])
    def health(self,expected):
        deadline=time.monotonic()+60
        while time.monotonic()<deadline:
            try:
                status=self.api('status')
                if status['instance_id']!=self.c['instance_id'] or status['uptime_ms']>90000:raise ValueError('instance_not_restarted')
                for p in self.c['components']:
                    if p.get('provider','github')=='github' and digest(p['binary'])!=expected[p['id']]:raise ValueError('installed_binary_mismatch')
                for home in self.c.get('slack_homes',[]):
                    cfg=read(Path(home)/'adapter.json');ctx=ssl.create_default_context(cafile=str(Path(home)/'tls/root-ca.crt'))
                    h={'Authorization':'Bearer '+(Path(home)/'secrets/control-token').read_text().strip()}
                    value=http_json('https://127.0.0.1:'+str(cfg['server']['port'])+'/api/v1/status',headers=h,ctx=ctx)
                    if cfg['enabled'] and value.get('state',{}).get('slack_connection',{}).get('state')!='connected':raise ValueError('slack_reconnecting')
                    if cfg['enabled'] and value.get('runtime',{}).get('updated_at',0)<self.state.get('cutover_at',0):raise ValueError('old_slack_process')
                return
            except Exception:time.sleep(.5)
        raise ValueError('reconnection_verification_failed')
    def provider_call(self,component,action,folder):
        env=os.environ.copy()
        if component.get('protocol')==2:
            env['CONSTELLATION_UPDATE_PLAN']=str(folder/(component['id']+'.provider.json'))
        return run(component['commands'][action],timeout=600,env=env)
    def provider_verify(self,plan):
        folder=self.root/'plans'/plan['id']
        for p in self.c['components']:
            if p.get('provider')=='command' and p.get('protocol')==2 and p['id'] in plan.get('provider_attempts',[]):
                self.provider_call(p,'verify',folder)
    def plan(self,policy):
        identifier=str(uuid.uuid4());folder=self.root/'plans'/identifier;folder.mkdir(parents=True,mode=0o700)
        self.save(state='checking',error=None);self.report();components=[]
        for p in self.c['components']:
            if p.get('provider','github')=='github':components.append(candidate(p,policy['channel'],folder))
            else:
                current=json.loads(run(p['commands']['status']).decode())
                result=json.loads(self.provider_call(p,'plan',folder).decode())
                part={'id':p['id'],'available':bool(result['available']),'version':str(current.get('version','')),'target_version':str(result.get('version','')),'provider':'command'}
                if p.get('protocol')==2:
                    file=folder/(p['id']+'.provider.json')
                    write(file,{'id':identifier,'component':p['id'],'plan':result})
                    part['provider_plan_sha256']=digest(file)
                components.append(part)
        plan={'id':identifier,'components':components,'policy_revision':policy['revision'],'channel':policy['channel'],'created_at':int(time.time()*1000)}
        write(folder/'plan.json',plan)
        self.save(state='available' if any(p['available'] for p in components) else 'up_to_date',plan_id=identifier,components=components,last_check=int(time.time()*1000))
        self.prune_plans();self.report();return plan
    def prune_plans(self):
        keep={self.state.get('plan_id'),self.state.get('rollback_plan_id')}
        folders=[]
        for folder in (self.root/'plans').iterdir():
            try:uuid.UUID(folder.name)
            except ValueError:continue
            if folder.is_dir() and not folder.is_symlink():folders.append(folder)
        for folder in sorted(folders,key=lambda p:p.stat().st_mtime,reverse=True)[3:]:
            if folder.name not in keep:shutil.rmtree(folder)
    def allowed(self,policy,agent):
        current=self.api()['policy']
        if current['revision']!=policy['revision'] or agent and not current['agent_initiated']:raise ValueError('update_policy_changed')
    @contextlib.contextmanager
    def drained(self,policy,agent):
        locks=[]
        try:
            self.save(state='waiting_for_idle');deadline=time.monotonic()+1800
            while True:
                self.allowed(policy,agent);status=self.report(drain=True);busy=status['active_prompts']
                for path in self.c.get('slack_homes',[]):
                    h=Path(path);write(h/'updates/drain.json',{'expires_at':int(time.time()*1000)+60000,'plan_id':self.state['plan_id']})
                    with sqlite3.connect('file:'+str(h/'state.sqlite3')+'?mode=ro',uri=True) as db:
                        busy+=db.execute("SELECT (SELECT count(*) FROM jobs WHERE status='running')+(SELECT count(*) FROM outbox WHERE status IN ('ready','sending','uncertain'))").fetchone()[0]
                if not busy:
                    if os.name!='nt':
                        import fcntl
                        for path in self.c.get('slack_homes',[]):
                            f=(Path(path)/'updates/admission.lock').open('rb');fcntl.flock(f,fcntl.LOCK_EX);locks.append(f)
                    # Recheck after admission locks; a prompt may have entered just
                    # before the drain signal was published.
                    if self.report(drain=True)['active_prompts']==0:
                        pending=0
                        for path in self.c.get('slack_homes',[]):
                            with sqlite3.connect('file:'+str(Path(path)/'state.sqlite3')+'?mode=ro',uri=True) as db:
                                pending+=db.execute("SELECT (SELECT count(*) FROM jobs WHERE status='running')+(SELECT count(*) FROM outbox WHERE status IN ('ready','sending','uncertain'))").fetchone()[0]
                        if not pending:break
                    for f in locks:f.close()
                    locks=[]
                if time.monotonic()>deadline:raise ValueError('work_did_not_drain')
                time.sleep(1)
            yield
        finally:
            for f in locks:f.close()
            for path in self.c.get('slack_homes',[]):write(Path(path)/'updates/drain.json',{'expires_at':0})
    def replace(self,source,dest):
        dest=Path(dest);temp=dest.with_name(dest.name+'.update-'+uuid.uuid4().hex)
        shutil.copyfile(source,temp);temp.chmod(0o755)
        with temp.open('r+b') as f:os.fsync(f.fileno())
        os.replace(temp,dest)
    def rollback(self,plan):
        folder=self.root/'plans'/plan['id'];self.validate_provider_plans(plan);self.save(state='rolling_back');self.control('stop')
        expected={}
        for p in self.c['components']:
            if p.get('provider','github')=='github':
                backup=folder/(p['id']+'.previous');expected[p['id']]=plan['previous'][p['id']]
                if digest(backup)!=expected[p['id']]:raise ValueError('rollback_digest_mismatch')
                self.replace(backup,p['binary'])
            elif p['id'] in plan.get('provider_attempts',[x['id'] for x in plan['components'] if x['available']]):self.provider_call(p,'rollback',folder)
        self.control('start');self.provider_verify(plan);self.health(expected);self.save(state='rolled_back',rollback_plan_id=None);self.report()
    def validate_provider_plans(self,plan):
        folder=self.root/'plans'/plan['id']
        for p in self.c['components']:
            if p.get('provider')=='command' and p.get('protocol')==2:
                part=next(x for x in plan['components'] if x['id']==p['id'])
                if digest(folder/(p['id']+'.provider.json'))!=part['provider_plan_sha256']:raise ValueError('provider_plan_changed')
    def apply(self,plan,policy,agent=False,rollback=False,scheduled=False):
        folder=self.root/'plans'/plan['id'];self.save(plan_id=plan['id'])
        if not rollback and not any(p['available'] for p in plan['components']):self.save(state='up_to_date');return
        if not rollback and plan['policy_revision']!=policy['revision']:raise ValueError('plan_policy_changed')
        with self.drained(policy,agent):
            self.allowed(policy,agent)
            if scheduled and policy['install']=='window' and not self.in_window(policy):raise ValueError('update_window_ended_before_idle')
            if rollback:
                if 'previous' not in plan:raise ValueError('plan_has_no_installed_version_to_rollback')
                for p in self.c['components']:
                    if p.get('provider','github')!='github':continue
                    part=next(x for x in plan['components'] if x['id']==p['id'])
                    expected=part['sha256'] if part['available'] else part['installed_sha256']
                    if digest(p['binary'])!=expected:raise ValueError('installed_binary_changed')
                self.rollback(plan);return
            self.validate_provider_plans(plan)
            previous={};expected={}
            for p in self.c['components']:
                if p.get('provider','github')!='github':continue
                part=next(x for x in plan['components'] if x['id']==p['id'])
                previous[p['id']]=digest(p['binary'])
                if previous[p['id']]!=part['installed_sha256']:raise ValueError('installed_binary_changed')
                if part['available'] and digest(folder/(p['id']+'.candidate'))!=part['sha256']:raise ValueError('candidate_digest_mismatch')
                shutil.copyfile(p['binary'],folder/(p['id']+'.previous'))
                expected[p['id']]=part['sha256'] if part['available'] else previous[p['id']]
            plan['previous']=previous;plan['provider_attempts']=[];write(folder/'plan.json',plan)
            # State survives both instance restarts and helper interruption.
            self.save(state='stopping',cutover_at=int(time.time()*1000));self.report(drain=True)
            try:
                self.control('stop');self.save(state='swapping')
                for p in self.c['components']:
                    part=next(x for x in plan['components'] if x['id']==p['id'])
                    if part['available']:
                        if p.get('provider','github')=='github':self.replace(folder/(p['id']+'.candidate'),p['binary'])
                        else:
                            plan.setdefault('provider_attempts',[]).append(p['id']);write(folder/'plan.json',plan)
                            self.provider_call(p,'apply',folder)
                self.control('start');self.save(state='verifying');self.provider_verify(plan);self.health(expected)
                self.save(state='succeeded',rollback_plan_id=plan['id'],installed_at=int(time.time()*1000));self.report()
            except BaseException:
                try:self.rollback(plan)
                except BaseException:self.save(state='recovery_required');raise
                raise
    def finish_request(self,identifier):
        # Persist completion before acknowledging it over a connection that may
        # disappear during restart. A retried request cannot repeat a cutover.
        completed=self.state.get('completed_requests',[])
        self.save(completed_requests=(completed+[identifier])[-32:],active_request=None)
        self.report(finished=identifier)
    @staticmethod
    def in_window(policy):return (time.gmtime().tm_hour-policy['window_start_utc'])%24<policy['window_hours']
    def tick(self):
        if self.state['state'] in CRITICAL:
            try:self.rollback(read(self.root/'plans'/self.state['plan_id']/'plan.json'))
            except Exception:
                self.save(state='recovery_required',error='Interrupted update could not recover; inspect the installation.');self.report();return
            if self.state.get('active_request'):self.finish_request(self.state['active_request'])
        elif self.state.get('active_request') and self.state['state'] in ['succeeded','rolled_back']:
            self.finish_request(self.state['active_request'])
        status=self.report();policy=status['policy'];request=status.get('request')
        active=request and request.get('pending')
        if active:
            if request['id'] in self.state.get('completed_requests',[]):self.report(finished=request['id']);return
            action=request['action'];agent=request['actor']=='agent'
            try:
                if self.state['state']=='recovery_required' and action!='rollback':raise ValueError('recovery_required_before_new_update')
                if request['policy_revision']!=policy['revision']:raise ValueError('queued_request_policy_changed')
                if agent and not policy['agent_initiated']:raise ValueError('agent_updates_disabled')
                self.save(active_request=request['id'])
                if action in ['check','plan','update']:plan=self.plan(policy)
                else:plan=read(self.root/'plans'/str(uuid.UUID(request['plan_id']))/'plan.json')
                if action in ['apply','update','rollback']:self.apply(plan,policy,agent,action=='rollback')
            except Exception as e:
                if self.state['state'] not in ['recovery_required','rolled_back']:self.save(state='failed')
                self.save(error=type(e).__name__+': '+str(e)[:160])
            self.finish_request(request['id'])
            return
        if self.state['state']=='recovery_required':return
        now=int(time.time()*1000)
        if policy['check_minutes'] and now-self.state.get('last_check',0)>=policy['check_minutes']*60000:
            try:self.plan(policy)
            except Exception as e:self.save(state='check_failed',last_check=now,error=type(e).__name__+': '+str(e)[:160]);self.report();return
        if self.state['state']=='available' and policy['install']!='on_request':
            if policy['install']=='when_idle' or self.in_window(policy):
                try:self.apply(read(self.root/'plans'/self.state['plan_id']/'plan.json'),policy,scheduled=True)
                except Exception as e:
                    if self.state['state'] not in ['recovery_required','rolled_back']:self.save(state='failed')
                    self.save(error=type(e).__name__+': '+str(e)[:160]);self.report()
    def refresh_helper(self):
        component=next((c for c in self.c['components'] if c.get('kind')=='instance'),None)
        if component is None:return
        current=digest(component['binary'])
        if self.state.get('helper_instance_sha256')==current:return
        exported=self.root/'worker-next.py'
        run([component['binary'],'export-update-helper','--path',exported])
        same=digest(exported)==digest(Path(__file__))
        self.save(helper_instance_sha256=current)
        if same:exported.unlink();return
        target=Path(__file__);os.replace(exported,target)
        os.execv(sys.executable,[sys.executable,'-I',str(target),'--registration',str(self.root/'registration.json')])
    def serve(self):
        while True:
            try:
                self.tick()
                if self.state["state"] not in CRITICAL|{"recovery_required"}:self.refresh_helper()
            except Exception:self.save(last_error_at=int(time.time()*1000))
            time.sleep(5)

@contextlib.contextmanager
def single_worker(root):
    root.mkdir(parents=True,exist_ok=True,mode=0o700)
    with (root/'worker.lock').open('a+b') as f:
        if os.name=='nt':
            import msvcrt
            f.seek(0);f.write(b'0');f.flush();f.seek(0);msvcrt.locking(f.fileno(),msvcrt.LK_NBLCK,1)
        else:
            import fcntl
            fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB)
        yield

if __name__=='__main__':
    os.umask(0o077);p=argparse.ArgumentParser(description=__doc__);p.add_argument('--registration',required=True,type=Path);p.add_argument('--once',action='store_true');a=p.parse_args()
    w=Worker(a.registration)
    with single_worker(w.root):w.tick() if a.once else w.serve()
