#!/usr/bin/env python3
"""Run Windows Desktop IPC checks against supplied native Gateway and NSH builds."""

import argparse
import ctypes
from ctypes import wintypes
from contextlib import contextmanager
import hashlib
import json
import msvcrt
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import uuid

from memory_lifecycle.windows_console import (
    deliver_control_event,
    prepare_console_parent,
    spawn_creation_flags,
)


UNAVAILABLE = 'Desktop IPC is unavailable on this platform'
FILE_NOT_FOUND = 2


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def sha256(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()


def run(argv, root, env):
    return subprocess.run(
        argv, cwd=root, env=env, stdin=subprocess.DEVNULL, capture_output=True,
        text=True, encoding='utf-8', errors='replace', timeout=20,
    )


def require_missing_pipe(pipe):
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.WaitNamedPipeW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD]
    kernel.WaitNamedPipeW.restype = wintypes.BOOL
    ready = kernel.WaitNamedPipeW(pipe, 1)
    require(not ready and ctypes.get_last_error() == FILE_NOT_FOUND,
            'expected an absent pipe, not an occupied or access-denied pipe')


def refuse_baseline(server, host, root, pipe, env):
    require_missing_pipe(pipe)
    commands = {
        'gateway': [server, '--bind', '127.0.0.1', '--port', '0',
                    '--desktop-ipc', pipe, 'start'],
        'host': [host, '--desktop-ipc-status', pipe],
    }
    for name, argv in commands.items():
        result = run(argv, root, env)
        print(json.dumps({'command': name, 'exit': result.returncode,
                          'stdout': result.stdout, 'stderr': result.stderr}), flush=True)
        require(result.returncode != 0 and not result.stdout and UNAVAILABLE in result.stderr,
                f'{name} did not take the baseline platform refusal path')
        require_missing_pipe(pipe)
        require(not list(root.iterdir()), f'{name} touched the fresh fixture directory')
    print('PASS baseline refuses on both sides; no pipe or profile/lock created', flush=True)


def nondefault_loopback_port():
    while True:
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        if port != 18790:
            return port


