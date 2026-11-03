#!/usr/bin/env python3
"""Real TLS/stdio identity events, without configuring or launching a backend."""
import argparse
import base64
import json
import pathlib
import select
import ssl
import struct
import subprocess
import tempfile
import time
import urllib.request
import zlib

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=pathlib.Path, default=pathlib.Path('target/debug/agent-instance'))
binary = parser.parse_args().binary.resolve()
namespace = 'com.microsoft.amplifier.identity'


def png():
    def chunk(kind, data):
        return struct.pack('!I', len(data)) + kind + data + struct.pack('!I', zlib.crc32(kind + data))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!2I5B', 32, 32, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress((b'\0' + bytes([30, 80, 90]) * 32) * 32)) + chunk(b'IEND', b'')


with tempfile.TemporaryDirectory(prefix='identity-events-') as directory:
    home = pathlib.Path(directory) / 'instance'
    subprocess.run([str(binary), 'init', '--home', str(home), '--name', 'Image fixture', '--bind', '127.0.0.1'], check=True, stdout=subprocess.DEVNULL)
    token = (home / 'secrets/control-token').read_text().strip()
    tls = ssl.create_default_context(cafile=str(home / 'tls/root-ca.crt'))
    server = subprocess.Popen([str(binary), 'serve', '--home', str(home)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    peers = []
    try:
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            try:
                base = json.loads((home / 'endpoints.json').read_text())['base_url']
                urllib.request.urlopen(base, context=tls, timeout=1).close()
                break
            except (OSError, ValueError):
                time.sleep(.05)
        else:
            raise AssertionError('Instance failed to become reachable')

        def api(path, data=None, method='GET'):
            request = urllib.request.Request(base + '/api/v1/' + path,
                data=json.dumps(data).encode() if data is not None else None, method=method,
                headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
            with urllib.request.urlopen(request, context=tls, timeout=4) as response:
                return json.load(response)

        def send(peer, identifier, method, params):
            peer.stdin.write((json.dumps({'jsonrpc': '2.0', 'id': identifier, 'method': method, 'params': params}) + '\n').encode())
            peer.stdin.flush()

        def receive(peer):
            ready, _, _ = select.select([peer.stdout], [], [], 5)
            assert ready, 'No identity response or notification'
            return json.loads(peer.stdout.readline())

        def connect():
            peer = subprocess.Popen([str(binary), 'acp', '--home', str(home)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            peers.append(peer)
            send(peer, 1, 'initialize', {'protocolVersion': 1, '_meta': {namespace: {'mode': 'observe'}}})
            assert receive(peer)['result']['_meta'][namespace]['observation_only'] is True
            send(peer, 2, '_amplifier/identity/subscribe', {})
            return peer, receive(peer)['result']

        peer, initial = connect()
        assert initial['display_name'] == 'Image fixture'
        status = api('status')
        assert status['enabled'] is False and status['backend']['configured'] is False
        assert all(c['backend'] is None for c in status['connections'])
        send(peer, 3, 'session/new', {})
        assert receive(peer)['error']['code'] == -32601
        api('identity/avatar', {'expected_revision': 0, 'media_type': 'image/png', 'data': base64.b64encode(png()).decode()}, 'PUT')
        event = receive(peer)
        assert event['method'] == '_amplifier/identity/changed'
        assert event['params']['avatar']['kind'] == 'uploaded'
        config = api('config')
        config['config']['name'] = 'Renamed fixture'
        api('config', {'expected_revision': config['revision'], 'config': config['config']}, 'PUT')
        assert receive(peer)['params']['display_name'] == 'Renamed fixture'
        api('identity/avatar?expected_revision=1', method='DELETE')
        removed = receive(peer)['params']
        assert removed['avatar']['kind'] == 'generated'
        peer.stdin.close()
        peer.wait(timeout=3)
        peer, current = connect()
        assert current == removed
        assert api('status')['sessions'] == []
        print(json.dumps({'passed': True, 'transport': 'TLS/WSS/stdio', 'backend_configured': False,
            'initial_snapshot': True, 'upload_rename_remove_events': True, 'reconnect_snapshot': True,
            'session_methods_rejected': True}))
    finally:
        for peer in peers:
            if peer.poll() is None:
                peer.kill()
                peer.wait()
        server.terminate()
        server.wait(timeout=5)
