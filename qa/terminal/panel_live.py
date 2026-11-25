"""Linux: verify the shipping Panel against a disposable loopback Gateway.

Requires Python 3.11+ with websockets 15+, Node.js, Playwright, and Chromium.
"""

import argparse
import asyncio
import json
import os
import signal
import socket
import sys
import tempfile
from contextlib import asynccontextmanager, suppress
from pathlib import Path

import websockets


async def stop_owned(process, timeout):
    if process.returncode is None:
        with suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
        try:
            await asyncio.wait_for(process.wait(), timeout)
        except TimeoutError:
            with suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)
            await process.wait()
    return process.returncode


def owns_listener(pid, host, port):
    """On Linux, verify our child owns the exact IPv4 listening socket."""
    address = f'{socket.inet_aton(host)[::-1].hex().upper()}:{port:04X}'
    process = Path(f'/proc/{pid}')
    try:
        entries = (process / 'net/tcp').read_text(encoding='ascii').splitlines()[1:]
        listeners = {f'socket:[{fields[9]}]' for entry in entries
                     if (fields := entry.split())[1] == address and fields[3] == '0A'}
        if listeners:
            for fd in (process / 'fd').iterdir():
                with suppress(FileNotFoundError):
                    if os.readlink(fd) in listeners:
                        return True
    except FileNotFoundError:
        return False
    return False


async def wait_until_ready(host, port, process):
    uri = f'ws://127.0.0.1:{port}/ws'
    async with asyncio.timeout(25):
        while process.returncode is None:
            if not owns_listener(process.pid, host, port):
                await asyncio.sleep(0.1)
                continue
            try:
                async with websockets.connect(uri, open_timeout=1, proxy=None) as ws:
                    await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "connect",
                                              "params": {"client_type": "cli"}}))
                    response = json.loads(await asyncio.wait_for(ws.recv(), 2))
                    if "result" in response:
                        return
                    raise RuntimeError(f'Gateway connect error code {response["error"]["code"]}')
            except (OSError, TimeoutError):
                await asyncio.sleep(0.1)
        raise RuntimeError(f'Gateway exited before readiness: {process.returncode}')


@asynccontextmanager
async def disposable_gateway(server, *, paired_device=False):
    host = '127.0.0.1'
    if paired_device:
        # Remote-device admission requires a non-loopback source address. Never
        # enable the fixture's insecure listener outside an isolated namespace.
        process = await asyncio.create_subprocess_exec(
            'ip', '-j', 'address', 'show', stdout=asyncio.subprocess.PIPE,
        )
        output, _ = await process.communicate()
        interfaces = json.loads(output) if process.returncode == 0 else []
        if ([link['ifname'] for link in interfaces] != ['lo']
                or '192.0.2.23' not in {entry.get('local') for entry in interfaces[0]['addr_info']}):
            raise RuntimeError('paired-device fixture needs its own network namespace and test address')
        host = '0.0.0.0'

    root = Path(tempfile.mkdtemp(prefix='aleph-terminal-live.'))
    print(f'Isolated terminal evidence: {root}', flush=True)
    home = root / 'home'
    aleph_home = home / '.aleph'
    workspace = root / 'workspace'
    aleph_home.mkdir(parents=True)
    workspace.mkdir()
    with socket.socket() as reservation:
        reservation.bind((host, 0))
        port = reservation.getsockname()[1]
    config = f'''[gateway]
host = "{host}"
port = {port}
allow_insecure_remote = {str(paired_device).lower()}

[policies.terminal]
enabled = true

[agents.defaults]
workspace_root = {json.dumps(str(workspace))}

[providers.qa-local]
enabled = true
protocol = "anthropic"
base_url = "http://127.0.0.1:1"
api_key = "qa-not-a-real-key"
models = ["qa-model"]

[[agents.list]]
id = "main"
name = "QA"
default = true
model = "qa-model"
provider = "qa-local"
'''
    for section in ('cron', 'heartbeat', 'mcp', 'acp', 'evolution', 'memory', 'skills'):
        config += f'\n[{section}]\nenabled = false\n'
    (aleph_home / 'config.toml').write_text(config, encoding='utf-8')
    env = {
        'PATH': os.environ['PATH'],
        'LANG': 'C.UTF-8',
        'HOME': str(home),
        'ALEPH_HOME': str(aleph_home),
        'SHELL': '/bin/sh',
        'XDG_CONFIG_HOME': str(home / '.config'),
        'XDG_DATA_HOME': str(home / '.local' / 'share'),
        'XDG_CACHE_HOME': str(home / '.cache'),
        'NO_PROXY': '127.0.0.1,localhost,192.0.2.23',
        'RUST_MIN_STACK': '268435456',
    }
    gateway_log = root / 'gateway.log'
    with gateway_log.open('wb') as log:
        gateway = await asyncio.create_subprocess_exec(
            str(server), '--port', str(port), 'start',
            env=env, cwd=root, stdout=log, stderr=log, start_new_session=True,
        )
        try:
            await wait_until_ready(host, port, gateway)
            yield port, root, workspace, env
        finally:
            status = await stop_owned(gateway, 8)
            print(f'Owned Gateway stopped: {status}', flush=True)
    if status != 0:
        raise RuntimeError('Gateway did not cleanly stop after the probe')


async def probe(args):
    async with disposable_gateway(args.server) as (port, root, _, env):
        browser = await asyncio.create_subprocess_exec(
            'node', str(Path(__file__).with_name('panel_live.mjs')), f'http://127.0.0.1:{port}',
            str(args.dist), str(args.playwright), str(args.chromium),
            env=env, cwd=root, start_new_session=True,
        )
        try:
            status = await asyncio.wait_for(browser.wait(), 120)
            if status != 0:
                raise RuntimeError(f'Browser fixture failed (exit {status})')
        finally:
            await stop_owned(browser, 5)


def file_path(value):
    path = Path(value).expanduser().resolve()
    if not path.is_file():
        raise argparse.ArgumentTypeError(f'File not found: {path}')
    return path


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=file_path, required=True,
                        help='aleph-server built from this revision after regenerating shipping dist')
    parser.add_argument('--dist', type=lambda value: Path(value).expanduser().resolve(), required=True)
    parser.add_argument('--playwright', type=file_path, required=True, help='Path to playwright/index.mjs')
    parser.add_argument('--chromium', type=file_path, required=True)
    args = parser.parse_args()
    if not sys.platform.startswith('linux'):
        parser.error('this runner verifies listener ownership using Linux /proc')
    for executable in (args.server, args.chromium):
        if not os.access(executable, os.X_OK):
            parser.error(f'not executable: {executable}')
    if not (args.dist / 'aleph_panel_bg.wasm').is_file():
        parser.error('--dist must contain the freshly built aleph_panel_bg.wasm')
    asyncio.run(probe(args))