def write_config(profile, port):
    profile.mkdir()
    (profile / 'config.toml').write_text(f'''[gateway]
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


def require_remote_rejected(pipe, root, env):
    # Use the same account via the real SMB redirector. The owner-only control
    # must accept before a denial on the candidate pipe counts as a pass.
    script = r'''
$ErrorActionPreference = 'Stop'
function Open-Remote([string]$Name) {
    $client = [IO.Pipes.NamedPipeClientStream]::new('127.0.0.1', $Name,
        [IO.Pipes.PipeDirection]::InOut, [IO.Pipes.PipeOptions]::None,
        [Security.Principal.TokenImpersonationLevel]::Identification)
    try { $client.Connect(1500); return 0 }
    catch { return ($_.Exception.GetBaseException().HResult -band 65535) }
    finally { $client.Dispose() }
}
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = [IO.Pipes.PipeSecurity]::new()
$acl.SetOwner($sid)
$acl.SetAccessRuleProtection($true, $false)
$acl.AddAccessRule([IO.Pipes.PipeAccessRule]::new($sid,
    [IO.Pipes.PipeAccessRights]::FullControl, [Security.AccessControl.AccessControlType]::Allow))
$name = 'aleph-s1b-control-' + [guid]::NewGuid()
$control = [IO.Pipes.NamedPipeServerStream]::new($name, [IO.Pipes.PipeDirection]::InOut, 1,
    [IO.Pipes.PipeTransmissionMode]::Byte, [IO.Pipes.PipeOptions]::Asynchronous, 0, 0, $acl)
try { $controlCode = Open-Remote $name } finally { $control.Dispose() }
$targetCode = Open-Remote $env:ALEPH_S1B_PIPE_NAME
Write-Output "control=$controlCode candidate=$targetCode"
if ($controlCode -ne 0 -or $targetCode -ne 5) { exit 1 }
'''
    result = run(['powershell', '-NoProfile', '-NonInteractive', '-Command', script],
                 root, {**env, 'ALEPH_S1B_PIPE_NAME': pipe.rsplit('\\', 1)[1]})
    require(result.returncode == 0 and result.stdout.strip() == 'control=0 candidate=5',
            f'SMB remote rejection not established: exit={result.returncode}, '
            f'{result.stdout.strip()} {result.stderr.strip()}')
    print('PASS same-account SMB control opened; candidate denied access (Win32 5)', flush=True)


@contextmanager
def established_peer(pipe):
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    create = kernel.CreateFileW
    create.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p,
                       wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    create.restype = wintypes.HANDLE
    peek = kernel.PeekNamedPipe
    peek.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD, ctypes.c_void_p,
                     ctypes.POINTER(wintypes.DWORD), ctypes.c_void_p]
    peek.restype = wintypes.BOOL
    # GENERIC_READ | GENERIC_WRITE, OPEN_EXISTING, and identification-level SQOS.
    handle = create(pipe, 0xC0000000, 0, None, 3, 0x110000, None)
    if handle == ctypes.c_void_p(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    with os.fdopen(msvcrt.open_osfhandle(handle, os.O_BINARY), 'r+b', buffering=0) as stream:
        def read_exact(size):
            deadline = time.monotonic() + 5
            available = wintypes.DWORD(0)
            while time.monotonic() < deadline:
                if not peek(handle, None, 0, None, ctypes.byref(available), None):
                    raise ctypes.WinError(ctypes.get_last_error())
                if available.value >= size:
                    data = stream.read(size)
                    require(len(data) == size, 'short pipe read')
                    return data
                time.sleep(0.01)
            raise TimeoutError('pipe peer response did not arrive')

        stream.write(b'\0')
        require(read_exact(1) == b'\0', 'same-account pipe prelude failed')
        hello = json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': 'system.hello', 'params': {
            'protocol': {'major': 1, 'minor': 0},
            'client': {'name': 'windows-fixture', 'version': '1', 'platform': 'windows',
                       'ui_runtime': 'test'},
            'features': [],
        }}).encode('utf-8')
        stream.write(len(hello).to_bytes(4, 'big') + hello)
        size = int.from_bytes(read_exact(4), 'big')
        require(0 < size <= 65536, 'invalid hello response size')
        response = json.loads(read_exact(size))
        require(response.get('id') == 1 and isinstance(response.get('result'), dict),
                'established peer did not complete hello')
        def require_eof():
            # Use the native result: the CRT can map PIPE_NOT_CONNECTED to EINVAL.
            available = wintypes.DWORD(0)
            open_pipe = peek(handle, None, 0, None, ctypes.byref(available), None)
            require(not open_pipe and ctypes.get_last_error() in (109, 233),
                    'established peer did not observe a broken or disconnected pipe')

        yield require_eof


def smoke(server, host, root, pipe, env, log):
    argv = [host, '--desktop-ipc-status', pipe]
    for command, environment, diagnostic in [
        (argv + ['--connect', 'http://127.0.0.1:1'], env, 'expected only'),
        (argv, {**env, 'ALEPH_PANEL_QA_STATUS': '1'}, 'QA configuration'),
    ]:
        result = run(command, root, environment)
        require(result.returncode != 0 and not result.stdout and diagnostic in result.stderr,
                f'NSH failed to refuse: exit={result.returncode}, stderr={result.stderr.strip()}')
    print('PASS mixed arguments and QA mode refused', flush=True)
    remote = rf'\\127.0.0.1\pipe\aleph-s1b-{uuid.uuid4()}'
    for command in [
        [server, '--bind', '127.0.0.1', '--port', '0', '--desktop-ipc', remote, 'start'],
        [host, '--desktop-ipc-status', remote],
    ]:
        result = run(command, root, env)
        require(result.returncode != 0 and not result.stdout and 'explicit local' in result.stderr,
                f'remote endpoint did not fail validation: exit={result.returncode}')
        require(not Path(env['ALEPH_HOME']).exists(), 'endpoint refusal touched the profile')
    print('PASS both real opt-in commands refused a remote namespace before profile creation',
          flush=True)
    port = nondefault_loopback_port()
    write_config(Path(env['ALEPH_HOME']), port)
    require(prepare_console_parent(), 'cannot enable the isolated console Ctrl-C path')
    for attempt in range(2):
        gateway_round(server, argv, root, pipe, env, log, port, attempt)
    print('PASS fresh Gateway rebound the same retired pipe', flush=True)


def gateway_round(server, argv, root, pipe, env, log, port, attempt):
    child = subprocess.Popen(
        [server, '--bind', '127.0.0.1', '--port', str(port), '--bundled-content-only',
         '--desktop-ipc', pipe, 'start'],
        cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT,
        creationflags=spawn_creation_flags(),
    )
    try:
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline and child.poll() is None:
            with socket.socket() as sock:
                sock.settimeout(0.2)
                if sock.connect_ex(('127.0.0.1', port)) == 0:
                    break
            time.sleep(0.1)
        else:
            raise RuntimeError(f'Gateway did not start: exit={child.poll()}; inspect --gateway-log')
        result = run(argv, root, env)
        require(result.returncode == 0 and not result.stderr,
                f'NSH status failed: exit={result.returncode}, stderr={result.stderr.strip()}')
        health = json.loads(result.stdout)
        require(health.keys() == {'status', 'timestamp'} and health['status'] == 'healthy'
                and isinstance(health['timestamp'], str) and health['timestamp'],
                'invalid canonical IPC health response')
        require(child.poll() is None, 'NSH must not stop the Gateway')
        print(json.dumps({'gate': 'real NSH/Gateway health', 'exit': 0, 'health': health}), flush=True)
        if attempt == 0:
            require_remote_rejected(pipe, root, env)
            missing = rf'\\.\pipe\aleph-s1b-{uuid.uuid4()}'
            require_missing_pipe(missing)
            result = run([argv[0], '--desktop-ipc-status', missing], root, env)
            require(result.returncode != 0 and not result.stdout
                    and 'local system.hello' in result.stderr,
                    'NSH did not refuse an absent local endpoint')
            require(child.poll() is None, 'Gateway stopped during the negative probes')
            print('PASS NSH refused an absent pipe while Gateway remained available', flush=True)
        with established_peer(pipe) as require_peer_eof:
            delivery = deliver_control_event(child.pid)
            require(delivery.get('generated'), 'owned console Ctrl-C was not delivered')
            require(child.wait(timeout=45) == 0, 'Gateway did not exit normally')
            require_peer_eof()
        require_missing_pipe(pipe)
        result = run(argv, root, env)
        require(result.returncode != 0 and not result.stdout, 'NSH still succeeds after shutdown')
        print('PASS owned Gateway exited on Ctrl-C; established peer EOF, pipe removed, NSH refused',
              flush=True)
    finally:
        if child.poll() is None:
            child.terminate()
            child.wait(timeout=15)
            print('CLEANUP forced termination; not graceful evidence', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['baseline', 'smoke'])
    parser.add_argument('server', type=Path)
    parser.add_argument('host', type=Path)
    parser.add_argument('--gateway-log', type=Path, help='exclusive new file for the smoke process log')
    args = parser.parse_args()
    if sys.platform != 'win32':
        parser.error('this fixture requires native Windows')
    if args.mode == 'smoke' and args.gateway_log is None:
        parser.error('smoke requires --gateway-log')
    server = args.server.resolve(strict=True)
    host = args.host.resolve(strict=True)
    print(json.dumps({'server_sha256': sha256(server), 'host_sha256': sha256(host),
                      'platform': sys.platform, 'mode': args.mode}), flush=True)
    with tempfile.TemporaryDirectory(prefix='aleph-s1b-') as directory:
        root = Path(directory)
        pipe = rf'\\.\pipe\aleph-s1b-{uuid.uuid4()}'
        env = {key: value for key, value in os.environ.items() if not key.startswith('ALEPH_')}
        env.update(ALEPH_HOME=str(root / 'profile'), RUST_LOG='info', NO_PROXY='127.0.0.1,localhost')
        if args.mode == 'smoke':
            home = root / 'home'
            roaming, local = home / 'AppData' / 'Roaming', home / 'AppData' / 'Local'
            roaming.mkdir(parents=True)
            local.mkdir()
            env.update(HOME=str(home), USERPROFILE=str(home), APPDATA=str(roaming),
                       LOCALAPPDATA=str(local))
        if args.mode == 'baseline':
            refuse_baseline(server, host, root, pipe, env)
        else:
            with args.gateway_log.open('xb') as log:
                smoke(server, host, root, pipe, env, log)


if __name__ == '__main__':
    main()
