"""Enrollment uses the guest's namespace without copying an installation home."""
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import uuid

ROOT = Path(__file__).parent
spec = importlib.util.spec_from_file_location('enroll', ROOT / 'install.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class IncusEnrollmentTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.home = self.root / 'guest-only-home'
        self.state = self.root / 'owner'
        self.bundle = self.root / 'bundle'
        self.bundle.mkdir()
        shutil.copyfile(ROOT / 'worker.py', self.bundle / 'worker.py')
        # The real Incus boundary has its own transport tests. This fixture
        # supplies only the observed identity to test installer publication.
        (self.bundle / 'incus_provider.py').write_text(
            'import json\nfrom pathlib import Path\nclass Provider:\n'
            ' def __init__(self,path): self.c=json.loads(Path(path).read_text())\n'
            ' def probe(self): return {"instance_id":self.c.get("observed_identity",self.c["instance_id"])}\n')
        self.identity = str(uuid.uuid4())
        self.participant = {'id':'domain','protocol':1,'components':['instance'],
                            'commands':{a:['/owner/domain',a] for a in ('prepare','verify','restore')}}
        self.config = {'schema':1,'instance_id':self.identity,'incus':'/usr/bin/incus',
                       'project':'default','guest':'fixture','home':str(self.home),
                       'binary':'/opt/fixture/bin/agent-instance','guest_python':'/usr/bin/python3',
                       'services':['fixture.service'],'instance_service':'fixture.service',
                       'state':str(self.state/'incus'),'participants':[self.participant]}
        self.config_path = self.root / 'mapping.json'

    def enroll(self):
        self.config_path.write_text(json.dumps(self.config))
        with patch.object(m.platform,'system',return_value='Linux'), \
             patch.object(m.Path,'home',return_value=self.root), \
             patch.object(m,'run'), \
             patch.object(m.subprocess,'run',return_value=subprocess.CompletedProcess([],0)):
            return m.enroll(self.home,self.state,[],self.bundle/'worker.py',incus_config=self.config_path)

    def test_guest_home_is_not_created_and_custom_backend_is_not_invented(self):
        self.enroll()
        registration=json.loads((self.state/'registration.json').read_text())
        self.assertFalse(self.home.exists())
        self.assertEqual(registration['home'],str(self.home))
        self.assertEqual(registration['services'],[])
        self.assertEqual(registration['slack_homes'],[])
        self.assertEqual([c['id'] for c in registration['components']],['instance'])
        self.assertEqual(registration['components'][0]['protocol'],2)
        self.assertEqual(registration['participants'],[self.participant])
        self.assertEqual(registration['management']['command'][-1],'management')
        self.assertEqual(json.loads((self.state/'incus.json').read_text()),self.config)

    def test_reenrollment_preserves_retained_history_and_additional_participant(self):
        self.enroll()
        registration=json.loads((self.state/'registration.json').read_text())
        extra={**self.participant,'id':'second'}
        registration['participants'].append(extra)
        (self.state/'registration.json').write_text(json.dumps(registration))
        state=json.loads((self.state/'state.json').read_text())
        state.update(completed_requests=['original'],unknown_receipts=['unchanged'])
        (self.state/'state.json').write_text(json.dumps(state))
        self.enroll()
        self.assertEqual(json.loads((self.state/'registration.json').read_text())['participants'],[self.participant,extra])
        after=json.loads((self.state/'state.json').read_text())
        self.assertEqual(after['completed_requests'],['original'])
        self.assertEqual(after['unknown_receipts'],['unchanged'])
        self.assertFalse(self.home.exists())

    def test_wrong_observed_identity_refuses_before_helper_publication(self):
        self.config['observed_identity']=str(uuid.uuid4())
        with self.assertRaisesRegex(ValueError,'identity_mismatch'):self.enroll()
        self.assertFalse((self.state/'registration.json').exists())
        self.assertFalse((self.state/'worker.py').exists())

    def test_changed_existing_participant_is_not_overwritten(self):
        self.enroll()
        before=(self.state/'registration.json').read_bytes()
        self.config['participants'][0]['commands']['restore']=['/different/owner','restore']
        with self.assertRaisesRegex(ValueError,'participant_changed'):self.enroll()
        self.assertEqual((self.state/'registration.json').read_bytes(),before)

    def test_guest_slack_paths_are_not_accepted_as_host_paths(self):
        with self.assertRaisesRegex(ValueError,'separate_participants'):
            m.enroll(self.home,self.state,[self.home/'slack'],self.bundle/'worker.py',incus_config=self.config_path)


if __name__=='__main__':unittest.main()
