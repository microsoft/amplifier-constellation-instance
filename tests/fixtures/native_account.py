#!/usr/bin/env python3
"""Independent fake OAuth callback and native CLI; never contacts a provider."""
import hashlib, http.server, json, os, pathlib, socketserver, sys, urllib.parse

args=sys.argv[1:]; home=pathlib.Path(args[1]); action=args[2]
config=home/'runtime.json'; credentials=home/'accounts/chatgpt/credentials.json'
def revision(): return hashlib.sha256(config.read_bytes()).hexdigest()
def public():
    return {'connected':credentials.exists(),'sharing':credentials.exists(),'email':'fixture@example.invalid','access_token':'must-never-leave-engine'}
if action=='diagnostics': print(json.dumps({'profile':'agent-runtime.observability/1','counts':{'model.started':12},'model_step_latency_ms':{'p50':1200,'p95':4500},'latest_context':{'model_context_bytes':12345},'latest_stop':{'reason':'completed'},'reported_usage':{'measured_responses':11,'cache_read_tokens':800},'coverage':{'sampled_events':100,'complete_store_window':True}}))
elif action=='chatgpt-status': print(json.dumps(public()))
elif action=='chatgpt-models': print(json.dumps({'models':[{'slug':'retired-account-model','display_name':'Retired model'},{'slug':'fixture-account-model','display_name':'Fixture account model','private':'do not forward'}]}))
elif action=='chatgpt-sign-out':
    credentials.unlink(missing_ok=True); print(json.dumps({'connected':False,'revoked':True,'registration_preserved':True}))
elif action=='chatgpt-sign-in':
    assert 'AGENT_MODEL_API_KEY' not in os.environ
    (home/'oauth-pid').write_text(str(os.getpid()))
    mode=(home/'oauth-mode').read_text().strip() if (home/'oauth-mode').exists() else 'normal'
    class Callback(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200);self.end_headers();self.wfile.write(b'Fake consent received')
        def log_message(self,*args): pass
    # HTTPServer.server_bind performs reverse DNS. This local callback fixture
    # must work even when a hosted runner's resolver is unavailable.
    server=socketserver.TCPServer(('127.0.0.1',int(args[-1])),Callback)
    callback=f'http://127.0.0.1:{server.server_address[1]}/auth/callback'
    authorization='https://auth.openai.com/api/accounts/authorize?'+urllib.parse.urlencode({'redirect_uri':callback,'state':'ephemeral-fixture-state'})
    if mode=='bad-url': authorization='https://untrusted.invalid/login'
    print(json.dumps({'authorization_url':authorization,'callback':callback,'expires_in_seconds':1 if mode=='expire' else 300}),flush=True)
    server.handle_request()
    credentials.parent.mkdir(parents=True,exist_ok=True);credentials.write_text('fake-secret-fixture-only');credentials.chmod(0o600)
    print(json.dumps({**public(),'credentials_file':str(credentials)}))
elif action=='config-schema':
    metadata={'contract_version':1,'provider_presets':[{'kind':'openai','model':'fixture-account-model','endpoint':'https://api.openai.com/v1/responses','api_key_env':'OPENAI_API_KEY'},{'kind':'anthropic','model':'fixture-anthropic-model','endpoint':'https://api.anthropic.com/v1/messages','api_key_env':'ANTHROPIC_API_KEY'}],'model_catalog':{'openai':[{'class':'standard','model':'fixture-account-model'}],'anthropic':[{'class':'standard','model':'fixture-anthropic-model'}]}}
    mode=(home/'catalog-mode').read_text().strip() if (home/'catalog-mode').exists() else 'normal'
    if mode=='legacy': metadata.pop('model_catalog')
    elif mode=='empty': metadata['model_catalog']['openai']=[]
    elif mode=='malformed': metadata['model_catalog']['openai']='invalid'
    elif mode=='missing': metadata['model_catalog'].pop('openai')
    print(json.dumps({'type':'object','x-agent-runtime':metadata}))
elif action=='config-inspect':
    value=json.loads(config.read_text());valid=value['provider']['model']!='previous-model'
    print(json.dumps({'schema_version':1,'revision':revision(),'config':value,'validation':{'valid':valid,'error':None if valid else 'This model is outside the current supported classes'},'capabilities':{} if valid else None,'restart_required':True}))
elif action in ('config-validate','configure'):
    candidate=json.loads(pathlib.Path(args[3]).read_text())
    if not candidate.get('provider',{}).get('model'):sys.stderr.write('Model is required');sys.exit(1)
    if action=='configure':
        if args[5]!=revision():sys.stderr.write('Configuration changed');sys.exit(1)
        config.write_text(json.dumps(candidate));print(json.dumps({'revision':revision(),'restart_required':True}))
    else: print(json.dumps({'valid':True}))
else: sys.exit(2)
