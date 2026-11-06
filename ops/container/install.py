"""Install one already-configured Linux installation in a non-root container.

Private manifest and deployment state stay outside the source repository.
Requires an owner-reviewed manifest; this is an admin tool, not an agent tool.
"""
import argparse, ipaddress, json, os, pathlib, pwd, re, shutil, subprocess

# Package installation needs these ordinary process/file capabilities. Never
# include host administration, raw sockets, device creation or tracing caps.
ADMIN_CAPABILITIES = ('CHOWN', 'DAC_OVERRIDE', 'FOWNER', 'FSETID', 'KILL',
                      'SETGID', 'SETUID', 'SETFCAP', 'SYS_CHROOT')

def isolation_arguments(container_admin=False):
    args = ['--cap-drop', 'ALL']
    if container_admin:
        for capability in ADMIN_CAPABILITIES:
            args.extend(['--cap-add', capability])
    else:
        args.extend(['--security-opt', 'no-new-privileges', '--read-only'])
    return args

def validate_manifest(manifest):
    """Fail before creating files or touching services; manifests are admin input."""
    if not isinstance(manifest.get('container_admin', False), bool):
        raise ValueError('container_admin must be an explicit boolean')
    if not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,62}', manifest['container']):
        raise ValueError('Invalid container name')
    state = pathlib.Path(manifest['state'])
    if not state.is_absolute() or any(c.isspace() or c in '%\\"' for c in str(state)):
        raise ValueError('State directory must be absolute, without whitespace or unit substitutions')
    if ipaddress.ip_address(manifest['ip']) not in ipaddress.ip_network('172.30.240.0/24'):
        raise ValueError('Container address must be on the isolated bridge')
    prohibited = [pathlib.Path(p).resolve() for p in ['/', '/root', '/home', '/etc', '/proc', '/sys', '/dev', '/run', '/var/run', '/var', '/var/lib', '/var/lib/docker', str(pathlib.Path.home())]]
    for mount in manifest['mounts']:
        path = pathlib.Path(mount['path']).resolve()
        if path in prohibited or any(path.is_relative_to(p) for p in [pathlib.Path('/proc'), pathlib.Path('/sys'), pathlib.Path('/dev'), pathlib.Path('/run')]) or path.is_relative_to(pathlib.Path.home()/'.ssh'):
            raise ValueError('Refusing host control or broad host mount')
        if not path.exists() or not pathlib.Path(mount.get('target', str(path))).is_absolute():
            raise ValueError('Mount source must exist and target must be absolute')
    for port in manifest['ports']:
        ipaddress.ip_address(port.get('bind', '127.0.0.1'))
        if not all(isinstance(port[k], int) and 1 <= port[k] <= 65535 for k in ['host', 'container']):
            raise ValueError('Invalid published port')
    for unit, command in manifest['services'].items():
        if not re.fullmatch(r'[a-zA-Z0-9_.@-]+\.service', unit):
            raise ValueError('Invalid service name')
        if not command['argv'] or not all(isinstance(x, str) for x in command['argv']) or not pathlib.Path(command['cwd']).is_absolute():
            raise ValueError('Invalid service command')
    for route in manifest.get('routes', []):
        ipaddress.ip_address(route['target'][0])
        if not all(isinstance(p, int) and 1 <= p <= 65535 for p in [route['listen'], route['target'][1]]):
            raise ValueError('Invalid fixed route')

def run(*args):
    return subprocess.run(args, check=True, text=True, capture_output=True).stdout.strip()

