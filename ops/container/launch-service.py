"""Host systemd entrypoint; no Docker control is passed into the container."""
import json, os, pathlib, signal, subprocess, sys
config=json.loads(pathlib.Path(sys.argv[1]).read_text())
command=config['services'][sys.argv[2]]
args=['/usr/bin/docker','exec','-i','--workdir',command['cwd']]
for key,value in command.get('env',{}).items():args.extend(['--env',key+'='+value])
args.extend([config['container'],'/usr/bin/python3','/opt/constellation/service.py',*command['argv']])
p=subprocess.Popen(args,stdin=subprocess.PIPE)
def stop(*_):
    if p.stdin:
        p.stdin.close()
        p.stdin=None
for sig in (signal.SIGINT,signal.SIGTERM):signal.signal(sig,stop)
sys.exit(p.wait())
