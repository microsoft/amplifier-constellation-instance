#!/usr/bin/env python3
"""Update an owner-enrolled vanilla ACP engine without changing its identity.

Official releases are staged before idle cutover. No prompts, credentials,
models, session files, engine forks, or global package installation are involved.
"""
import argparse, hashlib, importlib.util, json, os, platform, re, shutil, subprocess, sys, tempfile, urllib.request, uuid
from pathlib import Path

def module(name,path):
    spec=importlib.util.spec_from_file_location(name,path);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
w=module('update_worker',Path(__file__).with_name('worker.py'))

def tree_digest(root):
    h=hashlib.sha256()
    for path in sorted(Path(root).rglob('*')):
        relative=path.relative_to(root)
        if '__pycache__' in relative.parts or path.name=='engine-update.json':continue
        if path.is_symlink():value='link:'+os.readlink(path)
        elif path.is_file():value='file:'+w.digest(path)
        else:continue
        h.update((str(relative)+'\0'+value+'\n').encode())
    return h.hexdigest()

def remap(value,old,new):
    if isinstance(value,str):return value.replace(old,new)
    if isinstance(value,list):return [remap(x,old,new) for x in value]
    if isinstance(value,dict):return {k:remap(v,old,new) for k,v in value.items()}
    return value

def release_asset(asset,dest,repo):
    if not asset['browser_download_url'].startswith('https://github.com/'+repo+'/releases/download/') or not re.fullmatch('sha256:[0-9a-f]{64}',asset.get('digest','')):raise ValueError('verified_engine_asset_required')
    with urllib.request.urlopen(asset['browser_download_url'],timeout=90) as response,dest.open('wb') as out:
        size=0
        while block:=response.read(1024*1024):
            size+=len(block)
            if size>1024*1024*1024:raise ValueError('engine_asset_limit')
            out.write(block)
    if 'sha256:'+w.digest(dest)!=asset['digest']:raise ValueError('engine_asset_digest_mismatch')

