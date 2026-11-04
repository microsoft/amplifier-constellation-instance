import contextlib, importlib.util, io, json, pathlib, tarfile, tempfile, unittest, zipfile
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
  obj.control=lambda action:None
  obj.health=lambda expected:None
  return obj
 def plan(self,obj):
  folder=self.state/'plans'/'123';folder.mkdir(parents=True);(folder/'instance.candidate').write_bytes(b'new-binary')
  p={'id':'123','policy_revision':0,'components':[{'id':'instance','available':True,'installed_sha256':w.digest(self.binary),'sha256':w.digest(folder/'instance.candidate')}]};w.write(folder/'plan.json',p);return p
 def test_helper_state_cannot_be_inside_agent(self):
  self.registration['state']=str(self.home/'helper');w.write(self.path,self.registration)
  with self.assertRaisesRegex(ValueError,'outside'):w.load_registration(self.path)
 def test_apply_and_manual_rollback_preserve_data(self):
  obj=self.worker();plan=self.plan(obj);(self.home/'conversation.json').write_text('keep new messages')
  obj.apply(plan,obj.policy,True);self.assertEqual(self.binary.read_bytes(),b'new-binary');self.assertEqual(obj.state['state'],'succeeded')
  obj.apply(plan,obj.policy,True,True);self.assertEqual(self.binary.read_bytes(),b'old-binary');self.assertEqual(obj.state['state'],'rolled_back');self.assertEqual((self.home/'conversation.json').read_text(),'keep new messages')
 def test_health_failure_restores_previous_binary(self):
  obj=self.worker();plan=self.plan(obj);calls=[]
  def health(expected):
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

if __name__=='__main__':unittest.main()
