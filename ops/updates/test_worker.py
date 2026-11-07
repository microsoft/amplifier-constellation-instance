import contextlib, importlib.util, io, json, pathlib, sqlite3, subprocess, sys, tarfile, tempfile, time, unittest, uuid, zipfile
from unittest.mock import patch
ROOT=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('worker',ROOT/'worker.py');w=importlib.util.module_from_spec(spec);spec.loader.exec_module(w)

class UpdateTests(unittest.TestCase):
 def setUp(self):
  self.t=tempfile.TemporaryDirectory();self.addCleanup(self.t.cleanup);self.root=pathlib.Path(self.t.name);self.home=self.root/'agent';self.home.mkdir();self.binary=self.home/'agent-instance';self.binary.write_bytes(b'old-binary');self.state=self.root/'host';self.state.mkdir()
  self.registration={'schema':1,'instance_id':'fixture','home':str(self.home),'state':str(self.state),'components':[{'id':'instance','kind':'instance','binary':str(self.binary)}],'services':[]}
  self.path=self.root/'registration.json';w.write(self.path,self.registration)
 def worker(self):
  with patch.object(w.ssl,'create_default_context'):obj=w.Worker(self.path)
  obj.policy={'revision':0,'agent_initiated':True,'check_minutes':60,'channel':'stable','install':'on_request','window_start_utc':2,'window_hours':2}
  obj.api=lambda *args,**kwargs:{'policy':obj.policy,'active_prompts':0}
  obj.report=lambda *args,**kwargs:{'policy':obj.policy,'active_prompts':0}
  obj.control=lambda action,scope=None:None
  obj.service_snapshot=lambda scope:{name:True for name in scope['services']}
  obj.health=lambda expected,scope=None:None
  return obj
 def plan(self,obj):
  folder=self.state/'plans'/'123';folder.mkdir(parents=True);(folder/'instance.candidate').write_bytes(b'new-binary')
  p={'id':'123','policy_revision':0,'components':[{'id':'instance','available':True,'installed_sha256':w.digest(self.binary),'sha256':w.digest(folder/'instance.candidate')}]};w.write(folder/'plan.json',p);return p
 def test_helper_state_cannot_be_inside_agent(self):
  self.registration['state']=str(self.home/'helper');w.write(self.path,self.registration)
  with self.assertRaisesRegex(ValueError,'outside'):w.load_registration(self.path)
 def test_verification_helper_bootstrap_is_not_replaced_by_older_instance(self):
  obj=self.worker()
  for installed in ('0.19.0','0.20.0'):
   with self.subTest(installed=installed),patch.object(w,'run',return_value=('agent-instance '+installed).encode()) as run:
    obj.state.pop('helper_instance_sha256',None);obj.refresh_helper()
    run.assert_called_once_with([str(self.binary),'--version'])
    self.assertEqual(obj.state['helper_instance_sha256'],w.digest(self.binary))
 def test_stable_github_tags_match_installed_versions(self):
  for tag in ['v0.14.0','v0.12.0']:
   with self.subTest(tag=tag),patch.object(w,'gh',return_value={'tag_name':tag}),patch.object(w,'run',return_value=b'agent-instance 0.14.0'),patch.object(w,'download') as download:
    result=w.candidate(self.registration['components'][0],'stable',self.state)
    self.assertFalse(result['available']);download.assert_not_called()
 def test_newer_prefixed_release_prepares_verified_candidate(self):
  blob=io.BytesIO()
  with zipfile.ZipFile(blob,'w') as z:z.writestr('agent-instance',b'new-binary')
  release={'tag_name':'v0.15.0','html_url':'https://github.com/microsoft/amplifier-constellation-instance/releases/tag/v0.15.0','assets':[{'name':'agent-instance-aarch64-unknown-linux-gnu.tar.gz','browser_download_url':'https://github.com/microsoft/amplifier-constellation-instance/releases/download/v0.15.0/binary','digest':'sha256:'+'a'*64}]}
  def download(url,dest,digest):dest.write_bytes(blob.getvalue())
  with patch.object(w,'platform_names',return_value=('Linux','aarch64','aarch64-unknown-linux-gnu')),patch.object(w,'gh',return_value=release),patch.object(w,'run',side_effect=[b'agent-instance 0.14.0',b'agent-instance 0.15.0']),patch.object(w,'download',side_effect=download) as fetch:
   result=w.candidate(self.registration['components'][0],'stable',self.state)
   self.assertTrue(result['available']);self.assertEqual(result['tag'],'v0.15.0');self.assertEqual(fetch.call_args.args[2],release['assets'][0]['digest'])
   self.assertEqual((self.state/'instance.candidate').read_bytes(),b'new-binary')
 def test_apply_and_manual_rollback_preserve_data(self):
  obj=self.worker();plan=self.plan(obj);(self.home/'conversation.json').write_text('keep new messages')
  obj.apply(plan,obj.policy,True);self.assertEqual(self.binary.read_bytes(),b'new-binary');self.assertEqual(obj.state['state'],'succeeded')
  obj.apply(plan,obj.policy,True,True);self.assertEqual(self.binary.read_bytes(),b'old-binary');self.assertEqual(obj.state['state'],'rolled_back');self.assertEqual((self.home/'conversation.json').read_text(),'keep new messages')
 def test_verified_inventory_tracks_install_and_rollback_without_mutating_plan(self):
  obj=self.worker();plan=self.plan(obj)
  plan['components'][0].update(version='agent-instance 1.0.0',target_version='agent-instance 1.1.0')
  image={'id':'image','provider':'command','available':True,'version':'image-old','target_version':'image-new'}
  plan['components'].append(image)
  obj.save(components=json.loads(json.dumps(plan['components'])))
  original=json.loads(json.dumps(plan['components']))
  obj.apply(plan,obj.policy)
  installed=obj.state['components']
  self.assertEqual(installed[0]['version'],'agent-instance 1.1.0')
  self.assertEqual(installed[0]['installed_sha256'],w.digest(self.binary))
  self.assertFalse(installed[0]['available'])
  self.assertEqual(installed[1]['version'],'image-new');self.assertFalse(installed[1]['available'])
  self.assertEqual(plan['components'],original)
  obj.apply(plan,obj.policy,rollback=True)
  self.assertEqual(obj.state['components'][0]['version'],'agent-instance 1.0.0')
  self.assertEqual(obj.state['components'][0]['installed_sha256'],w.digest(self.binary))
  self.assertTrue(obj.state['components'][0]['available'])
  self.assertEqual(obj.state['components'][1]['version'],'image-old')
  self.assertEqual(plan['components'],original)
 def test_health_failure_restores_previous_binary(self):
  obj=self.worker();plan=self.plan(obj);calls=[]
  def health(expected,scope=None):
   calls.append(expected)
   if len(calls)==1:raise ValueError('new version failed')
  obj.health=health
  with self.assertRaises(ValueError):obj.apply(plan,obj.policy)
  self.assertEqual(len(calls),2);self.assertEqual(self.binary.read_bytes(),b'old-binary');self.assertEqual(obj.state['state'],'rolled_back')
 def test_revocation_prevents_cutover(self):
  obj=self.worker();plan=self.plan(obj);obj.policy['agent_initiated']=False
  with self.assertRaisesRegex(ValueError,'policy_changed'):obj.apply(plan,obj.policy,True)
  self.assertEqual(self.binary.read_bytes(),b'old-binary')
 def test_tampered_candidate_never_starts(self):
  obj=self.worker();plan=self.plan(obj);(self.state/'plans/123/instance.candidate').write_bytes(b'tampered')
  with self.assertRaisesRegex(ValueError,'digest'):obj.apply(plan,obj.policy)
  self.assertEqual(self.binary.read_bytes(),b'old-binary')
 def test_crash_after_swap_recovers_before_new_requests(self):
  obj=self.worker();plan=self.plan(obj);folder=self.state/'plans/123';(folder/'instance.previous').write_bytes(b'old-binary');plan['previous']={'instance':w.digest(self.binary)};w.write(folder/'plan.json',plan);self.binary.write_bytes(b'new-binary');obj.save(state='swapping',plan_id='123')
  obj.api=lambda *a,**k:{'policy':obj.policy,'active_prompts':0};obj.policy['check_minutes']=0
  obj.tick();self.assertEqual(self.binary.read_bytes(),b'old-binary');self.assertEqual(obj.state['state'],'rolled_back')
 def test_release_extraction_ignores_paths_and_rejects_symlink(self):
  buf=io.BytesIO()
  with tarfile.open(fileobj=buf,mode='w:gz') as t:
   info=tarfile.TarInfo('../../agent-instance');info.size=3;t.addfile(info,io.BytesIO(b'bad'))
   link=tarfile.TarInfo('agent-instance');link.type=tarfile.SYMTYPE;link.linkname='/etc/passwd';t.addfile(link)
  with self.assertRaises(ValueError):w.extract(buf.getvalue(),'agent-instance')
  buf=io.BytesIO()
  with zipfile.ZipFile(buf,'w') as z:z.writestr('agent-instance',b'correct')
  self.assertEqual(w.extract(buf.getvalue(),'agent-instance'),b'correct')
 def test_idle_auto_install_and_manual_checks(self):
  obj=self.worker();obj.policy['check_minutes']=0;obj.save(state='up_to_date');obj.plan=lambda p:self.fail('manual mode checked automatically');obj.tick()
  obj.save(state='available',plan_id='123');self.plan(obj);obj.policy['install']='when_idle';calls=[];obj.apply=lambda *args,**kwargs:calls.append(args);obj.tick();self.assertEqual(len(calls),1)

 def test_completed_request_is_not_reapplied_after_ack_loss(self):
  obj=self.worker();obj.save(state='succeeded',active_request='receipt-1');acks=[]
  request={'pending':True,'id':'receipt-1','actor':'agent','action':'update','policy_revision':0}
  obj.report=lambda **kwargs: (acks.append(kwargs.get('finished')) or {'policy':obj.policy,'request':request,'active_prompts':0})
  obj.plan=lambda policy:self.fail('completed update was repeated')
  obj.tick();self.assertIn('receipt-1',obj.state['completed_requests']);self.assertIn('receipt-1',acks)
 def test_queued_request_rechecks_policy_revision(self):
  obj=self.worker();obj.policy['revision']=1
  obj.report=lambda **kwargs:{'policy':obj.policy,'request':{'pending':True,'id':'stale','actor':'agent','action':'update','policy_revision':0},'active_prompts':0}
  obj.plan=lambda policy:self.fail('stale request was executed');obj.tick()
  self.assertIn('policy_changed',obj.state['error']);self.assertEqual(self.binary.read_bytes(),b'old-binary')
 def test_window_is_rechecked_after_draining(self):
  obj=self.worker();plan=self.plan(obj);obj.policy['install']='window';obj.in_window=lambda _:False
  with self.assertRaisesRegex(ValueError,'window_ended'):obj.apply(plan,obj.policy,scheduled=True)
  self.assertEqual(self.binary.read_bytes(),b'old-binary')

 def test_frozen_provider_plan_and_health_failure_rollback(self):
  obj=self.worker();comp={'id':'image','provider':'command','protocol':2,'commands':{x:['/provider',x] for x in ['status','plan','apply','rollback','verify']}}
  obj.c['components'].append(comp);calls=[]
  def invoke(argv,**kwargs):
   action=argv[-1];calls.append(action)
   if action=='status':return b'{"version":"old"}'
   if action=='plan':return b'{"available":true,"version":"new","image":"sha256:fixed"}'
   plan=json.loads(pathlib.Path(kwargs['env']['CONSTELLATION_UPDATE_PLAN']).read_text())
   self.assertEqual(plan['plan']['image'],'sha256:fixed')
   if action=='verify' and 'rollback' not in calls:raise ValueError('provider health failed')
   return b'{}'
  with patch.object(w,'candidate',return_value={'id':'instance','available':False,'installed_sha256':w.digest(self.binary)}),patch.object(w,'run',side_effect=invoke):
   plan=obj.plan(obj.policy)
   with self.assertRaisesRegex(ValueError,'provider health'):obj.apply(plan,obj.policy)
  self.assertEqual(calls,['status','plan','apply','verify','rollback','verify'])
  self.assertEqual(obj.state['state'],'rolled_back')
 def test_tampered_provider_plan_never_stops_services(self):
  obj=self.worker();comp={'id':'source','provider':'command','protocol':2,'commands':{x:['/provider',x] for x in ['status','plan','apply','rollback','verify']}}
  obj.c['components'].append(comp)
  with patch.object(w,'candidate',return_value={'id':'instance','available':False,'installed_sha256':w.digest(self.binary)}),patch.object(w,'run',return_value=b'{"available":true,"version":"new"}'):
   plan=obj.plan(obj.policy)
  (self.state/'plans'/plan['id']/'source.provider.json').write_text('{}')
  obj.control=lambda action,scope=None:self.fail('service stopped before validation')
  with self.assertRaisesRegex(ValueError,'provider_plan_changed'):obj.apply(plan,obj.policy)

 def connector_fixture(self):
  self.registration['services']=[{'kind':'systemd-user','name':'instance.service'}]
  self.registration['slack_homes']=[]
  for i in range(2):
   home=self.root/('slack-'+str(i));(home/'bin').mkdir(parents=True);(home/'updates').mkdir();(home/'updates/admission.lock').touch()
   binary=home/'bin/acp-slack-adapter';binary.write_bytes(b'old-slack')
   identity=str(uuid.uuid4());name='acp-slack-adapter-'+identity+'.service'
   w.write(home/'adapter.json',{'id':identity,'enabled':True,'server':{'port':19000+i}})
   (home/'secrets').mkdir();(home/'secrets/control-token').write_text('fixture-only')
   with contextlib.closing(sqlite3.connect(home/'state.sqlite3')) as db:
    db.executescript('CREATE TABLE jobs(status TEXT); CREATE TABLE outbox(status TEXT);')
    if i:db.execute("INSERT INTO jobs VALUES ('running')")
    db.commit()
   self.registration['components'].append({'id':'slack-'+str(i),'kind':'slack','binary':str(binary)})
   self.registration['services'].append({'kind':'systemd-user','name':name})
   self.registration['slack_homes'].append(str(home))
  w.write(self.path,self.registration);obj=self.worker();plan=self.plan(obj)
  plan['components'][0]={'id':'instance','available':False,'installed_sha256':w.digest(self.binary)}
  for i,p in enumerate(self.registration['components'][1:]):
   part={'id':p['id'],'available':i==0,'installed_sha256':w.digest(p['binary'])}
   if i==0:
    candidate=self.state/'plans/123/slack-0.candidate';candidate.write_bytes(b'new-slack');part['sha256']=w.digest(candidate)
   plan['components'].append(part)
  return obj,plan

 def test_connector_only_apply_and_rollback_leave_live_host_connection_alone(self):
  obj,plan=self.connector_fixture();calls=[];drains=[];host_stat=self.binary.stat()
  # A real persistent transport represents another experience's ACP connection.
  host=subprocess.Popen([sys.executable,'-u','-c','import sys\nfor line in sys.stdin: print(line.strip(), flush=True)'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
  self.addCleanup(lambda: (host.terminate(),host.wait(),host.stdin.close(),host.stdout.close()) if host.poll() is None else None)
  def service(argv,**kwargs):
   action,name=argv[-2:];calls.append((action,name))
   if name=='instance.service':host.terminate();host.wait()
   return b''
  def report(**kwargs):
   drains.append(kwargs.get('drain',False));return {'policy':obj.policy,'active_prompts':1}
  obj.report=report;obj.control=lambda action,scope=None:w.Worker.control(obj,action,scope)
  with patch.object(w,'run',side_effect=service):
   obj.apply(plan,obj.policy)
   host.stdin.write('still connected\n');host.stdin.flush();self.assertEqual(host.stdout.readline(),'still connected\n')
   obj.apply(plan,obj.policy,rollback=True)
   host.stdin.write('still connected after rollback\n');host.stdin.flush();self.assertEqual(host.stdout.readline(),'still connected after rollback\n')
  name=self.registration['services'][1]['name']
  self.assertEqual(calls,[('stop',name),('start',name)]*2)
  self.assertFalse(any(drains));self.assertEqual(self.binary.stat().st_mtime_ns,host_stat.st_mtime_ns)
  self.assertFalse((pathlib.Path(self.registration['slack_homes'][1])/'updates/drain.json').exists())
  self.assertEqual((pathlib.Path(self.registration['slack_homes'][0])/'bin/acp-slack-adapter').read_bytes(),b'old-slack')

 def test_connector_health_accepts_unrestarted_host_and_ignores_other_connector(self):
  obj,plan=self.connector_fixture();scope=obj.restart_scope(plan)
  obj.api=lambda *a,**k:{'instance_id':'fixture','uptime_ms':86400000}
  expected={p['id']:w.digest(p['binary']) for p in obj.c['components']}
  obj.state['cutover_at']=1234
  with patch.object(w.ssl,'create_default_context'),patch.object(w,'http_json',return_value={'state':{'slack_connection':{'state':'connected'}},'runtime':{'updated_at':1235}}) as http:
   w.Worker.health(obj,expected,scope)
  self.assertEqual(http.call_count,1);self.assertIn(':19000/',http.call_args.args[0])

 def test_connector_failure_and_crash_recovery_keep_frozen_service_scope(self):
  obj,plan=self.connector_fixture();calls=[];checks=[]
  obj.control=lambda action,scope=None:calls.append((action,scope))
  def health(expected,scope=None):
   checks.append(scope)
   if len(checks)==1:raise ValueError('new connector failed')
  obj.health=health
  with self.assertRaisesRegex(ValueError,'new connector failed'):obj.apply(plan,obj.policy)
  self.assertTrue(all(not scope['instance'] and len(scope['services'])==1 for _,scope in calls))
  calls.clear();obj.health=lambda *a:None
  # Recover from the persisted plan, as a newly started helper does.
  obj.rollback(w.read(self.state/'plans/123/plan.json'))
  self.assertEqual(len(calls),2);self.assertFalse(calls[0][1]['instance'])

 def test_runtime_and_unknown_provider_changes_keep_full_restart_dependencies(self):
  obj,plan=self.connector_fixture();plan['components'][0]['available']=True
  self.assertEqual(obj.restart_scope(plan),obj.full_restart_scope())
  plan['components'][0]['available']=False
  obj.c['components'].append({'id':'image','provider':'command'})
  plan['components'].append({'id':'image','available':True})
  self.assertEqual(obj.restart_scope(plan),obj.full_restart_scope())

 def test_legacy_connector_mapping_and_explicit_native_service_mapping(self):
  obj,plan=self.connector_fixture();scope=obj.restart_scope(plan);self.assertFalse(scope['instance'])
  obj.c['services'][1]={'kind':'launchd','name':'io.fixture.connector'}
  home=pathlib.Path(obj.c['slack_homes'][0]);w.write(home/'install.json',{'service':obj.c['services'][1]})
  self.assertEqual(obj.restart_scope(plan)['services'],['io.fixture.connector'])
  obj.c['components'][1]['service_name']='io.fixture.connector';(home/'install.json').unlink()
  self.assertEqual(obj.restart_scope(plan)['services'],['io.fixture.connector'])
  del obj.c['components'][1]['service_name']
  self.assertTrue(obj.restart_scope(plan)['instance'])

 def test_changed_scope_rejected_before_cutover_or_rollback(self):
  obj,plan=self.connector_fixture();plan['restart_scope']=obj.restart_scope(plan)
  obj.c['services'][1]['name']='different.service'
  obj.control=lambda *a:self.fail('service stopped before scope validation')
  with self.assertRaisesRegex(ValueError,'restart_scope_changed'):obj.apply(plan,obj.policy)
  with self.assertRaisesRegex(ValueError,'restart_scope_changed'):obj.rollback(plan)

 def test_invalid_explicit_service_binding_is_rejected(self):
  self.registration['components'][0]['service_name']='missing.service';w.write(self.path,self.registration)
  with self.assertRaisesRegex(ValueError,'unknown_component_service'):w.load_registration(self.path)

if __name__=='__main__':unittest.main()
