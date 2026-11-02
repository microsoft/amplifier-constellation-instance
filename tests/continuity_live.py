#!/usr/bin/env python3
"""Explicit opt-in live experiment. Disposable homes; selected source key is copied privately.
Usage: python3 tests/continuity_live.py --source-home /path/to/native-workshop --report-dir /private/path
No messages are sent to external conversations. The paid model is used only for a local artifact.
"""
import argparse,hashlib,json,os,pathlib,shutil,ssl,subprocess,tempfile,time,urllib.request,urllib.error,uuid
ROOT=pathlib.Path(__file__).resolve().parents[1]
SSL=ssl._create_unverified_context()  # Private loopback fixture certificates only.
class Instance:
 def __init__(self,root,name,source=None):
  self.home=root/name;self.process=None
  subprocess.run([str(ROOT/'target/debug/agent-instance'),'--home',str(self.home),'init','--name',name,'--bind','127.0.0.1','--port','0'],check=True,stdout=subprocess.DEVNULL)
  document=json.loads((self.home/'config.json').read_text());document['config']['setup_complete']=True;document['config']['enabled']=True
  document['config']['backend']={'command':'/usr/bin/python3','args':[str(ROOT/'tests/fixtures/scheduler_backend.py'),'${AGENT_HOME}/workspace'],'env':{},'secret_env':{},'working_directory':'${AGENT_HOME}/workspace','allowed_working_roots':['${AGENT_HOME}/workspace']}
  if source:
   original=json.loads((source/'config.json').read_text())['config']['backend']
   binary=pathlib.Path(original['command'].replace('${AGENT_HOME}',str(source)))
   engine=self.home/'engine';engine.mkdir(mode=0o700)
   shutil.copy2(binary,engine/'agent-runtime')
   runtime=json.loads((source/'engine/runtime.json').read_text());runtime.update(name=name,instructions='You are a local software engineering participant in a controlled experiment. Build and check the requested artifact within the authorized workspace.',workspace=str(self.home/'workspace'),mode='workspace',decisions=[])
   runtime['provider']['native']={};runtime['allowed_mcp_mutations']=[];runtime['trusted_mcp_readonly_urls']=[];runtime['allow_client_stdio_mcp']=False
   runtime['limits'].update(max_model_calls=16,max_tool_calls=40,turn_seconds=180,max_output_tokens=7000)
   private(engine/'runtime.json',json.dumps(runtime));key_env=runtime['provider']['api_key_env'];key_name=original['secret_env'][key_env]
   private(self.home/'secrets/experiment-model-key',(source/'secrets'/key_name).read_bytes())
   document['config']['backend']={'command':'${AGENT_HOME}/engine/agent-runtime','args':['--home','${AGENT_HOME}/engine','acp'],'env':{},'secret_env':{key_env:'experiment-model-key'},'working_directory':'${AGENT_HOME}/workspace','allowed_working_roots':['${AGENT_HOME}/workspace']}
  private(self.home/'config.json',json.dumps(document));self.id=document['config']['instance_id'];self.token=(self.home/'secrets/control-token').read_text().strip()
  self.log=(self.home/'service.log').open('w');self.process=subprocess.Popen([str(ROOT/'target/debug/agent-instance'),'--home',str(self.home),'serve'],stdout=subprocess.DEVNULL,stderr=self.log)
  for _ in range(100):
   if (self.home/'endpoints.json').exists():self.url=json.loads((self.home/'endpoints.json').read_text())['base_url'];break
   if self.process.poll() is not None:raise RuntimeError('Fixture service failed: '+(self.home/'service.log').read_text()[-2000:])
   time.sleep(.05)
  else:raise RuntimeError('Fixture startup timed out')
 def api(self,method,path,body=None,token=None,expected=200):
  req=urllib.request.Request(self.url+'/api/v1/'+path,data=None if body is None else json.dumps(body).encode(),method=method,headers={'Authorization':'Bearer '+(token or self.token),'Content-Type':'application/json'})
  try:
   with urllib.request.urlopen(req,context=SSL,timeout=25) as r:status=r.status;data=json.load(r)
  except urllib.error.HTTPError as e:status=e.code;data=json.load(e)
  if status!=expected:raise AssertionError(f'{method} {path}: status {status}; expected {expected}; {data}')
  return data
 def command(self,w,command):return self.api('POST','work/'+w['id']+'/commands',{'expected_revision':w['revision'],'command':command})
 def wait(self,wid,rid,terminal=True,timeout=400):
  end=time.time()+timeout
  while time.time()<end:
   r=next(r for r in self.api('GET','work/'+wid+'/execution')['runs'] if r['id']==rid)
   if (terminal and r['state'] not in ('queued','starting','running','cancelling')) or (not terminal and r['state']=='running'):return r
   time.sleep(.15)
  raise AssertionError('Timed out waiting for background work')
 def stop(self):
  if self.process:
   self.process.terminate()
   try:self.process.wait(timeout=8)
   except subprocess.TimeoutExpired:self.process.kill();self.process.wait(timeout=4)
  self.log.close()
