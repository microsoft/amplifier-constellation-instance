import importlib.util,json,os,sys,tempfile,unittest
from pathlib import Path
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('engine',Path(__file__).with_name('engine_provider.py'));e=importlib.util.module_from_spec(spec);spec.loader.exec_module(e)
class EngineTests(unittest.TestCase):
 def setUp(self):
  t=tempfile.TemporaryDirectory();self.addCleanup(t.cleanup);self.root=Path(t.name);self.home=self.root/'agent';self.runtime=self.home/'engine/releases';self.old=self.runtime/'claude-agent-acp-1.0.0';self.old.mkdir(parents=True)
  (self.old/'claude-agent-acp').write_text('old');(self.old/'claude').write_text('old companion')
  self.backend={'command':str(self.old/'claude-agent-acp'),'args':[],'env':{'KEEP':'same'},'secret_env':{'KEY':'owned'},'working_directory':'${AGENT_HOME}/workspace'}
  e.w.write(self.home/'config.json',{'revision':7,'config':{'backend':self.backend,'identity':'keep'}})
  c=self.root/'provider.json';e.w.write(c,{'home':str(self.home),'runtime_root':str(self.runtime),'state':str(self.root/'owner'),'kind':'claude'});self.p=e.Provider(c)
 def planned(self):
  def stage(release,dest):
   (dest/'claude-agent-acp').write_text('new');(dest/'claude').write_text('new companion');return str(dest/'claude-agent-acp')
  with patch.object(self.p,'discover',return_value={'version':'1.1.0'}),patch.object(self.p,'stage',side_effect=stage):p=self.p.plan()
  file=self.root/'plan.json';e.w.write(file,{'id':'12345678-1234-1234-1234-123456789012','plan':p});env=patch.dict(os.environ,CONSTELLATION_UPDATE_PLAN=str(file));env.start();self.addCleanup(env.stop);return p
 def test_staged_apply_and_rollback_preserve_engine_settings_and_history(self):
  (self.home/'history').write_text('conversation');p=self.planned();self.assertEqual(e.w.read(self.home/'config.json')['revision'],7)
  self.p.apply();cfg=e.w.read(self.home/'config.json');self.assertEqual(cfg['revision'],8);self.assertEqual(cfg['config']['backend']['env'],self.backend['env']);self.assertEqual(cfg['config']['backend']['secret_env'],self.backend['secret_env'])
  self.assertEqual(self.p.status()['version'],'1.1.0');self.p.apply();self.assertEqual(e.w.read(self.home/'config.json')['revision'],8)
  self.p.rollback();self.assertEqual(e.w.read(self.home/'config.json')['config']['backend'],self.backend);self.assertEqual((self.home/'history').read_text(),'conversation')
 def test_tampered_candidate_refused_before_binding_switch(self):
  p=self.planned();(Path(p['candidate'])/'claude').write_text('tampered')
  with self.assertRaisesRegex(ValueError,'staged_engine_changed'):self.p.apply()
  self.assertEqual(e.w.read(self.home/'config.json')['revision'],7)
 def test_concurrent_backend_change_not_overwritten(self):
  self.planned();cfg=e.w.read(self.home/'config.json');cfg['config']['backend']['env']['KEEP']='new choice';e.w.write(self.home/'config.json',cfg)
  with self.assertRaisesRegex(ValueError,'backend_changed'):self.p.apply()
 def test_different_identity_configuration_survives_cutover(self):
  self.planned();cfg=e.w.read(self.home/'config.json');cfg['config']['identity']='edited';cfg['revision']=8;e.w.write(self.home/'config.json',cfg);self.p.apply()
  self.assertEqual(e.w.read(self.home/'config.json')['config']['identity'],'edited');self.assertEqual(e.w.read(self.home/'config.json')['revision'],9)
 def test_failed_staging_cleans_only_its_candidate(self):
  with patch.object(self.p,'discover',return_value={'version':'1.1.0'}),patch.object(self.p,'stage',side_effect=ValueError('failed')):
   with self.assertRaises(ValueError):self.p.plan()
  self.assertEqual(list(self.runtime.iterdir()),[self.old])
 def test_no_update_does_not_install_or_mutate(self):
  with patch.object(self.p,'discover',return_value=None),patch.object(self.p,'stage') as stage:
   self.assertFalse(self.p.plan()['available']);stage.assert_not_called()
 def test_missing_platform_release_preserves_engine_and_reports_unavailable(self):
  before=(self.home/'config.json').read_bytes()
  for assets in ([],[{'name':'claude-agent-acp-windows-x64.zip'}]):
   release={'tag_name':'v1.1.0','html_url':'https://github.com/agentclientprotocol/claude-agent-acp/releases/tag/v1.1.0','assets':assets}
   with self.subTest(assets=assets),patch.object(e.w,'gh',return_value=release),patch.object(e.platform,'system',return_value='Linux'),patch.object(e.platform,'machine',return_value='aarch64'),patch.object(self.p,'stage') as stage:
    result=self.p.plan()
    self.assertEqual(result,{'available':False,'version':'1.0.0','release_version':'1.1.0','reason':'release_not_available_for_platform'})
    stage.assert_not_called();self.assertEqual(list(self.runtime.iterdir()),[self.old]);self.assertEqual((self.home/'config.json').read_bytes(),before)
 def test_platform_release_becomes_available_on_later_check(self):
  release={'tag_name':'v1.1.0','html_url':'https://github.com/agentclientprotocol/claude-agent-acp/releases/tag/v1.1.0','assets':[]}
  asset={'name':'claude-agent-acp-linux-arm64.tar.gz','digest':'sha256:'+'a'*64}
  def stage(release,dest):
   self.assertEqual(release['asset'],asset);(dest/'claude-agent-acp').write_text('new');return str(dest/'claude-agent-acp')
  with patch.object(e.w,'gh',side_effect=[release,{**release,'assets':[asset]}]),patch.object(e.platform,'system',return_value='Linux'),patch.object(e.platform,'machine',return_value='aarch64'),patch.object(self.p,'stage',side_effect=stage) as install:
   self.assertFalse(self.p.plan()['available']);self.assertTrue(self.p.plan()['available']);install.assert_called_once()
  self.assertEqual(e.w.read(self.home/'config.json')['revision'],7)
 def test_release_metadata_failure_remains_an_error(self):
  with patch.object(e.w,'gh',side_effect=RuntimeError('metadata unavailable')),patch.object(self.p,'stage') as stage:
   with self.assertRaisesRegex(RuntimeError,'metadata unavailable'):self.p.plan()
   stage.assert_not_called()
 def test_worker_still_plans_independent_source_when_engine_asset_is_absent(self):
  components=[]
  for name in ['engine','minecraft']:
   components.append({'id':name,'provider':'command','protocol':2,'commands':{action:[sys.executable,name,action] for action in ['status','plan','apply','verify','rollback']},'restart_scope':{'services':[],'instance':False,'slack_homes':[]}})
  registration=self.root/'registration.json';e.w.write(registration,{'schema':1,'instance_id':'fixture','home':str(self.home),'state':str(self.root/'shared'),'components':components,'services':[]})
  with patch.object(e.w.ssl,'create_default_context'):worker=e.w.Worker(registration)
  worker.report=lambda *args,**kwargs:None
  calls=[]
  def run(argv,**kwargs):
   _,component,action=argv;calls.append((component,action))
   result=getattr(self.p,action)() if component=='engine' else ({'version':'old-source'} if action=='status' else {'available':True,'version':'new-source'})
   return json.dumps(result).encode()
  release={'tag_name':'v1.1.0','html_url':'https://github.com/agentclientprotocol/claude-agent-acp/releases/tag/v1.1.0','assets':[]}
  with patch.object(e.w,'run',side_effect=run),patch.object(e.w,'gh',return_value=release),patch.object(self.p,'stage') as stage:
   plan=worker.build_plan({'revision':0,'channel':'stable'})
  self.assertEqual(calls,[('engine','status'),('engine','plan'),('minecraft','status'),('minecraft','plan')]);stage.assert_not_called()
  self.assertEqual([(p['id'],p['available']) for p in plan['components']],[('engine',False),('minecraft',True)])
  frozen=e.w.read(worker.root/'plans'/plan['id']/'engine.provider.json')['plan']
  self.assertEqual(frozen['reason'],'release_not_available_for_platform');self.assertEqual(self.p.status()['version'],'1.0.0')
 def test_declared_family_cannot_switch(self):
  self.planned();self.p.apply();current=self.p.current()[2];record=e.w.read(current/'engine-update.json');record['kind']='codex';e.w.write(current/'engine-update.json',record)
  with self.assertRaisesRegex(ValueError,'family_changed'):self.p.current()
 def test_bytecode_cache_is_not_engine_drift(self):
  p=self.planned();folder=Path(p['candidate']);(folder/'__pycache__').mkdir();(folder/'__pycache__/loaded.pyc').write_bytes(b'transient');self.p.apply();self.assertTrue(self.p.verify()['healthy'])
 def test_windows_executable_paths_keep_placeholder_and_account_paths(self):
  value={'command':'C:/agent/releases/old/bin/engine.exe','env':{'ACCOUNT':'C:/agent/account'}}
  result=e.remap(value,r'C:\agent\releases\old',r'C:\agent\releases\new')
  self.assertEqual(result['command'],'C:/agent/releases/new/bin/engine.exe');self.assertEqual(result['env'],value['env'])
 def test_release_binary_limit_is_explicit(self):
  import io,tarfile
  out=io.BytesIO()
  with tarfile.open(fileobj=out,mode='w:gz') as archive:
   member=tarfile.TarInfo('claude');member.size=6;archive.addfile(member,io.BytesIO(b'123456'))
  with self.assertRaises(ValueError):e.w.extract(out.getvalue(),'claude',limit=5)
  self.assertEqual(e.w.extract(out.getvalue(),'claude',limit=6),b'123456')
if __name__=='__main__':unittest.main()
