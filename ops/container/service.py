"""Keep a service inside the container tied to its host supervisor's lifetime."""
import os, signal, subprocess, sys, threading
p = subprocess.Popen(sys.argv[1:], stdin=subprocess.DEVNULL, start_new_session=True)
def stop(*_):
    try: os.killpg(p.pid, signal.SIGTERM)
    except ProcessLookupError: pass
for sig in (signal.SIGINT, signal.SIGTERM): signal.signal(sig, stop)
def control():
    while sys.stdin.buffer.read(1): pass
    stop()
threading.Thread(target=control, daemon=True).start()
try: code=p.wait()
finally: stop()
sys.exit(code)