def private(path,value):
 path.write_bytes(value if isinstance(value,bytes) else value.encode());path.chmod(0o600)
def organization(instance,peer,send,receive):
 return instance.api('PUT','organization',{'expected_revision':0,'organization':{'templates':[],'assignments':[{'id':'domain','name':'Domain responsibility','domain':'test','purpose':'Keep this domain independent','template_id':None,'audience':{'kind':'owner'},'active':True}],'relationships':[{'id':'coordinator','label':'Explicit relationship','peer_agent_id':peer,'assignment_ids':['domain'],'send':send,'receive':receive,'allow_details':False,'active':True}]}})
def boundary(root,instances):
 created=[]
 for n in ['work','personal','coordinator']:
  agent=Instance(root,n);instances.append(agent);created.append(agent)
 work,personal,coordinator=created
 for agent,statement in [(work,'Private release problem'),(personal,'Private family detail')]:agent.api('POST','memory',{'key':'private topic','statement':statement,'source_note':'Synthetic private fixture'})
 work.api('GET','memory',token=personal.token,expected=401);personal.api('GET','memory',token=work.token,expected=401);assert coordinator.api('GET','memory')['items']==[]
 organization(work,coordinator.id,True,False);organization(coordinator,work.id,False,True)
 responsibility=work.api('POST','work',{'title':'Private release responsibility','goal':'Private detailed target must stay here','assignment_id':'domain','audience':{'kind':'owner'}})
 exported=work.api('POST','work/'+responsibility['id']+'/export',{'expected_revision':responsibility['revision'],'relationship_id':'coordinator','disclosure':'summary','summary':'Reserve one hour for release work.'})
 packet=exported.get('packet',exported.get('envelope',exported))
 assert 'Private detailed target' not in json.dumps(packet) and 'Private release problem' not in json.dumps(packet)
 imported=coordinator.api('POST','work/import',{'relationship_id':'coordinator','assignment_id':'domain','packet':packet})
 assert imported['work']['state']=='proposed';assert coordinator.api('GET','memory')['items']==[];assert personal.api('GET','work')['items']==[]
 first=work.api('POST','memory',{'key':'release date','statement':'Monday','source_note':'Synthetic initial decision'})
 conflict=work.api('POST','memory',{'key':'release date','statement':'Tuesday','source_note':'Synthetic correction'})
 assert first['id']==conflict['id'] and conflict['state']=='conflicted' and conflict['selected_candidate_id'] is None
 stale=work.api('POST','memory/'+first['id']+'/commands',{'expected_revision':1,'command':{'type':'resolve','candidate_id':conflict['candidates'][1]['id'],'reason':'Confirmed correction'}},expected=409)
 resolved=work.api('POST','memory/'+first['id']+'/commands',{'expected_revision':conflict['revision'],'command':{'type':'resolve','candidate_id':conflict['candidates'][1]['id'],'reason':'Confirmed correction'}})
 assert resolved['state']=='active'
 return {'independent_instances':3,'lateral_tokens_rejected':True,'coordinator_received_only_explicit_summary':True,'coordinator_memory_unchanged':True,'personal_work_unchanged':True,'conflicts_preserved':True,'stale_resolution_rejected':True,'explicit_resolution_recorded':True}
