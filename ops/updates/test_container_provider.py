import importlib.util,json,os,pathlib,tempfile,unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('provider',pathlib.Path(__file__).with_name('container_provider.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class Tests(unittest.TestCase):
 def setUp(self):
  t=tempfile.TemporaryDirectory();self.addCleanup(t.cleanup);self.root=pathlib.Path(t.name)
  self.manifest={'container':'agent-demo','state':str(self.root/'container'),'homes':[{'home':str(self.root/'agent')}],'services':{'demo.service':{}},'image':'sha256:'+'a'*64,'mounts':[]}
  m.write(self.root/'manifest.json',self.manifest)
  m.write(self.root/'config.json',{'manifest':str(self.root/'manifest.json'),'state':str(self.root/'provider'),'installer':str(self.root/'install.py'),'image':'approved:latest'})
  (self.root/'install.py').write_text('def install(manifest):\n pass\n')
  self.p=m.Provider(self.root/'config.json');self.current={'Id':'container-original','Image':'sha256:'+'a'*64,'State':{'Running':True}}
  self.new={'Id':'sha256:'+'b'*64}
 def prepare(self):
  with patch.object(m,'inspect',side_effect=[self.new,self.current]):plan=self.p.plan()
  file=self.root/'plan.json';m.write(file,{'id':'12345678-1234-1234-1234-123456789abc','component':'image','plan':plan})
  self.env=patch.dict(os.environ,CONSTELLATION_UPDATE_PLAN=str(file));self.env.start();self.addCleanup(self.env.stop)
  return plan
 def test_plan_pins_digest(self):
  p=self.prepare();self.assertTrue(p['available']);self.assertEqual(p['image'],'sha256:'+'b'*64);self.assertEqual(p['manifest'],self.manifest)
 def test_drift_prevents_cutover(self):
  self.prepare()
  with patch.object(m,'inspect',return_value={**self.current,'Id':'replaced'}),patch.object(m,'run') as run:
   with self.assertRaisesRegex(ValueError,'changed_since_plan'):self.p.apply()
   run.assert_not_called()
 def test_recovery_before_rename_is_idempotent(self):
  self.prepare();_,receipt,_=self.p.context();m.write(receipt,{'state':'applying','expected_image':'sha256:'+'b'*64})
  with patch.object(m,'exists',return_value=False),patch.object(m,'inspect',return_value=self.current),patch.object(m,'run'):
   self.p.rollback();self.p.rollback()
  self.assertEqual(m.read(receipt)['state'],'rolled_back')
 def test_recovery_after_create_restores_old_container(self):
  self.prepare();_,receipt,old=self.p.context();m.write(receipt,{'state':'applying'})
  with patch.object(m,'exists',side_effect=[True,True,False]),patch.object(m,'inspect',return_value=self.current),patch.object(m,'run') as run:
   self.p.rollback()
  self.assertIn(('docker','rename',old,'agent-demo'),[x.args for x in run.call_args_list])
  self.assertEqual(m.read(self.root/'manifest.json'),self.manifest)
 def test_verify_rejects_wrong_running_image(self):
  self.prepare();_,receipt,_=self.p.context();m.write(receipt,{'expected_image':'sha256:'+'b'*64})
  with patch.object(m,'inspect',return_value=self.current):
   with self.assertRaisesRegex(ValueError,'health_mismatch'):self.p.verify()
 def test_cannot_mount_helper_state_into_agent(self):
  c=m.read(self.root/'config.json');c['state']=str(self.root/'agent/provider');m.write(self.root/'config.json',c)
  with self.assertRaisesRegex(ValueError,'outside_agent'):m.Provider(self.root/'config.json')
if __name__=='__main__':unittest.main()
