"""Run only the installation's configured services and fixed loopback routes."""
import asyncio, json, os, pathlib, signal

async def pump(reader, writer):
    try:
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()
    except (OSError, ConnectionError):
        pass
    finally:
        writer.close()

async def forward(reader, writer, port):
    try:
        other_reader, other_writer = await asyncio.open_unix_connection('/run/constellation/tcp-' + str(port) + '.sock')
        await asyncio.gather(pump(reader, other_writer), pump(other_reader, writer))
    except OSError:
        writer.close()

async def main():
    config = json.loads(pathlib.Path('/etc/constellation-container.json').read_text())
    os.umask(0o077)
    pathlib.Path(os.environ['HOME']).mkdir(parents=True, exist_ok=True)
    servers = []
    for port in config.get('loopback_ports', []):
        servers.append(await asyncio.start_server(lambda r,w,p=port: forward(r,w,p), '127.0.0.1', port))
    processes = []
    stop = asyncio.Event()
    for sig in (signal.SIGTERM,signal.SIGINT): asyncio.get_running_loop().add_signal_handler(sig,stop.set)
    async def watch(command):
        while not stop.is_set():
            process = await asyncio.create_subprocess_exec(*command['argv'], cwd=command['cwd'], env={**os.environ, **command.get('env', {})})
            processes.append(process)
            await process.wait()
            processes.remove(process)
            if not stop.is_set():
                await asyncio.sleep(3)
    tasks = [asyncio.create_task(watch(command)) for command in config['services']]
    await stop.wait()
    for process in processes:
        try: process.terminate()
        except ProcessLookupError: pass
    if processes:
        try: await asyncio.wait_for(asyncio.gather(*(p.wait() for p in processes)), 25)
        except asyncio.TimeoutError:
            for process in processes:
                try: process.kill()
                except ProcessLookupError: pass
    for task in tasks: task.cancel()
    for server in servers: server.close()

asyncio.run(main())
