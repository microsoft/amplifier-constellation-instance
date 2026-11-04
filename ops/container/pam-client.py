"""Forward one owner login to this installation's host-side PAM checker."""
import json, os, socket, sys
try:
    password = sys.stdin.buffer.read(1025).rstrip(b'\0').decode()
    if not password or len(password) > 1024:
        sys.exit(1)
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(20)
        client.connect('/run/constellation/owner-login.sock')
        client.sendall(json.dumps({'username': os.environ.get('PAM_USER', ''), 'password': password}).encode() + b'\n')
        result = client.recv(64)
    sys.exit(0 if result == b'OK\n' else 1)
except Exception:
    sys.exit(1)