GOAL='''Build a small dependency-free Python product called Work Digest in this workspace. Deliver work_digest.py with summarize(items), and a command-line interface reading a JSON array from stdin and printing the summary as JSON. Each item is an object with nonempty string id, nonempty string title, and state from proposed, accepted, in_progress, blocked, review, completed, cancelled. IDs must be unique. Reject invalid input with ValueError from summarize and a nonzero CLI exit. Return exactly {"total":N,"counts":{all seven state names:integer},"needs_attention":[IDs of blocked and review items in their original order]}. Empty input is valid. Do not mutate input.
Use two checkpoints so continuity can be examined. If PLAN.md is absent, the first turn must write only a concise PLAN.md describing the interface and acceptance cases, then stop with AGENT_WORK_STATUS {"status":"continue","summary":"Plan saved; implement Work Digest next"}. If PLAN.md exists and the implementation is missing or incomplete, implement the product, add useful tests, run them, and return review only when your own checks pass. Do not repeat the planning-only checkpoint when its file already exists. Work only locally. No external messages or unrelated changes. The final artifact will be checked by a separate process.'''
def verify(workspace):
 program=r'''
import copy,importlib.util,json,pathlib,subprocess,sys
p=pathlib.Path(sys.argv[1]);spec=importlib.util.spec_from_file_location('digest',p/'work_digest.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
states=['proposed','accepted','in_progress','blocked','review','completed','cancelled'];passed=0
def check(condition):
 global passed
 assert condition
 passed+=1
for items in [[],[{'id':'a','title':'A','state':'blocked'}],[{'id':str(i),'title':s,'state':s} for i,s in enumerate(states)]]:
 before=copy.deepcopy(items);expected={'total':len(items),'counts':{s:sum(x['state']==s for x in items) for s in states},'needs_attention':[x['id'] for x in items if x['state'] in ('blocked','review')]}
 check(m.summarize(items)==expected);check(items==before)
 cli=subprocess.run([sys.executable,str(p/'work_digest.py')],input=json.dumps(items),text=True,capture_output=True);check(cli.returncode==0 and json.loads(cli.stdout)==expected)
for bad in [None,{},'bad',[None],[{}],[{'id':'a','title':'A','state':'bogus'}],[{'id':'','title':'A','state':'review'}],[{'id':'a','title':'','state':'review'}],[{'id':1,'title':'A','state':'review'}],[{'id':'a','title':'A','state':'review'}]*2]:
 try:m.summarize(bad)
 except ValueError:passed+=1
 else:raise AssertionError('Invalid input was accepted')
 cli=subprocess.run([sys.executable,str(p/'work_digest.py')],input=json.dumps(bad),text=True,capture_output=True);check(cli.returncode!=0)
print(json.dumps({'independent_checks':passed,'passed':True}))
'''
 r=subprocess.run(['python3','-c',program,str(workspace)],text=True,capture_output=True,timeout=20)
 if r.returncode:raise AssertionError('Independent artifact checks failed: '+r.stderr[-4000:])
 return json.loads(r.stdout)
