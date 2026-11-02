#!/usr/bin/env python3
"""Explicit, opt-in model call through an installed default runtime and the ACP relay.

Uses a disposable copy of runtime/account files. Never enables the source agent,
changes its draft/setup state, or sends platform messages. Requires installed SDK.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--source-home', required=True, type=Path)
parser.add_argument('--allow-model-call', action='store_true', required=True)
parser.add_argument('--binary', type=Path, default=Path('target/release/agent-instance'))
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
binary = args.binary.resolve()
source = args.source_home.resolve()
os.umask(0o077)
with tempfile.TemporaryDirectory(prefix='agent-default-live-') as directory:
    home = Path(directory) / 'instance'
    subprocess.run([str(binary), 'init', '--home', str(home), '--name', 'Default runtime validation', '--bind', '127.0.0.1'], check=True, stdout=subprocess.DEVNULL)
    shutil.copytree(source / 'backend/runtime', home / 'backend/runtime')
    (home / 'backend/provider').mkdir(mode=0o700)
    for name in ['auth.json', 'config.toml', 'account-source.json']:
        original = source / 'backend/provider' / name
        if original.is_file():
            shutil.copy2(original, home / 'backend/provider' / name)
    doc = json.loads((home / 'config.json').read_text())
    doc['config']['backend'] = json.loads((source / 'config.json').read_text())['config']['backend']
    assert doc['config']['backend']['command'].startswith('${AGENT_HOME}/backend/runtime/'), 'Use a default-runtime source'
    doc['config'].update(setup_complete=True, enabled=True)
    (home / 'config.json').write_text(json.dumps(doc))
    with (home / 'validation.log').open('w') as log:
        service = subprocess.Popen([str(binary), 'serve', '--home', str(home)], stdout=log, stderr=log)
        try:
            for _ in range(100):
                if (home / 'endpoints.json').exists(): break
                if service.poll() is not None: raise RuntimeError('Test service did not start')
                time.sleep(.1)
            result = subprocess.run([os.environ.get('AGENT_INSTANCE_TEST_NODE','node'), str(repo/'tests/acp-client/default-runtime.mjs'), str(home/'workspace'), str(binary), str(home)], capture_output=True, text=True, timeout=110)
            if result.returncode:
                raise RuntimeError('Live default runtime did not complete its test: ' + result.stderr[-2500:])
            report = json.loads(result.stdout)
            activity = subprocess.run([str(binary), 'api', 'GET', 'activity?category=outcomes', '--home', str(home)], check=True, capture_output=True, text=True)
            outcomes = json.loads(activity.stdout)['items']
            assert any(item['kind'] == 'acp.prompt.finished' for item in outcomes)
            print(json.dumps({**report, 'activity_outcome_recorded':True, 'source_agent_enabled_or_edited':False}))
        finally:
            service.send_signal(signal.SIGTERM)
            try: service.wait(timeout=10)
            except subprocess.TimeoutExpired: service.kill(); service.wait()
