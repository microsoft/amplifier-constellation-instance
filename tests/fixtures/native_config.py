#!/usr/bin/env python3
"""A versioned native configuration CLI fixture; never calls a model."""
import hashlib, json, os, pathlib, sys
args=sys.argv[1:]; home=pathlib.Path(args[1]); action=args[2]; cfg=home/'runtime.json'
def revision(): return hashlib.sha256(cfg.read_bytes()).hexdigest()
if action=='config-schema':
 print(json.dumps({'type':'object','x-agent-runtime':{'contract_version':1,'provider_presets':[{'kind':'mock','endpoint':'https://example.invalid','api_key_env':'AGENT_MODEL_API_KEY'}]}}))
elif action=='config-inspect':
 print(json.dumps({'schema_version':1,'revision':revision(),'config':json.loads(cfg.read_text()),'capabilities':{},'restart_required':True,'credential_inherited':'AGENT_MODEL_API_KEY' in os.environ}))
else:
 candidate=json.loads(pathlib.Path(args[3]).read_text())
 if not candidate.get('provider',{}).get('model'):
  sys.stderr.write('Model name must not be empty');sys.exit(1)
 if action=='configure':
  if args[5]!=revision():sys.stderr.write('Configuration changed. Inspect it again before saving your changes.');sys.exit(1)
  cfg.write_text(json.dumps(candidate));print(json.dumps({'revision':revision(),'restart_required':True}))
 else:print(json.dumps({'valid':True}))
