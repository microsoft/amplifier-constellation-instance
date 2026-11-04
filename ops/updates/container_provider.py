#!/usr/bin/env python3
"""Owner-enrolled image updates for an existing Constellation container.

Run outside the container. Fixed --config/--action argv; the helper supplies the
frozen plan path in the environment. No agent-supplied commands or image names.
"""
import argparse, contextlib, importlib.util, io, json, os, re, subprocess, tempfile
from pathlib import Path


def read(path): return json.loads(Path(path).read_text())
def write(path, value):
    path=Path(path); path.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
    with tempfile.NamedTemporaryFile(dir=path.parent,mode='w',delete=False) as f:
        json.dump(value,f,indent=2);f.flush();os.fsync(f.fileno());name=f.name
    os.chmod(name,0o600);os.replace(name,path)
def run(*args):
    return subprocess.run(args,check=True,capture_output=True,text=True,timeout=600).stdout.strip()
def inspect(name): return json.loads(run('docker','inspect',name))[0]
def exists(name):
    return subprocess.run(['docker','inspect',name],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0

class Provider:
    def __init__(self,config):
        self.c=read(config);self.path=Path(self.c['manifest']);self.m=read(self.path)
        self.name=self.m['container']
        self.root=Path(self.c['state']);self.root.mkdir(parents=True,exist_ok=True,mode=0o700)
        for key in ['manifest','state','installer']:
            if not Path(self.c[key]).is_absolute():raise ValueError('absolute_owner_paths_required')
        for entry in self.m['homes']:
            if self.root.resolve().is_relative_to(Path(entry['home']).resolve()):raise ValueError('provider_state_must_be_outside_agent')
    def status(self): return {'version':inspect(self.name)['Image']}
    def plan(self):
        reference=self.c['image']
        if not isinstance(reference,str) or not reference or reference.startswith('-'):raise ValueError('invalid_image_reference')
        if self.c.get('pull',False):run('docker','pull',reference)
        image=inspect(reference)['Id'];current=inspect(self.name)
        if not re.fullmatch(r'sha256:[0-9a-f]{64}',image):raise ValueError('image_digest_required')
        return {'available':image!=current['Image'],'version':image,'previous_image':current['Image'],
                'container_id':current['Id'],'manifest':self.m,'image':image}
    def context(self):
        wrapper=read(os.environ['CONSTELLATION_UPDATE_PLAN']);p=wrapper['plan']
        if p['manifest']['container']!=self.name or p['manifest']['state']!=self.m['state']:raise ValueError('wrong_container_plan')
        if not re.fullmatch(r'[0-9a-f-]{36}',wrapper['id']):raise ValueError('invalid_plan_id')
        receipt=self.root/(wrapper['id']+'.json');return p,receipt, self.name+'-previous-'+wrapper['id'][:8]
    def stop(self):
        run('systemctl','--user','stop',*self.m['services'])
        run('systemctl','--user','stop',self.name+'.service')
    def start(self):run('systemctl','--user','start',self.name+'.service',*self.m['services'])
    def apply(self):
        p,receipt,old=self.context()
        if receipt.exists() and read(receipt).get('state')=='applied':return self.verify()
        current=inspect(self.name)
        if current['Id']!=p['container_id'] or read(self.path)!=p['manifest']:raise ValueError('container_changed_since_plan')
        # Write intent first: recovery tolerates a crash before/after rename/create.
        write(receipt,{'state':'applying','previous_container':old,'expected_image':p['image']})
        self.stop();run('docker','rename',self.name,old)
        manifest=dict(p['manifest']);manifest['image']=p['image']
        spec=importlib.util.spec_from_file_location('owner_container_installer',self.c['installer'])
        installer=importlib.util.module_from_spec(spec);spec.loader.exec_module(installer)
        with contextlib.redirect_stdout(io.StringIO()):installer.install(manifest)
        write(receipt,{'state':'applied','previous_container':old,'expected_image':p['image']})
        return self.verify()
    def rollback(self):
        p,receipt,old=self.context()
        if not receipt.exists():return {'untouched':True}
        if exists(old):
            if exists(self.name):
                self.stop()
                # Retain a failed container for diagnosis; do not delete data.
                failed=self.name+'-failed-'+receipt.stem[:8]
                if exists(failed):raise ValueError('failed_container_name_conflict')
                run('docker','rename',self.name,failed)
            run('docker','rename',old,self.name)
        elif inspect(self.name)['Id']!=p['container_id']:raise ValueError('previous_container_missing')
        write(self.path,p['manifest']);self.start()
        write(receipt,{'state':'rolled_back','expected_image':p['previous_image']})
        return self.verify()
    def verify(self):
        _,receipt,_=self.context();expected=read(receipt)['expected_image'];actual=inspect(self.name)
        if actual['Image']!=expected or not actual['State']['Running']:raise ValueError('container_image_or_health_mismatch')
        return {'version':expected,'healthy':True}

if __name__=='__main__':
    os.umask(0o077);p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--config',required=True);p.add_argument('--action',choices=['status','plan','apply','rollback','verify'],required=True)
    a=p.parse_args();print(json.dumps(getattr(Provider(a.config),a.action)()))
