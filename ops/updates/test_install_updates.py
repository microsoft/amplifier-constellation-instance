import importlib.util,json,pathlib,subprocess,tempfile,unittest,uuid
from unittest.mock import patch
ROOT=pathlib.Path(__file__).parent
spec=importlib.util.spec_from_file_location('enroll',ROOT/'install.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class EnrollmentTests(unittest.TestCase):
 def test_reenrollment_preserves_experience_and_separate_runtime_helper(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=pathlib.Path(tmp);home=root/'home';state=root/'owner';(home/'bin').mkdir(parents=True);state.mkdir();binary=home/'bin/agent-instance';binary.write_text('fixture');helper=home/'helper';helper.write_text('fixture')
   identity=str(uuid.uuid4());(home/'config.json').write_text(json.dumps({'config':{'instance_id':identity,'backend':None}}))
   participant={'id':'game','protocol':1,'components':['instance','task-helpers'],'commands':{a:['/owner/helper',a] for a in ('prepare','verify','restore')}}
   old={'schema':1,'instance_id':identity,'home':str(home),'state':str(state),'components':[{'id':'instance','kind':'instance','binary':str(binary)},{'id':'task-helpers','kind':'runtime','binary':str(helper),'restart_scope':{'instance':False,'services':[],'slack_homes':[]}}],'services':[],'participants':[participant]}
   (state/'registration.json').write_text(json.dumps(old))
   with patch.object(m.platform,'system',return_value='Linux'),patch.object(m.Path,'home',return_value=root),patch.object(m,'run'),patch.object(m.subprocess,'run',return_value=subprocess.CompletedProcess([],0)):
    m.enroll(home,state,[],ROOT/'worker.py')
   new=json.loads((state/'registration.json').read_text());self.assertEqual(new['participants'],[participant]);self.assertIn('task-helpers',[p['id'] for p in new['components']])
 def test_vanilla_engine_is_enrolled_without_modifying_backend(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=pathlib.Path(tmp);home=root/'home';state=root/'owner';(home/'bin').mkdir(parents=True);(home/'bin/agent-instance').write_text('fixture')
   command=home/'engine/releases/claude-agent-acp-1.0.0/claude-agent-acp';command.parent.mkdir(parents=True);command.write_text('fixture');config={'config':{'instance_id':str(uuid.uuid4()),'backend':{'command':str(command),'env':{'KEEP':'private'}}}};(home/'config.json').write_text(json.dumps(config))
   with patch.object(m.platform,'system',return_value='Linux'),patch.object(m.Path,'home',return_value=root),patch.object(m,'run'),patch.object(m.subprocess,'run',return_value=subprocess.CompletedProcess([],0)):
    m.enroll(home,state,[],ROOT/'worker.py')
   new=json.loads((state/'registration.json').read_text());self.assertIn('engine',[p['id'] for p in new['components']]);self.assertEqual(json.loads((home/'config.json').read_text()),config);self.assertEqual(json.loads((state/'engine.json').read_text())['kind'],'claude')
if __name__=='__main__':unittest.main()
