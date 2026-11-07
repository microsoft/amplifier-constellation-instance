import copy,json,sys,tempfile,unittest
from pathlib import Path
from unittest.mock import patch
import worker

class Connections(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.root=Path(self.temp.name)
        self.entry={'id':'world','service':'minecraft','label':'Workshop','revision':'r1','context_provider_ids':['world-context'],'commands':{a:[sys.executable,'/fixed/domain.py',a] for a in ['status','connect','disconnect']}}
        self.catalog={'schema':1,'services':[{'id':'minecraft','label':'Minecraft','setup_url':'https://example.test/setup'}],'connections':[self.entry]}
        self.command={'connection_id':'world','revision':'r1','action':'disconnect','request_id':'4e9cf830-1574-4a40-b6b6-d4a67352d184'}
        self.pending={'command':self.command};self.calls=[];self.receipts=[]
        self.w=object.__new__(worker.Worker);self.w.root=self.root;self.w.c={'service_connections':self.catalog}
        self.w.api=self.api
    def tearDown(self):self.temp.cleanup()
    def api(self,path,body=None):
        if body:
            self.receipts.append(copy.deepcopy(body));c=body['connections'][0]
            if c['request_id']==self.command['request_id'] and c['settled'] and c['state']=='disconnected':self.pending=None
        return {'request':self.pending}
    def call(self,entry,action,request_id):
        self.calls.append(action)
        if action=='disconnect':raise TimeoutError('reply lost after effects')
        return {'schema':1,'connection_id':'world','state':'unknown','request_id':request_id,'settled':False,'message':''}
    def test_lost_delivery_restart_reconciles_status_without_repeating_effects(self):
        with patch.object(worker,'connection_call',self.call):
            self.assertTrue(self.w.connection_tick());self.assertEqual(self.calls,['status','disconnect'])
            # Fresh worker with only the durable journal and unchanged enrollment.
            w=object.__new__(worker.Worker);w.root=self.root;w.c=self.w.c;w.api=self.api
            self.assertTrue(w.connection_tick());self.assertEqual(self.calls,['status','disconnect','status'])
        result={'schema':1,'connection_id':'world','state':'disconnected','request_id':self.command['request_id'],'settled':True,'message':''}
        with patch.object(worker,'connection_call',return_value=result):self.assertFalse(w.connection_tick())
        self.assertFalse((self.root/'connection-operation.json').exists())
    def test_changed_enrollment_after_unknown_dispatch_blocks(self):
        with patch.object(worker,'connection_call',self.call):self.w.connection_tick()
        self.entry['commands']['status']=[sys.executable,'/different.py']
        with patch.object(worker,'connection_call',self.call),self.assertRaisesRegex(ValueError,'enrollment_changed'):self.w.connection_tick()
    def test_unenrollment_does_not_release_unknown_operation(self):
        worker.write(self.root/'connection-operation.json',{'command':self.command})
        self.w.c={};self.assertTrue(self.w.connection_tick())
    def test_catalog_rejects_overlapping_context_unsafe_urls_and_relative_commands(self):
        worker.validate_service_connections(self.catalog)
        for mutate in [lambda c:c['services'][0].update(setup_url='http://remote/setup'),lambda c:c['connections'][0]['commands'].update(connect=['sh','-c','bad']),lambda c:c['connections'].append(dict(self.entry,id='other'))]:
            c=copy.deepcopy(self.catalog);mutate(c)
            with self.assertRaises(ValueError):worker.validate_service_connections(c)
    def test_actual_fixed_command_stdin_boundary_and_output_validation(self):
        script=self.root/'domain.py';script.write_text('import json,sys\nx=json.load(sys.stdin)\nprint(json.dumps(dict(schema=1,connection_id=x["connection_id"],state="disconnected",request_id=x["request_id"],settled=True,message="retained")))\n')
        self.entry['commands']['disconnect']=[sys.executable,str(script)]
        result=worker.connection_call(self.entry,'disconnect',self.command['request_id'])
        self.assertEqual(result['request_id'],self.command['request_id']);self.assertTrue(result['settled'])
        script.write_text('print("not-json")')
        with self.assertRaises(ValueError):worker.connection_call(self.entry,'disconnect',self.command['request_id'])
    def test_readiness_report_ack_precedes_next_domain_status_after_lost_http_reply(self):
        self.command['action']='connect';order=[];fail=[True]
        def domain(entry,action,request_id):
            order.append(action)
            return {'schema':1,'connection_id':'world','state':'busy','request_id':request_id,'settled':False,'context_ready':action=='connect','message':''}
        def api(path,body=None):
            if body:
                ready=body['connections'][0].get('context_ready',False);order.append('report_ready' if ready else 'report_gate')
                if ready and fail[0]:fail[0]=False;raise OSError('ack lost')
            return {'request':self.pending}
        self.w.api=api
        with patch.object(worker,'connection_call',domain):
            with self.assertRaises(OSError):self.w.connection_tick()
            self.assertTrue((self.root/'connection-report.json').exists())
            self.w.connection_tick()
        self.assertEqual(order[:6],['status','report_gate','connect','report_ready','report_ready','status'])

    def test_add_uses_fixed_provider_and_preserves_registration_and_idempotence(self):
        script=self.root/'add.py'
        script.write_text('import json,sys\nx=json.load(sys.stdin)\nconnection='+repr(self.entry)+'\nprint(json.dumps(dict(schema=1,service_id=x["service_id"],request_id=x["request_id"],state="saved",settled=True,message="Saved",connection=connection)))\n')
        service={'id':'minecraft','label':'Minecraft','add':{'revision':'t1','fields':[]},'add_commands':{a:[sys.executable,str(script)] for a in ['add','status']}}
        catalog={'schema':1,'services':[service],'connections':[]}
        registration={'schema':1,'home':str(self.root/'home'),'state':str(self.root/'state'),'components':[{'id':'test','provider':'command','commands':{a:[sys.executable,'/fixed.py'] for a in ['status','plan','apply','rollback']}}],'service_connections':catalog,'preserve_private':'unchanged'}
        self.w.registration=self.root/'registration.json';worker.write(self.w.registration,registration);self.w.c=copy.deepcopy(registration)
        journal=self.root/'connection-operation.json';command={'action':'add','service_id':'minecraft','revision':'t1','request_id':self.command['request_id'],'values':{}}
        result=self.w.add_connection(self.w.c['service_connections'],command,None,journal)
        self.assertTrue(result['settled']);self.assertEqual(result['connection_id'],'world')
        after=worker.read(self.w.registration);self.assertEqual(after['preserve_private'],'unchanged');self.assertEqual(after['components'],registration['components'])
        result=self.w.add_connection(self.w.c['service_connections'],command,worker.read(journal),journal)
        self.assertTrue(result['settled']);self.assertEqual(len(worker.read(self.w.registration)['service_connections']['connections']),1)
        # Same returned ID with a conflicting descriptor cannot overwrite a saved connection.
        after=worker.read(self.w.registration);after['service_connections']['connections'][0]['label']='Retained original';worker.write(self.w.registration,after)
        result=self.w.add_connection(self.w.c['service_connections'],command,worker.read(journal),journal)
        self.assertFalse(result['settled']);self.assertEqual(worker.read(self.w.registration)['service_connections']['connections'][0]['label'],'Retained original')
if __name__=='__main__':unittest.main()
