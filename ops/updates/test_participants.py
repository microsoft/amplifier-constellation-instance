"""Interruption and service-boundary tests for the shared update transaction."""
import json, subprocess
from unittest.mock import patch
import unittest
import test_worker as fixtures
w=fixtures.w

class ParticipantTests(unittest.TestCase):
 setUp=fixtures.UpdateTests.setUp
 worker=fixtures.UpdateTests.worker
 plan=fixtures.UpdateTests.plan
 def participant(self,obj,name='game'):
  p={'id':name,'protocol':1,'components':['instance'],'commands':{a:['/owner-helper',name,a] for a in w.PARTICIPANT_ACTIONS}}
  obj.c.setdefault('participants',[]).append(p);return p
 def test_busy_preparation_precedes_any_cutover(self):
  obj=self.worker();self.participant(obj);plan=self.plan(obj);events=[];n=[0]
  def call(p,a,t,o):
   events.append(a);n[0]+=a=='prepare';return a!='prepare' or n[0]>1
  obj.participant_call=call;obj.control=lambda a,scope=None:events.append(a)
  with patch.object(w.time,'sleep'):obj.apply(plan,obj.policy)
  self.assertEqual(events,['prepare','prepare','stop','start','verify','restore']);self.assertIsNone(obj.state['transaction'])
 def test_lost_restore_ack_keeps_verified_release(self):
  obj=self.worker();self.participant(obj);plan=self.plan(obj);events=[]
  def call(p,a,t,o):
   events.append((a,o))
   if a=='restore':raise TimeoutError('ack lost')
   return True
  obj.participant_call=call
  with self.assertRaises(TimeoutError):obj.apply(plan,obj.policy)
  self.assertEqual(obj.state['transaction']['phase'],'restoring');self.assertEqual(self.binary.read_bytes(),b'new-binary')
  obj.participant_call=lambda p,a,t,o:events.append((a,o)) or True
  obj.recover_transaction()
  self.assertEqual(events,[('prepare','unchanged'),('verify','applied'),('restore','applied'),('restore','applied')]);self.assertEqual(self.binary.read_bytes(),b'new-binary')
 def test_failed_continuity_rolls_back_before_restoration(self):
  obj=self.worker();self.participant(obj);plan=self.plan(obj);events=[]
  def call(p,a,t,o):
   events.append((a,o));return not(a=='verify' and o=='applied')
  obj.participant_call=call
  with self.assertRaisesRegex(ValueError,'continuity'):obj.apply(plan,obj.policy)
  self.assertEqual(self.binary.read_bytes(),b'old-binary');self.assertEqual(events[-2:],[('verify','rolled_back'),('restore','rolled_back')])
 def test_changed_enrollment_rejects_before_pause(self):
  obj=self.worker();plan=self.plan(obj);plan['registration_sha256']=obj.registration_digest();self.participant(obj)
  obj.participant_call=lambda *a:self.fail('must not pause')
  with self.assertRaisesRegex(ValueError,'enrollment_changed'):obj.apply(plan,obj.policy)
 def test_game_only_scope_does_not_restart_instance(self):
  obj=self.worker();obj.c['components'].append({'id':'game','provider':'command','restart_scope':{'services':[],'instance':False,'slack_homes':[]}})
  scope=obj.restart_scope({'components':[{'id':'instance','available':False},{'id':'game','available':True}]})
  self.assertEqual(scope,{'services':[],'instance':False,'slack_homes':[]})
 def test_prepare_lost_reply_recovers_original_state(self):
  obj=self.worker();self.participant(obj);plan=self.plan(obj);events=[]
  def call(p,a,t,o):
   events.append((a,o))
   if a=='prepare':raise TimeoutError()
   return True
  obj.participant_call=call
  with self.assertRaises(TimeoutError):obj.apply(plan,obj.policy)
  self.assertEqual(events,[('prepare','unchanged'),('verify','unchanged'),('restore','unchanged')]);self.assertEqual(self.binary.read_bytes(),b'old-binary')

 def test_scheduled_idle_install_uses_participant_and_changes_binary(self):
  obj=self.worker();self.participant(obj);plan=self.plan(obj);obj.policy.update(check_minutes=0,install='when_idle');obj.save(state='available',plan_id=plan['id']);events=[]
  obj.participant_call=lambda p,a,t,o:events.append(a) or True
  obj.tick();self.assertEqual(self.binary.read_bytes(),b'new-binary');self.assertEqual(events,['prepare','verify','restore']);self.assertEqual(obj.state['state'],'succeeded')
 def test_inactive_services_never_started(self):
  obj=self.worker();obj.c['services']=[{'name':'live.service','kind':'systemd-user'},{'name':'paused.service','kind':'systemd-user'}];obj.save(service_snapshot={'live.service':True,'paused.service':False})
  with patch.object(w,'run') as run:
   w.Worker.control(obj,'start',{'services':['live.service','paused.service'],'instance':True,'slack_homes':[]})
  self.assertEqual([c.args[0] for c in run.call_args_list],[['systemctl','--user','start','live.service']])

 def test_status_does_not_export_owner_commands_or_context_paths(self):
  obj=self.worker();obj.state['transaction']={'id':'tx','participants':[{'id':'game','commands':{'prepare':['/private/owner','secret-canary']}}],'directory':'/private/receipts','contexts':{'game':'private'},'phase':'preparing'}
  captured=[];obj.api=lambda path,body:captured.append(body) or {}
  w.Worker.report(obj)
  value=json.dumps(captured);self.assertNotIn('secret-canary',value);self.assertNotIn('/private',value);self.assertEqual(captured[0]['status']['transaction']['participants'],['game'])

 def test_progressing_session_replay_can_finish_after_short_control_deadline(self):
  obj=self.worker();self.participant(obj);plan=self.plan(obj);calls=[]
  def run(argv,timeout,env):
   action=argv[-1];calls.append((action,timeout,env['CONSTELLATION_UPDATE_OUTCOME']))
   # Simulated healthy replay takes 45 seconds. The subprocess boundary must
   # permit it rather than rolling back an otherwise compatible installation.
   if action=='verify' and timeout<45:raise subprocess.TimeoutExpired(argv,timeout)
   return json.dumps({{'prepare':'ready','verify':'verified','restore':'restored'}[action]:True}).encode()
  with patch.object(w,'run',side_effect=run):obj.apply(plan,obj.policy)
  self.assertEqual(self.binary.read_bytes(),b'new-binary');self.assertIsNone(obj.state['transaction'])
  self.assertEqual(calls,[('prepare',30,'unchanged'),('verify',300,'applied'),('restore',30,'applied')])

 def test_exhausted_replay_budget_still_rolls_back_before_restore(self):
  obj=self.worker();self.participant(obj);plan=self.plan(obj);calls=[]
  def run(argv,timeout,env):
   action=argv[-1];outcome=env['CONSTELLATION_UPDATE_OUTCOME'];calls.append((action,outcome))
   if action=='verify' and outcome=='applied':raise subprocess.TimeoutExpired(argv,timeout)
   return json.dumps({{'prepare':'ready','verify':'verified','restore':'restored'}[action]:True}).encode()
  with patch.object(w,'run',side_effect=run),self.assertRaises(subprocess.TimeoutExpired):obj.apply(plan,obj.policy)
  self.assertEqual(self.binary.read_bytes(),b'old-binary')
  self.assertEqual(calls,[('prepare','unchanged'),('verify','applied'),('verify','rolled_back'),('restore','rolled_back')])

if __name__=='__main__':
 import unittest
 unittest.main()