def product(root,source,instances,reportdir):
 instance=Instance(root,'product-worker',source);instances.append(instance)
 settings=instance.api('GET','scheduler');instance.api('PUT','scheduler',{'expected_revision':settings['settings']['revision'],'enabled':True,'paused':False,'concurrency':1,'approved_backend_sha256':settings['backend_sha256'],'session_mode':'workspace'})
 w=instance.api('POST','work',{'title':'Work Digest continuity experiment','goal':GOAL});w=instance.command(w,{'type':'accept'})
 budget={'max_seconds':350,'max_output_bytes':524288,'max_tool_calls':60,'max_turns':2}
 initial=instance.api('POST','work/'+w['id']+'/execution',{'type':'queue','expected_work_revision':w['revision'],'request_id':'interrupt-probe','budget':budget});active=instance.wait(w['id'],initial['id'],terminal=False)
 # Cancel an actual started prompt; no automatic replay is permitted.
 current=next(r for r in instance.api('GET','work/'+w['id']+'/execution')['runs'] if r['id']==active['id'])
 instance.api('POST','work/'+w['id']+'/execution',{'type':'cancel','run_id':current['id'],'expected_revision':current['revision'],'reason':'Controlled interruption; inspect before continuing'})
 interrupted=instance.wait(w['id'],initial['id']);assert interrupted['state']=='needs_reconciliation'
 workspace=instance.home/'workspace';inspection=[str(p.relative_to(workspace)) for p in workspace.rglob('*') if p.is_file()]
 reconciled=instance.api('POST','work/'+w['id']+'/execution',{'type':'reconcile','run_id':interrupted['id'],'expected_revision':interrupted['revision'],'note':'Independent operator inspected the bounded experiment workspace. Existing files: '+json.dumps(inspection)+'. The goal is unchanged; a fresh attempt may inspect and continue local files.','allow_retry':True})
 w=instance.api('GET','work/'+w['id']);budget['max_turns']=1
 retry=instance.api('POST','work/'+w['id']+'/execution',{'type':'retry','run_id':reconciled['id'],'expected_revision':reconciled['revision'],'expected_work_revision':w['revision'],'request_id':'planning-checkpoint','reason':'Continue after inspecting the interrupted attempt','budget':budget})
 planned=instance.wait(w['id'],retry['id']);private(reportdir/'planning-run.json',json.dumps(planned,indent=2));assert planned['state']=='paused_limit',planned['state']+': '+str(planned.get('reason'));assert (workspace/'PLAN.md').is_file();assert not (workspace/'work_digest.py').exists()
 w=instance.api('GET','work/'+w['id']);budget['max_turns']=3
 resumed=instance.api('POST','work/'+w['id']+'/execution',{'type':'resume','run_id':planned['id'],'expected_revision':planned['revision'],'expected_work_revision':w['revision'],'request_id':'implementation-checkpoint','reason':'The plan exists; implement and check the artifact using the same working session','budget':budget})
 done=instance.wait(w['id'],resumed['id']);private(reportdir/'implementation-run.json',json.dumps(done,indent=2));assert done['state']=='reported',done['state']+': '+str(done.get('reason'));assert done['session_id']==planned['session_id'];assert done['execution_chain_id']==planned['execution_chain_id']
 result=verify(workspace);w=instance.api('GET','work/'+w['id']);assert w['state']=='review' and w['verification'] is None
 artifacts=reportdir/'artifact';artifacts.mkdir();
 for p in workspace.iterdir():
  if p.is_file():shutil.copy2(p,artifacts/p.name)
 return {'real_model':True,'engine':'native Rust 0.3.0 over ordinary ACP','interruption_state':interrupted['state'],'automatic_replay':False,'operator_reconciliation':True,'first_checkpoint':planned['state'],'same_session_resumed':True,'turns_completed':planned['turns_completed']+done['turns_completed'],'tool_calls':planned['tool_calls']+done['tool_calls'],'final_work_state':w['state'],'model_report_not_completion':True,'verification':result,'artifact_sha256':hashlib.sha256((workspace/'work_digest.py').read_bytes()).hexdigest()}
def main():
 parser=argparse.ArgumentParser();parser.add_argument('--source-home',type=pathlib.Path,required=True);parser.add_argument('--report-dir',type=pathlib.Path,required=True);args=parser.parse_args();args.report_dir.mkdir(parents=True,exist_ok=True,mode=0o700)
 instances=[];report={'started_at':time.time(),'scope':'Disposable isolated homes, no external conversation messages'}
 with tempfile.TemporaryDirectory(prefix='agent-continuity-live-') as temp:
  try:
   report['boundaries']=boundary(pathlib.Path(temp),instances);print('Boundary scenario passed',flush=True)
   report['product']=product(pathlib.Path(temp),args.source_home,instances,args.report_dir);report['passed']=True
  finally:
   for i in instances:i.stop()
   private(args.report_dir/'result.json',json.dumps(report,indent=2))
 print(json.dumps(report,indent=2))
if __name__=='__main__':main()
