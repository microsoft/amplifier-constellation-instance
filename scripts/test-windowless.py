"""Windowless helper policy and fixed paired interpreter; no OS task mutation."""
import ast
import hashlib
import importlib.util
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
ROOT=pathlib.Path(__file__).resolve().parents[1]
def load(name,relative):
    spec=importlib.util.spec_from_file_location(name,ROOT/relative);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module
installer=load('windowless_install','ops/updates/install.py')
native=load('windowless_native','scripts/native-install.py')
worker=load('windowless_worker','ops/updates/worker.py')
provision=load('windowless_provision','scripts/provision-native.py')
consumer=load('windowless_consumer','scripts/consumer-bundle.py')
class WindowlessTests(unittest.TestCase):
    def paired(self,root):
        home=root/'private home';folder=home/'support/python-3.13.18-fixture';folder.mkdir(parents=True)
        python=folder/'python.exe';python.write_bytes(b'python')
        windowless=folder/'pythonw.exe';windowless.write_bytes(b'windowless')
        metadata={'private_python':{'directory':folder.name,'executable':'python.exe','windowless_executable':'pythonw.exe','windowless_sha256':hashlib.sha256(windowless.read_bytes()).hexdigest()}}
        (home/'install.json').write_text(json.dumps(metadata));return home,python,windowless,metadata
    def test_exact_private_windowless_pair_is_selected_without_mutation(self):
        with tempfile.TemporaryDirectory() as tmp:
            home,python,windowless,_=self.paired(pathlib.Path(tmp));before=(home/'install.json').read_bytes()
            with patch.object(installer.platform,'system',return_value='Windows'),patch.object(installer.sys,'executable',str(python)):
                self.assertEqual(installer.worker_python(home),str(windowless))
            self.assertEqual((home/'install.json').read_bytes(),before)
    def test_no_ambient_or_unrecorded_windowless_fallback(self):
        for case in ('wrong_interpreter','missing_metadata','missing_windowless','wrong_digest','path_escape'):
            with self.subTest(case=case),tempfile.TemporaryDirectory() as tmp:
                home,python,windowless,metadata=self.paired(pathlib.Path(tmp))
                if case=='missing_metadata':metadata['private_python'].pop('windowless_executable')
                if case=='missing_windowless':windowless.unlink()
                if case=='wrong_digest':windowless.write_bytes(b'changed')
                if case=='path_escape':metadata['private_python']['directory']='python-../../elsewhere'
                (home/'install.json').write_text(json.dumps(metadata))
                with patch.object(installer.platform,'system',return_value='Windows'),patch.object(installer.sys,'executable',str(home/'ambient-python.exe') if case=='wrong_interpreter' else str(python)):
                    with self.assertRaises((ValueError,OSError)):installer.worker_python(home)
    def test_interpreter_symlink_refuses_without_repair(self):
        with tempfile.TemporaryDirectory() as tmp:
            home,python,windowless,_=self.paired(pathlib.Path(tmp));windowless.unlink()
            try:windowless.symlink_to(python)
            except OSError:self.skipTest('This account cannot create a symlink')
            with patch.object(installer.platform,'system',return_value='Windows'),patch.object(installer.sys,'executable',str(python)):
                with self.assertRaises(ValueError):installer.worker_python(home)
            self.assertTrue(windowless.is_symlink())
    def test_native_and_updater_commands_preserve_capture_and_add_only_no_window(self):
        for module,invoke in [(native,lambda:native.run(['fixture.exe','argument'],timeout=15)),(installer,lambda:installer.run('fixture.exe','argument'))]:
            with self.subTest(module=module.__name__),patch.object(module.sys,'platform','win32'),patch.object(module.subprocess,'CREATE_NO_WINDOW',0x08000000,create=True),patch.object(module.subprocess,'run',return_value=subprocess.CompletedProcess([],0,'result','')) as run:
                self.assertEqual(invoke(),'result');kwargs=run.call_args.kwargs
                self.assertEqual(kwargs['creationflags'],0x08000000);self.assertTrue(kwargs['capture_output']);self.assertTrue(kwargs['check']);self.assertTrue(kwargs['text'])
    def test_worker_captured_call_keeps_protocol_and_timeout(self):
        with patch.object(worker.sys,'platform','win32'),patch.object(worker.subprocess,'CREATE_NO_WINDOW',0x08000000,create=True),patch.object(worker.subprocess,'run',return_value=subprocess.CompletedProcess([],0,b'protocol',b'')) as run:
            self.assertEqual(worker.run(['fixture.exe'],timeout=30),b'protocol')
            self.assertEqual(run.call_args.kwargs,{'capture_output':True,'stdin':subprocess.DEVNULL,'timeout':30,'env':None,'creationflags':0x08000000})
    def test_all_worker_process_calls_have_explicit_window_policy(self):
        tree=ast.parse((ROOT/'ops/updates/worker.py').read_text());calls=[n for n in ast.walk(tree) if isinstance(n,ast.Call) and isinstance(n.func,ast.Attribute) and isinstance(n.func.value,ast.Name) and n.func.value.id=='subprocess' and n.func.attr in ('run','Popen')]
        self.assertGreaterEqual(len(calls),8)
        for call in calls:self.assertIn('creationflags',{keyword.arg for keyword in call.keywords})
    def test_native_provision_flags_do_not_change_unix(self):
        with patch.object(provision.sys,'platform','linux'):self.assertEqual(provision.child_creationflags(),0)
        with patch.object(provision.sys,'platform','win32'),patch.object(provision.subprocess,'CREATE_NO_WINDOW',0x08000000,create=True):self.assertEqual(provision.child_creationflags(),0x08000000)
    def test_worker_needs_no_stdout_or_stderr_for_its_controlled_idle_cycle(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp);registration=root/'registration.json'
            with patch.object(worker,'load_registration',return_value={'state':str(root)}),patch.object(worker,'Worker') as Worker,patch.object(worker.sys,'stdout',None),patch.object(worker.sys,'stderr',None):
                worker.run_worker(registration,once=True)
                Worker.return_value.tick.assert_called_once_with()
if __name__=='__main__':unittest.main()
