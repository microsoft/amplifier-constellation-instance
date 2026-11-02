#!/usr/bin/env python3
"""Independent fake OAuth callback and native CLI; never contacts a provider."""
import hashlib, http.server, json, os, pathlib, socketserver, sys, urllib.parse

args=sys.argv[1:]; home=pathlib.Path(args[1]); action=args[2]
config=home/'runtime.json'; credentials=home/'accounts/chatgpt/credentials.json'
def revision(): return hashlib.sha256(config.read_bytes()).hexdigest()
def public():
    return {'connected':credentials.exists(),'sharing':credentials.exists(),'email':'fixture@example.invalid','access_token':'must-never-leave-engine'}
if action=='chatgpt-status': print(json.dumps(public()))
elif action=='chatgpt-models': print(json.dumps({'models':[{'slug':'fixture-account-model','display_name':'Fixture account model','private':'do not forward'}]}))
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
elif action=='config-schema': print(json.dumps({'type':'object','x-agent-runtime':{'contract_version':1,'provider_presets':[{'kind':'openai','endpoint':'https://api.openai.com/v1/responses','api_key_env':'OPENAI_API_KEY'}]}}))
elif action=='config-inspect': print(json.dumps({'schema_version':1,'revision':revision(),'config':json.loads(config.read_text()),'capabilities':{},'restart_required':True}))
elif action in ('config-validate','configure'):
    candidate=json.loads(pathlib.Path(args[3]).read_text())
    if not candidate.get('provider',{}).get('model'):sys.stderr.write('Model is required');sys.exit(1)
    if action=='configure':
        if args[5]!=revision():sys.stderr.write('Configuration changed');sys.exit(1)
        config.write_text(json.dumps(candidate));print(json.dumps({'revision':revision(),'restart_required':True}))
    else: print(json.dumps({'valid':True}))
else: sys.exit(2)