class Provider:
    def __init__(self,config):
        self.config_digest=w.digest(config);self.c=w.read(config);self.home=Path(self.c['home']);self.root=Path(self.c['state']);self.runtime=Path(self.c['runtime_root']);self.kind=self.c['kind']
        if self.kind not in ('codex','claude','amplifier-agent'):raise ValueError('unsupported_engine')
        if not all(p.is_absolute() for p in (self.home,self.root,self.runtime)):raise ValueError('absolute_owner_paths_required')
        if self.root.resolve().is_relative_to(self.home.resolve()) or not self.runtime.resolve().is_relative_to(self.home.resolve()):raise ValueError('invalid_engine_provider_boundary')
        self.root.mkdir(parents=True,exist_ok=True,mode=0o700)
    def current(self):
        cfg=w.read(self.home/'config.json');backend=cfg['config']['backend'];command=Path(backend['command'].replace('${AGENT_HOME}',str(self.home)))
        relative=command.relative_to(self.runtime)
        if len(relative.parts)<2:raise ValueError('engine_is_not_versioned')
        folder=self.runtime/relative.parts[0]
        if (folder/'engine-update.json').exists():
            record=w.read(folder/'engine-update.json')
            if record['kind']!=self.kind:raise ValueError('engine_family_changed')
            v=record['version']
        elif self.kind=='codex':v=w.read(folder/'runtime.json')['bundle']
        else:
            match=re.fullmatch(('claude-agent-acp' if self.kind=='claude' else 'amplifier-agent')+r'-(\d+\.\d+\.\d+)',folder.name)
            if not match:raise ValueError('unrecognized_installed_engine')
            v=match[1]
        return cfg,backend,folder,v
    def status(self):
        _,_,_,v=self.current();return {'version':v,'engine':self.kind}
    def discover(self,current):
        if self.kind!='codex':
            repo='agentclientprotocol/claude-agent-acp' if self.kind=='claude' else 'microsoft/amplifier-agent'
            r=w.gh('repos/'+repo+'/releases/latest');v=r['tag_name'].removeprefix('v')
            if not re.fullmatch(r'\d+\.\d+\.\d+',v):raise ValueError('stable_engine_release_required')
            if w.version(v)<=w.version(current):return None
            result={'version':v,'repository':repo,'tag':r['tag_name'],'url':r['html_url']}
            if self.kind=='claude':
                target={'Linux':'linux','Darwin':'darwin','Windows':'windows'}[platform.system()]+'-'+{'aarch64':'arm64','arm64':'arm64','x86_64':'x64','AMD64':'x64'}[platform.machine()]
                name='claude-agent-acp-'+target+('.tar.gz' if platform.system()=='Linux' else '.zip')
                result['asset']=next(a for a in r['assets'] if a['name']==name)
            else:
                ref=w.gh('repos/'+repo+'/commits/'+r['tag_name']);result['commit']=ref['sha']
                if not re.fullmatch('[0-9a-f]{40}',result['commit']):raise ValueError('release_commit_required')
            return result
        # The reviewed installer still owns extraction/version validation. Resolve
        # only official stable npm versions and update patches within the enrolled Node LTS line.
        p=module('provision_engine',Path(__file__).with_name('provision-backend.py'))
        codex=w.http_json('https://registry.npmjs.org/@openai%2Fcodex/latest')
        acp=w.http_json('https://registry.npmjs.org/@agentclientprotocol%2Fcodex-acp/latest')
        key=p.platform_key();old=w.read(self.current()[2]/'runtime.json')
        cv,av=codex['version'],acp['version'];nv=old['node_version']
        node_releases=w.http_json('https://nodejs.org/dist/index.json')
        same_major=[r['version'].removeprefix('v') for r in node_releases if r['version'].startswith('v'+nv.split('.')[0]+'.') and r.get('lts')]
        if same_major:nv=max([nv,*same_major],key=w.version)
        if any(not re.fullmatch(r'\d+\.\d+\.\d+',v) for v in (cv,av,nv)):raise ValueError('stable_engine_release_required')
        if w.version(cv)<=w.version(old['codex_version']) and w.version(av)<=w.version(old['acp_version']) and w.version(nv)<=w.version(old['node_version']):return None
        if w.version(cv)<w.version(old['codex_version']) or w.version(av)<w.version(old['acp_version']):raise ValueError('engine_downgrade_refused')
        package=w.http_json('https://registry.npmjs.org/@openai%2Fcodex/'+cv+'-'+key)
        node_name='node-v'+nv+'-'+key.replace('win32','win')+('.zip' if os.name=='nt' else '.tar.gz')
        with urllib.request.urlopen('https://nodejs.org/dist/v'+nv+'/SHASUMS256.txt',timeout=30) as f:checks=f.read(65536).decode().splitlines()
        checksum=next(line.split()[0] for line in checks if line.split()[-1]==node_name)
        target={'linux-arm64':'aarch64-unknown-linux-musl','linux-x64':'x86_64-unknown-linux-musl','darwin-arm64':'aarch64-apple-darwin','darwin-x64':'x86_64-apple-darwin','win32-x64':'x86_64-pc-windows-msvc','win32-arm64':'aarch64-pc-windows-msvc'}[key]
        def dist(x):return {'url':x['dist']['tarball'],'integrity':x['dist']['integrity']}
        bundle=f'codex-{cv}-acp-{av}-node-{nv}'
        lock={'schema_version':1,'bundle':bundle,'codex_version':cv,'acp_version':av,'node_version':nv,'acp':dist(acp),'platforms':{key:{'target':target,'node':{'url':'https://nodejs.org/dist/v'+nv+'/'+node_name,'integrity':'sha256-'+checksum},'codex':dist(package)}}}
        return {'version':bundle,'repository':'agentclientprotocol/codex-acp','lock':lock}
    def stage(self,release,destination):
        scratch=self.root/('scratch-'+uuid.uuid4().hex);scratch.mkdir(mode=0o700)
        try:
            if self.kind=='claude':
                archive=scratch/'release';release_asset(release['asset'],archive,release['repository'])
                for name in ('claude-agent-acp','claude'):
                    if os.name=='nt':name+='.exe'
                    target=destination/name;target.write_bytes(w.extract(archive.read_bytes(),name,limit=512*1024*1024));target.chmod(0o755)
                return str(destination/('claude-agent-acp.exe' if os.name=='nt' else 'claude-agent-acp'))
            if self.kind=='codex':
                p=module('provision_engine',Path(__file__).with_name('provision-backend.py'));w.write(scratch/'config.json',{})
                p.provision(scratch,release['lock'],False)
                built=scratch/'backend/runtime'/release['lock']['bundle']
                for item in built.iterdir():shutil.move(str(item),str(destination/item.name))
                return str(destination/'node/bin'/('node.exe' if os.name=='nt' else 'node'))
            uv=self.c.get('uv')
            if not uv or not Path(uv).is_absolute():raise ValueError('owner_uv_path_required')
            python=self.c.get('python',sys.executable)
            # Exact upstream source commit, including its own declared dependency
            # graph. Stage once; apply never resolves or installs anything.
            env={**os.environ,'UV_CACHE_DIR':str(scratch/'cache'),'PYTHONDONTWRITEBYTECODE':'1','UV_PYTHON_DOWNLOADS':'never'}
            w.run([uv,'venv','--python',python,str(destination)],timeout=120,env=env)
            executable=destination/('Scripts/python.exe' if os.name=='nt' else 'bin/python')
            requirement='git+https://github.com/microsoft/amplifier-agent@'+release['commit']+'#subdirectory=packages/acp'
            w.run([uv,'pip','install','--python',str(executable),requirement],timeout=600,env=env)
            w.run([uv,'pip','check','--python',str(executable)],timeout=60,env=env)
            installed=json.loads(w.run([str(executable),'-c',"import importlib.metadata as m,json;print(json.dumps({n:m.version(n) for n in ['amplifier-agent-acp','amplifier-agent-engine','amplifier-agent']}))"],env=env))
            if set(installed.values())!={release['version']}:raise ValueError('upstream_engine_version_mismatch')
            w.write(destination/'upstream-packages.json',json.loads(w.run([uv,'pip','list','--python',str(executable),'--format','json'],env=env)))
            return str(destination/('Scripts/amplifier-agent-acp.exe' if os.name=='nt' else 'bin/amplifier-agent-acp'))
        finally:shutil.rmtree(scratch)
    def prune_staged(self):
        # Only provider-created directories unreferenced by retained plans. Never
        # delete an owner baseline, current executable, or rollback candidate.
        plans=self.c.get('plans_directory')
        if not plans:return
        current=self.current()[2];keep={str(current)}
        for file in Path(plans).glob('*/engine.provider.json'):
            saved=w.read(file).get('plan',{})
            keep.update(saved.get(k,'') for k in ('candidate','previous'))
        for folder in self.runtime.glob('update-*'):
            if folder.is_symlink() or not folder.is_dir() or str(folder) in keep:continue
            marker=folder/'engine-update.json'
            if marker.is_file() and w.read(marker).get('kind')==self.kind:shutil.rmtree(folder)
    def plan(self):
        self.prune_staged()
        cfg,backend,old,current=self.current();release=self.discover(current)
        if release is None:return {'available':False,'version':current}
        destination=self.runtime/('update-'+release['version']+'-'+uuid.uuid4().hex[:8]);destination.mkdir(parents=True,mode=0o700)
        try:
            command=self.stage(release,destination)
            old_token=str(old) if not backend['command'].startswith('${AGENT_HOME}') else '${AGENT_HOME}/'+str(old.relative_to(self.home))
            new_token=str(destination) if not backend['command'].startswith('${AGENT_HOME}') else '${AGENT_HOME}/'+str(destination.relative_to(self.home))
            after=remap(backend,old_token,new_token)
            if Path(after['command'].replace('${AGENT_HOME}',str(self.home)))!=Path(command):raise ValueError('engine_command_layout_changed')
            w.write(destination/'engine-update.json',{'kind':self.kind,'version':release['version'],'source':release})
            return {'available':True,'config_sha256':self.config_digest,'version':release['version'],'previous_version':current,'kind':self.kind,'candidate':str(destination),'sha256':tree_digest(destination),'previous':str(old),'previous_sha256':tree_digest(old),'backend_before':backend,'backend_after':after,'source':release}
        except BaseException:shutil.rmtree(destination);raise
    def context(self):
        wrapper=w.read(os.environ['CONSTELLATION_UPDATE_PLAN']);p=wrapper['plan'];identifier=str(uuid.UUID(wrapper['id']))
        if p['kind']!=self.kind:raise ValueError('wrong_engine_plan')
        if p.get('config_sha256',self.config_digest)!=self.config_digest:raise ValueError('engine_enrollment_changed')
        for key in ('candidate','previous'):
            if Path(p[key]).parent!=self.runtime:raise ValueError('engine_plan_outside_installation')
        return p,self.root/(identifier+'.json')
    def switch(self,rollback=False):
        p,receipt=self.context();target='backend_before' if rollback else 'backend_after';old='backend_after' if rollback else 'backend_before'
        cfg=w.read(self.home/'config.json')
        if cfg['config']['backend'] not in (p[old],p[target]):raise ValueError('backend_changed_since_plan')
        folder=Path(p['previous' if rollback else 'candidate']);expected=p['previous_sha256' if rollback else 'sha256']
        if tree_digest(folder)!=expected:raise ValueError('staged_engine_changed')
        w.write(receipt,{'state':'switching','expected':target})
        if cfg['config']['backend']!=p[target]:
            cfg['config']['backend']=p[target];cfg['revision']+=1;w.write(self.home/'config.json',cfg)
        w.write(receipt,{'state':'rolled_back' if rollback else 'applied','expected':target})
        return self.verify()
    def apply(self):return self.switch()
    def rollback(self):return self.switch(True)
    def verify(self):
        p,receipt=self.context();target=w.read(receipt)['expected'];before=target=='backend_before'
        if w.read(self.home/'config.json')['config']['backend']!=p[target]:raise ValueError('engine_binding_mismatch')
        if tree_digest(Path(p['previous' if before else 'candidate']))!=p['previous_sha256' if before else 'sha256']:raise ValueError('engine_installation_mismatch')
        return {'healthy':True,'version':p['previous_version' if before else 'version']}

if __name__=='__main__':
    os.umask(0o077);parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--config',required=True);parser.add_argument('--action',required=True,choices=['status','plan','apply','rollback','verify']);args=parser.parse_args()
    print(json.dumps(getattr(Provider(args.config),args.action)()))