def install(manifest):
    validate_manifest(manifest)
    if subprocess.run(['docker','inspect',manifest['container']],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode == 0:
        raise ValueError('Container already exists; stop and back it up before an explicit replacement')
    admin = manifest.get('container_admin', False)
    if admin:
        labels = json.loads(run('docker', 'image', 'inspect', '--format', '{{json .Config.Labels}}', manifest['image'])) or {}
        if (labels.get('com.microsoft.constellation.container-admin') != '1'
                or labels.get('com.microsoft.constellation.owner-uid') != str(os.getuid())
                or labels.get('com.microsoft.constellation.owner-gid') != str(os.getgid())):
            raise ValueError('Build Dockerfile.admin for this owner before enabling container_admin')
    source=pathlib.Path(__file__).resolve().parent
    state=pathlib.Path(manifest['state']).resolve();state.mkdir(parents=True,mode=0o700,exist_ok=True)
    os.chmod(state,0o700)
    for name in ['bridge.py','launch-service.py','seccomp.json']:
        shutil.copy2(source/name,state/name)
    (state/'sockets').mkdir(mode=0o700,exist_ok=True)
    (state/'home').mkdir(mode=0o700,exist_ok=True)
    owner=pwd.getpwuid(os.getuid()).pw_name
    (state/'passwd').write_text(f'root:x:0:0:root:/root:/bin/sh\n{owner}:x:{os.getuid()}:{os.getgid()}::/home/agent:/bin/bash\nnobody:x:65534:65534::/nonexistent:/usr/sbin/nologin\n')
    name=manifest['container']
    if not name.replace('-','').isalnum():raise ValueError('Invalid container name')
    services=manifest['services']
    (state/'services.json').write_text(json.dumps({'container':name,'services':services},indent=2))
    (state/'bridge.json').write_text(json.dumps({'socket_directory':str(state/'sockets'),'routes':manifest.get('routes',[])},indent=2))
    (state/'container.json').write_text(json.dumps({'services':[],'loopback_ports':[r['listen'] for r in manifest.get('routes',[])], 'container_admin': admin, 'command_boundary': 'container' if admin else 'workspace'}))
    # A private bridge; peer traffic is disabled and host firewall controls egress.
    network='constellation-isolated'
    if subprocess.run(['docker','network','inspect',network],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode:
        run('docker','network','create','--subnet','172.30.240.0/24','--opt','com.docker.network.bridge.name=br-constel','--opt','com.docker.network.bridge.enable_icc=false',network)
    args=['docker','create','--name',name,'--hostname',name,'--network',network,'--ip',manifest['ip'],'--user',f'{os.getuid()}:{os.getgid()}',
          *isolation_arguments(admin), '--security-opt','apparmor=constellation-installation',
          '--security-opt','seccomp='+str(state/'seccomp.json'),'--security-opt','systempaths=unconfined',
          '--pids-limit','512','--memory','6g','--memory-swap','6g','--cpus','4',
          '--sysctl','net.ipv6.conf.all.disable_ipv6=1','--tmpfs','/tmp:rw,nosuid,nodev,size=1g',
          '--tmpfs','/run:rw,nosuid,nodev,size=16m',
          '--mount',f'type=bind,src={state}/sockets,dst=/run/constellation,readonly',
          '--mount',f'type=bind,src={state}/container.json,dst=/etc/constellation-container.json,readonly',
          '--mount',f'type=bind,src={state}/home,dst=/home/agent',
          '--label','com.microsoft.constellation.isolation=installation']
    if not admin:
        args.extend(['--mount',f'type=bind,src={state}/passwd,dst=/etc/passwd,readonly'])
    for mount in manifest['mounts']:
        path=str(pathlib.Path(mount['path']).resolve())
        if path in ['/',str(pathlib.Path.home()),'/var/run/docker.sock','/run/docker.sock','/etc']:
            raise ValueError('Refusing broad host mount')
        args.extend(['--mount',f'type=bind,src={path},dst={mount.get("target",path)}'+(',readonly' if mount.get('readonly') else '')])
    for port in manifest['ports']:
        args.extend(['--publish',f'{port.get("bind","127.0.0.1")}:{port["host"]}:{port["container"]}'])
    for key,value in manifest.get('env',{}).items():args.extend(['--env',key+'='+value])
    for host,ip in manifest.get('hosts',{}).items():args.extend(['--add-host',host+':'+ip])
    args.append(manifest['image'])
    run(*args)
    units=pathlib.Path.home()/'.config/systemd/user';units.mkdir(parents=True,exist_ok=True)
    bridge=name+'-bridge.service';container=name+'.service'
    (units/bridge).write_text(f'''[Unit]
Description=Owner login and fixed local routes for {name}
Wants={container}
[Service]
ExecStart=/usr/bin/python3 {state}/bridge.py {state}/bridge.json
Restart=on-failure
RestartSec=2
UMask=0077
MemoryMax=128M
TasksMax=64
LimitNOFILE=256
[Install]
WantedBy=default.target
''')
    (units/container).write_text(f'''[Unit]
Description=Isolated installation {name}
Requires={bridge}
After={bridge} network-online.target
Wants={' '.join(services)}
PartOf={bridge}
[Service]
ExecStartPre=/usr/bin/systemctl is-active --quiet constellation-container-firewall.service
ExecStartPre=/usr/bin/test -S {state}/sockets/owner-login.sock
ExecStart=/usr/bin/docker start --attach {name}
ExecStop=/usr/bin/docker stop --time 35 {name}
Restart=on-failure
RestartSec=3
TimeoutStopSec=45
[Install]
WantedBy=default.target
''')
    for unit in services:
        drop=units/(unit+'.d');drop.mkdir(exist_ok=True)
        (drop/'50-container.conf').write_text(f'''[Unit]
Requires={container}
After={container}
PartOf={container}
[Service]
ExecStart=
ExecStart=/usr/bin/python3 {state}/launch-service.py {state}/services.json {unit}
TimeoutStopSec=40
''')
    run('systemctl','--user','daemon-reload')
    run('systemctl','--user','enable',bridge,container)
    run('systemctl','--user','start',bridge)
    import time
    for _ in range(100):
        if (state/'sockets/owner-login.sock').is_socket():break
        time.sleep(.1)
    run('systemctl','--user','start',container)
    for _ in range(100):
        if run('docker','inspect','--format','{{.State.Running}}',name)=='true':break
        time.sleep(.1)
    for unit in services:run('systemctl','--user','start',unit)
    (state/'manifest.json').write_text(json.dumps(manifest,indent=2))
    print(json.dumps({'container':name,'services':list(services),'installed':True}))

if __name__=='__main__':
    os.umask(0o077)
    parser=argparse.ArgumentParser();parser.add_argument('manifest',type=pathlib.Path)
    install(json.loads(parser.parse_args().manifest.read_text()))
