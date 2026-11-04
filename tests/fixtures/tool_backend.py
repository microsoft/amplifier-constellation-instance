#!/usr/bin/env python3
"""Generic ACP fixture: consume descriptors without persisting their credentials."""
import json
import os
import pathlib
import sys
import uuid

folder = pathlib.Path(sys.argv[1])
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    params = request.get('params', {})
    result = {}
    if method == 'initialize':
        result = {'protocolVersion': 1, 'agentCapabilities': {'loadSession': True, 'mcpCapabilities': {'http': os.environ.get('FIXTURE_HTTP') != 'no'}, 'sessionCapabilities': {'resume': {}}}}
    elif method in ('session/new', 'session/load', 'session/resume'):
        sid = uuid.uuid4().hex if method == 'session/new' else params['sessionId']
        if method == 'session/new':
            (folder / (sid + '.json')).write_text(json.dumps({'cwd': params['cwd']}))
        descriptors = params.get('mcpServers', [])
        report = [{'name': s['name'], 'type': s.get('type'), 'url': s.get('url'), 'headers': [h['name'] for h in s.get('headers', [])], 'secret_matches': any(h['value'] == 'Bearer private-mcp-canary' for h in s.get('headers', []))} for s in descriptors]
        result = {'sessionId': sid, '_meta': {'fixture': report, 'optional': [s.get('_meta', {}).get('io.github.bkrabach.agent-runtime', {}).get('optional', False) for s in descriptors]}}
    elif method == 'session/prompt':
        result = {'stopReason': 'end_turn'}
    if 'id' in request:
        print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
