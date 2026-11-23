import importlib.util,json,os,pathlib,subprocess,tempfile,unittest,uuid
from unittest.mock import patch
ROOT=pathlib.Path(__file__).parent
spec=importlib.util.spec_from_file_location('enroll',ROOT/'install.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class EnrollmentTests(unittest.TestCase):
 def test_existing_native_relative_home_still_resolves(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=pathlib.Path(tmp).resolve();home=root/'agent-home';state=root/'owner';(home/'bin').mkdir(parents=True)
   (home/'bin/agent-instance').write_text('fixture');identity=str(uuid.uuid4())
   (home/'config.json').write_text(json.dumps({'config':{'instance_id':identity,'backend':None}}))
   worker=ROOT.resolve()/'worker.py';previous=pathlib.Path.cwd()
   try:
    os.chdir(root)
    with patch.object(m.platform,'system',return_value='Linux'),patch.object(m.Path,'home',return_value=root),patch.object(m,'run'),patch.object(m.subprocess,'run',return_value=subprocess.CompletedProcess([],0)):
     m.enroll(pathlib.Path('agent-home'),state,[],worker)
   finally:os.chdir(previous)
   self.assertEqual(json.loads((state/'registration.json').read_text())['home'],str(home))
 def test_existing_native_registration_resolves_equivalent_home(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=pathlib.Path(tmp).resolve();home=root/'home';state=root/'owner';(home/'bin').mkdir(parents=True)
   (home/'bin/agent-instance').write_text('fixture');identity=str(uuid.uuid4())
   (home/'config.json').write_text(json.dumps({'config':{'instance_id':identity,'backend':None}}))
   with patch.object(m.platform,'system',return_value='Linux'),patch.object(m.Path,'home',return_value=root),patch.object(m,'run'),patch.object(m.subprocess,'run',return_value=subprocess.CompletedProcess([],0)):
    m.enroll(home,state,[],ROOT/'worker.py')
    registration=json.loads((state/'registration.json').read_text());registration['home']=str(home/'..'/'home')
    (state/'registration.json').write_text(json.dumps(registration))
    m.enroll(home,state,[],ROOT/'worker.py')
   self.assertEqual(json.loads((state/'registration.json').read_text())['home'],str(home))
 def test_reenrollment_preserves_experience_and_separate_runtime_helper(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=pathlib.Path(tmp).resolve();home=root/'home';state=root/'owner';(home/'bin').mkdir(parents=True);state.mkdir();binary=home/'bin/agent-instance';binary.write_text('fixture');helper=home/'helper';helper.write_text('fixture')
   identity=str(uuid.uuid4());(home/'config.json').write_text(json.dumps({'config':{'instance_id':identity,'backend':None}}))
   participant={'id':'game','protocol':1,'components':['instance','task-helpers'],'commands':{a:[str(helper),a] for a in ('prepare','verify','restore')}}
   old={'schema':1,'instance_id':identity,'home':str(home),'state':str(state),'components':[{'id':'instance','kind':'instance','binary':str(binary)},{'id':'task-helpers','kind':'runtime','binary':str(helper),'restart_scope':{'instance':False,'services':[],'slack_homes':[]}}],'services':[],'participants':[participant]}
   old['service_connections']={'schema':1,'services':[],'connections':[]}
   old['when_idle_components']=['task-helpers']
   (state/'registration.json').write_text(json.dumps(old))
   with patch.object(m.platform,'system',return_value='Linux'),patch.object(m.Path,'home',return_value=root),patch.object(m,'run'),patch.object(m.subprocess,'run',return_value=subprocess.CompletedProcess([],0)):
    m.enroll(home,state,[],ROOT/'worker.py')
   new=json.loads((state/'registration.json').read_text());self.assertEqual(new['service_connections'],old['service_connections']);self.assertEqual(new['participants'],[participant]);self.assertIn('task-helpers',[p['id'] for p in new['components']])
   self.assertEqual(new['when_idle_components'],['task-helpers'])
 def test_removing_selected_backend_refuses_before_updater_publication(self):
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory).resolve()
        home, state = root/'agent', root/'owner'
        (home/'bin').mkdir(parents=True)
        state.mkdir()
        instance = home/'bin/agent-instance'
        runtime = home/'bin/agent-runtime'
        instance.write_bytes(b'instance')
        runtime.write_bytes(b'prior runtime')
        identity = str(uuid.uuid4())
        (home/'config.json').write_text(json.dumps({'config': {'instance_id': identity, 'backend': None}}))
        previous = {'schema': 1, 'instance_id': identity, 'home': str(home), 'state': str(state),
                    'components': [{'id': 'instance', 'kind': 'instance', 'binary': str(instance)},
                                   {'id': 'runtime', 'kind': 'runtime', 'binary': str(runtime)}],
                    'services': [], 'when_idle_components': ['runtime']}
        registration = state/'registration.json'
        registration.write_text(json.dumps(previous))
        original = registration.read_bytes()
        # Exercise registration for each manager without starting a real service.
        (home/'install.json').write_text(json.dumps({'service': {'name': 'fixture'}}))
        for system in ('Linux', 'Darwin', 'Windows'):
            with self.subTest(system=system), patch.object(m.platform, 'system', return_value=system), \
                    patch.object(m, 'worker_python', return_value=m.sys.executable):
                for attempt in range(2):
                    with self.subTest(attempt=attempt), self.assertRaisesRegex(ValueError, 'idle component selection'):
                        m.enroll(home, state, [], ROOT/'worker.py')
                    self.assertEqual(registration.read_bytes(), original)
                    self.assertFalse((state/'state.json').exists())
                    self.assertFalse((state/'worker.py').exists())

 def test_vanilla_engine_is_enrolled_without_modifying_backend(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=pathlib.Path(tmp).resolve();home=root/'home';state=root/'owner';(home/'bin').mkdir(parents=True);(home/'bin/agent-instance').write_text('fixture')
   command=home/'engine/releases/claude-agent-acp-1.0.0/claude-agent-acp';command.parent.mkdir(parents=True);command.write_text('fixture');config={'config':{'instance_id':str(uuid.uuid4()),'backend':{'command':str(command),'env':{'KEEP':'private'}}}};(home/'config.json').write_text(json.dumps(config))
   with patch.object(m.platform,'system',return_value='Linux'),patch.object(m.Path,'home',return_value=root),patch.object(m,'run'),patch.object(m.subprocess,'run',return_value=subprocess.CompletedProcess([],0)):
    m.enroll(home,state,[],ROOT/'worker.py')
   new=json.loads((state/'registration.json').read_text());self.assertIn('engine',[p['id'] for p in new['components']]);self.assertEqual(json.loads((home/'config.json').read_text()),config);self.assertEqual(json.loads((state/'engine.json').read_text())['kind'],'claude')
if __name__=='__main__':unittest.main()
