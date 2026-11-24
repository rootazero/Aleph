#!/usr/bin/env python3
"""Exercise supplied Gateway/NSH builds without touching an installed service."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time


def sha256(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def fixture_port():
    while True:
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        if port != 18790:
            return port


def smoke(server, host, log):
    with tempfile.TemporaryDirectory(prefix='aleph-ipc-', dir='/tmp') as directory:
        root = Path(directory)
        home, aleph = root / 'home', root / 'aleph'
        home.mkdir(mode=0o700)
        aleph.mkdir(mode=0o700)
        port = fixture_port()
        (aleph / 'config.toml').write_text(f'''[gateway]
host = "127.0.0.1"
port = {port}
[agents]
[[agents.list]]
id = "main"
default = true
name = "Main Agent"
[memory]
enabled = false
[memory.dreaming]
enabled = false
[memory.profile]
enabled = false
[memory.embedding]
providers = []
active_provider_id = ""
[cron]
enabled = false
[heartbeat]
enabled = false
[mcp]
enabled = false
[acp]
enabled = false
[evolution]
enabled = false
[skills]
enabled = false
''', encoding='utf-8')
        env = {
            'PATH': os.environ['PATH'],
            'HOME': str(home),
            'ALEPH_HOME': str(aleph),
            'RUST_LOG': 'info',
            'NO_PROXY': '127.0.0.1,localhost',
        }
        endpoint = root / 'gw.sock'
        status_argv = [str(host), '--desktop-ipc-status', str(endpoint)]

        def run_host(argv=status_argv, *, environment=env):
            return subprocess.run(
                argv, cwd=root, env=environment, capture_output=True, text=True, timeout=20,
            )

        help_result = run_host([str(host), '--help'])
        require(help_result.returncode == 0 and '--desktop-ipc-status <socket>' in help_result.stdout,
                'NSH help did not advertise the status command')
        for result, diagnostic in [
            (run_host(), 'local system.hello'),
            (run_host(status_argv + ['--connect', 'http://127.0.0.1:1']), 'expected only'),
            (run_host(environment={**env, 'ALEPH_NSH_QA_URL': 'http://127.0.0.1:1'}), 'QA configuration'),
        ]:
            require(result.returncode != 0 and not result.stdout and diagnostic in result.stderr,
                    'NSH failed to refuse a missing or mixed endpoint as expected')
        print('PASS missing, mixed-mode and QA options refuse before launch', flush=True)

        server_argv = [
            str(server), '--bind', '127.0.0.1', '--port', str(port),
            '--bundled-content-only', '--desktop-ipc', str(endpoint), 'start',
        ]
        child = subprocess.Popen(
            server_argv, cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            deadline = time.monotonic() + 90
            while not endpoint.exists() and time.monotonic() < deadline and child.poll() is None:
                time.sleep(0.1)
            require(child.poll() is None and endpoint.exists(), 'isolated Gateway did not mount IPC')
            require(stat.S_ISSOCK(endpoint.lstat().st_mode), 'endpoint is not a Unix socket')
            require(stat.S_IMODE(root.lstat().st_mode) == 0o700, 'endpoint parent is not private')
            result = run_host()
            require(result.returncode == 0 and not result.stderr, 'NSH health command failed')
            reply = json.loads(result.stdout)
            require(reply['status'] == 'healthy' and reply['timestamp'], 'invalid health response')
            print('PASS built NSH host received real Gateway health via private UDS', flush=True)
            require(child.poll() is None, 'status must not stop the Gateway')
            child.send_signal(signal.SIGTERM)
            require(child.wait(timeout=15) == 0, 'Gateway did not shut down cleanly')
            require(stat.S_ISSOCK(endpoint.lstat().st_mode), 'caller-owned entry was not retained')
            result = run_host()
            require(result.returncode != 0 and not result.stdout and 'local system.hello' in result.stderr,
                    'NSH did not fail closed after shutdown')
            print('PASS listener retired; path retained and NSH refuses after shutdown', flush=True)
        finally:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('server', type=Path)
    parser.add_argument('host', type=Path)
    parser.add_argument('log', type=Path, help='new file for the isolated server log')
    args = parser.parse_args()
    if sys.platform != 'darwin' and not sys.platform.startswith('linux'):
        parser.error('Desktop IPC smoke requires native macOS or Linux')
    server = args.server.resolve(strict=True)
    host = args.host.resolve(strict=True)
    print(json.dumps({
        'server_sha256': sha256(server), 'host_sha256': sha256(host), 'platform': sys.platform,
    }), flush=True)
    with args.log.open('x', encoding='utf-8') as log:
        smoke(server, host, log)


if __name__ == '__main__':
    main()
