"""Per-installation fixed local routes and PAM authentication; never a shell bridge.

Runs as the installing OS account. The private config is not mounted in the
container. Only socket files are shared. No requests can choose a destination,
PAM service, command, or executable. Passwords are neither logged nor saved.
"""
import asyncio, collections, json, os, pathlib, pwd, signal, sys, time

async def pump(reader, writer):
    try:
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()
    except (OSError, ConnectionError):
        pass
    finally:
        writer.close()

async def forward(reader, writer, target):
    try:
        other_reader, other_writer = await asyncio.wait_for(asyncio.open_connection(*target), 10)
        await asyncio.gather(pump(reader, other_writer), pump(other_reader, writer))
    except (OSError, asyncio.TimeoutError):
        writer.close()

def pam_check(owner, password):
    import PAM
    answered = False
    def conversation(_auth, messages, _data):
        nonlocal answered
        result = []
        for message, kind in messages:
            if kind == PAM.PAM_PROMPT_ECHO_OFF and not answered and message.strip().lower() == 'password:':
                answered = True
                result.append((password, 0))
            elif kind in (PAM.PAM_TEXT_INFO, PAM.PAM_ERROR_MSG):
                result.append(('', 0))
            else:
                raise ValueError('Unsupported login prompt')
        return result
    try:
        auth = PAM.pam()
        auth.start('agent-instance', owner, conversation)
        auth.authenticate()
        auth.acct_mgmt()
        return True
    except Exception:
        return False

async def main(config):
    owner = pwd.getpwuid(os.getuid()).pw_name
    sockets = pathlib.Path(config['socket_directory'])
    sockets.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(sockets, 0o700)
    gate = asyncio.Semaphore(1)
    attempts = collections.deque()
    async def login(reader, writer):
        success = False
        try:
            async with gate:
                now = time.monotonic()
                while attempts and attempts[0] < now - 60:
                    attempts.popleft()
                if len(attempts) >= 5:
                    return
                attempts.append(now)
                raw = await asyncio.wait_for(reader.readline(), 5)
                if len(raw) > 8192:
                    return
                value = json.loads(raw)
                if value.get('username') == owner and isinstance(value.get('password'), str) and 0 < len(value['password']) <= 1024:
                    # A timed-out call retains its slot until the PAM worker ends.
                    success = await asyncio.to_thread(pam_check, owner, value['password'])
        except Exception:
            pass
        finally:
            writer.write(b'OK\n' if success else b'NO\n')
            try: await writer.drain()
            except OSError: pass
            writer.close()
    servers = []
    auth = sockets / 'owner-login.sock'
    auth.unlink(missing_ok=True)
    servers.append(await asyncio.start_unix_server(login, str(auth), limit=8192))
    os.chmod(auth, 0o600)
    for route in config.get('routes', []):
        path = sockets / ('tcp-' + str(int(route['listen'])) + '.sock')
        path.unlink(missing_ok=True)
        host, port = route['target']
        callback = lambda r, w, target=(host, int(port)): forward(r, w, target)
        servers.append(await asyncio.start_unix_server(callback, str(path)))
        os.chmod(path, 0o600)
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGTERM, signal.SIGINT): loop.add_signal_handler(sig, stop.set)
    await stop.wait()
    for server in servers: server.close()
    await asyncio.gather(*(s.wait_closed() for s in servers))

if __name__ == '__main__':
    os.umask(0o077)
    asyncio.run(main(json.loads(pathlib.Path(sys.argv[1]).read_text())))
