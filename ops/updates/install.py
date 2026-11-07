#!/usr/bin/env python3
"""Register this installation's independent update helper (owner/admin operation)."""
import argparse, importlib.util, json, os, pathlib, platform, plistlib, shutil, subprocess, sys, uuid
from pathlib import Path

def run(*args):return subprocess.run([str(a) for a in args],check=True,capture_output=True,text=True).stdout

def enroll(home,state,connector_homes,worker,container_manifest=None,container_image=None,pull_container_image=False):
    home=home.resolve();state=state.resolve()
    if state.is_relative_to(home):raise ValueError('Place the helper outside the agent directory')
    config=json.loads((home/'config.json').read_text())['config'];identifier=str(uuid.UUID(config['instance_id']))
    spec=importlib.util.spec_from_file_location('update_worker',worker);w=importlib.util.module_from_spec(spec);spec.loader.exec_module(w)
    existing_path=state/'registration.json'
    previous=w.load_registration(existing_path) if existing_path.exists() else None
    if previous and (previous['instance_id']!=identifier or Path(previous['home']).resolve()!=home):raise ValueError('Helper directory belongs to a different installation')
    if (state/'state.json').exists() and (w.read(state/'state.json').get('state') in w.CRITICAL|{'waiting_for_idle','recovery_required'} or w.read(state/'state.json').get('transaction')):raise ValueError('Wait for the current update before changing enrollment')
    connector_homes=list(dict.fromkeys([Path(h) for h in (previous or {}).get('slack_homes',[])]+[h.resolve() for h in connector_homes]))
    services=[];components=[];system=platform.system()
    def service(folder,kind,id):
        if system=='Linux':return {'kind':'systemd-user','name':kind+'-'+id+'.service'}
        info=json.loads((folder/'install.json').read_text());return info['service']
    services.append(service(home,'agent-instance',identifier))
    components.append({'id':'instance','kind':'instance','binary':str(home/'bin'/('agent-instance.exe' if system=='Windows' else 'agent-instance'))})
    backend=(config.get('backend') or {}).get('command','').replace('${AGENT_HOME}',str(home))
    if backend and Path(backend).name in ('agent-runtime','agent-runtime.exe'):
        if not Path(backend).resolve().is_relative_to(home):raise ValueError('Runtime must belong to this installation')
        components.append({'id':'runtime','kind':'runtime','binary':backend})
    for n,folder in enumerate(connector_homes):
        folder=folder.resolve();adapter=json.loads((folder/'adapter.json').read_text())
        services.append(service(folder,'acp-slack-adapter',str(uuid.UUID(adapter['id']))))
        components.append({'id':'slack-'+str(n+1),'kind':'slack','binary':str(folder/'bin/acp-slack-adapter'),'service_name':services[-1]['name']})
    # Enroll supported upstream engines by their actual installed layout. No
    # custom engine command is guessed, replaced, or converted to a native one.
    command=Path(backend) if backend else None
    engine_kind=None;runtime_root=None
    if command:
        candidates=[('codex',home/'backend/runtime'),('claude',home/'engine/releases'),('amplifier-agent',home/'engine/releases')]
        for kind,root in candidates:
            if not command.is_relative_to(root):continue
            folder=command.relative_to(root).parts[0]
            if (kind=='codex' and (root/folder/'runtime.json').is_file()) or (kind=='claude' and command.name in ('claude-agent-acp','claude-agent-acp.exe')) or (kind=='amplifier-agent' and command.name in ('amplifier-agent-acp','amplifier-agent-acp.exe')):
                engine_kind=kind;runtime_root=root;break
    if engine_kind:
        provider_config=state/'engine.json'
        settings={'kind':engine_kind,'home':str(home),'state':str(state/'engine'),'runtime_root':str(runtime_root),'python':sys.executable,'plans_directory':str(state/'plans')}
        if engine_kind=='amplifier-agent':
            settings['uv']=shutil.which('uv')
            if not settings['uv']:raise ValueError('Install uv on this host to track Amplifier Agent updates')
        w.write(provider_config,settings)
        components.append({'id':'engine','provider':'command','protocol':2,'commands':{a:[sys.executable,'-I',str(state/'engine_provider.py'),'--config',str(provider_config),'--action',a] for a in ('status','plan','apply','rollback','verify')}})
    if container_manifest:
        manifest=w.read(container_manifest)
        if not any(Path(entry['home']).resolve()==home for entry in manifest['homes']):raise ValueError('Container does not own this installation')
        provider_config=state/'container-image.json'
        w.write(provider_config,{'manifest':str(container_manifest.resolve()),'state':str(state/'container-image'),'installer':str(state/'container/install.py'),'image':container_image or manifest['image'],'pull':pull_container_image})
        components.append({'id':'container-image','provider':'command','protocol':2,'commands':{a:[sys.executable,'-I',str(state/'container_provider.py'),'--config',str(provider_config),'--action',a] for a in ('status','plan','apply','rollback','verify')}})
    if previous:
        known={p['id'] for p in components}
        components.extend(p for p in previous['components'] if p['id'] not in known and p['id'] not in ('instance','runtime') and not (p['id'].startswith('slack-') and p.get('kind')=='slack'))
        service_names={s['name'] for s in services}
        services.extend(s for s in previous['services'] if s['name'] not in service_names)
    registration={'schema':1,'instance_id':identifier,'home':str(home),'state':str(state),'components':components,'services':services,'slack_homes':[str(h.resolve()) for h in connector_homes]}
    registration['participants']=(previous or {}).get('participants',[])
    state.mkdir(parents=True,exist_ok=True,mode=0o700)
    for p in connector_homes:
        (p/'updates').mkdir(exist_ok=True,mode=0o700);(p/'updates/admission.lock').touch(exist_ok=True)
    path=state/'registration.json';w.write(path,registration);w.load_registration(path)
    if previous!=registration and (state/'state.json').exists():
        status=w.read(state/'state.json');status.update(enrolled_at=int(w.time.time()*1000),state='idle',last_check=0,plan_id=None,rollback_plan_id=None);w.write(state/'state.json',status)
    target=state/'worker.py'
    if worker.resolve()!=target.resolve():shutil.copyfile(worker,target)
    target.chmod(0o700)
    for name in ('engine_provider.py','container_provider.py','provision-backend.py'):
        source=worker.parent/name
        if source.is_file() and source.resolve()!=(state/name).resolve():shutil.copyfile(source,state/name)

    if (worker.parent/'container').is_dir():shutil.copytree(worker.parent/'container',state/'container',dirs_exist_ok=True)
    name='constellation-updates-'+identifier
    argv=[sys.executable,'-I',str(target),'--registration',str(path)]
    if system=='Linux':
        def quote(x):return '"'+x.replace('\\','\\\\').replace('"','\\"').replace('%','%%').replace('$','$$')+'"'
        unit=Path.home()/'.config/systemd/user'/(name+'.service');unit.parent.mkdir(parents=True,exist_ok=True)
        unit.write_text('[Unit]\nDescription=Updates for one Constellation installation\nAfter=network-online.target\n[Service]\nExecStart='+' '.join(map(quote,argv))+'\nRestart=on-failure\nRestartSec=5\nUMask=0077\n[Install]\nWantedBy=default.target\n');unit.chmod(0o600)
        active=subprocess.run(['systemctl','--user','is-active','--quiet',name+'.service'],capture_output=True).returncode==0
        run('systemctl','--user','daemon-reload');run('systemctl','--user','enable','--now',name+'.service')
        if active:run('systemctl','--user','restart',name+'.service')
    elif system=='Darwin':
        label='io.'+name;domain='gui/'+str(os.getuid());path=Path.home()/'Library/LaunchAgents'/(label+'.plist');path.parent.mkdir(parents=True,exist_ok=True)
        subprocess.run(['launchctl','bootout',domain+'/'+label],capture_output=True)
        # launchd does not inherit the installer shell's PATH. Preserve the
        # owner's tool lookup so Homebrew gh remains available for release API
        # rate-limit fallback and authenticated development-build downloads.
        path.write_bytes(plistlib.dumps({'Label':label,'ProgramArguments':argv,'EnvironmentVariables':{'PATH':os.environ.get('PATH',os.defpath)},'RunAtLoad':True,'KeepAlive':True,'ThrottleInterval':5,'StandardOutPath':str(state/'stdout.log'),'StandardErrorPath':str(state/'stderr.log')}));path.chmod(0o600)
        run('launchctl','bootstrap',domain,str(path))
    elif system=='Windows':
        import base64
        encoded=base64.b64encode(json.dumps([name,sys.executable,subprocess.list2cmdline(argv[1:])]).encode()).decode()
        script="$ErrorActionPreference='Stop'; $d=ConvertFrom-Json ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('"+encoded+"'))); $action=New-ScheduledTaskAction -Execute $d[1] -Argument $d[2]; $trigger=New-ScheduledTaskTrigger -AtLogOn; $settings=New-ScheduledTaskSettingsSet -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero); Register-ScheduledTask -TaskName $d[0] -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null; Start-ScheduledTask -TaskName $d[0]"
        run('powershell.exe','-NoProfile','-NonInteractive','-EncodedCommand',base64.b64encode(script.encode('utf-16-le')).decode())
    else:raise ValueError('Unsupported platform')
    return {'installed':True,'registration':str(state/'registration.json'),'service':name}

if __name__=='__main__':
    os.umask(0o077);p=argparse.ArgumentParser(description=__doc__);p.add_argument('--home',required=True,type=Path);p.add_argument('--state',type=Path);p.add_argument('--slack-home',type=Path,action='append',default=[]);p.add_argument('--container-manifest',type=Path);p.add_argument('--container-image');p.add_argument('--pull-container-image',action='store_true');p.add_argument('--worker',type=Path,default=Path(__file__).with_name('worker.py'));a=p.parse_args()
    identity=json.loads((a.home/'config.json').read_text())['config']['instance_id']
    root=a.state or Path.home()/'.local/share/constellation-updates'/identity
    print(json.dumps(enroll(a.home,root,a.slack_home,a.worker,a.container_manifest,a.container_image,a.pull_container_image)))
