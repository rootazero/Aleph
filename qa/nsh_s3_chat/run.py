#!/usr/bin/env python3
"""S3 chat vertical-slice receipt: one isolated Gateway (no provider, so Core
runs the simulated engine and streams real `stream.*` frames), the TSX Chat
module in the NSH shell over Desktop IPC (macOS) and in Chrome over Web v1.

Reuses the S2 fixture (`qa/nsh_s2_shell/run.py`): temporary HOME/ALEPH_HOME,
random port (never 18790), self-signed TLS. Never touches an installed service.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location('s2_shell', HERE.parent / 'nsh_s2_shell' / 'run.py')
s2 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(s2)

# No spaces: the Mac this runs on may have a Chinese input method selected,
# where space picks a candidate instead of typing one.
DESKTOP_MESSAGE = 'S3desktophello'


def keys(pid, body):
    script = (
        f'tell application "System Events" to tell (first process whose unix id is {pid})\n'
        f'{body}\n'
        'end tell'
    )
    subprocess.run(['osascript', '-e', script], check=True, capture_output=True, timeout=20)


def type_and_send(desktop, text):
    """Keystrokes into the frontmost shell window (the Chat composer has
    focus). Under an input method the first Return commits the composition
    and must not send; the second sends. With a plain layout the first Return
    sends and the second lands in an empty composer: either way the web phase
    asserts the message reached Core exactly once."""

    def action(pid):
        keys(pid, f'  keystroke {json.dumps(text)}')
        time.sleep(1)
        keys(pid, '  key code 36')
        time.sleep(1.5)
        window = desktop.main_window(pid)
        subprocess.run(['screencapture', '-x', '-o', f'-l{window["id"]}',
                        str(desktop.out / 'desktop-first-return.png')], check=True, timeout=20)
        keys(pid, '  key code 36')
        # The simulated run takes well under a second; leave time for the
        # history refetch that follows its end.
        time.sleep(4)

    return action


def input_source():
    result = subprocess.run(
        ['defaults', 'read', str(Path.home() / 'Library/Preferences/com.apple.HIToolbox.plist'),
         'AppleSelectedInputSources'],
        capture_output=True, text=True, timeout=10,
    )
    return result.stdout.strip()


def web_phase(args, gateway, root, env, port, aleph, out):
    """Runs the Chrome driver; restarts the Gateway when the driver asks."""
    fixture = {
        'origin': f'https://127.0.0.1:{port}',
        'certificate': (aleph / 'data/tls/cert.pem').read_text(encoding='utf-8'),
        'ticket': s2.pairing_ticket(args.server, root, env),
        'desktop_message': DESKTOP_MESSAGE,
        'out': str(out),
    }
    fixture_path = root / 'web-fixture.json'
    fixture_path.write_text(json.dumps(fixture), encoding='utf-8')
    driver = subprocess.Popen(
        ['node', str(HERE / 'web-chat.cjs'), str(fixture_path)],
        env={**os.environ, 'NODE_PATH': str(args.node_modules)},
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    lines = []
    restarts = 0
    for line in driver.stdout:
        lines.append(line)
        if line.strip() == 'RESTART_GATEWAY':
            restarts += 1
            s2.require(gateway.stop() == 0, 'Gateway did not stop cleanly for the restart')
            gateway.start(f'gw-web-{restarts}.sock')
            driver.stdin.write(s2.pairing_ticket(args.server, root, env) + '\n')
            driver.stdin.flush()
    code = driver.wait(timeout=600)
    (out / 'web.log').write_text(''.join(lines), encoding='utf-8')
    s2.require(code == 0, 'web driver failed; see web.log')
    return json.loads((out / 'web-report.json').read_text(encoding='utf-8'))


def receipt(args):
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {
        'platform': sys.platform,
        'server_sha256': s2.sha256(args.server),
        'shell_sha256': s2.sha256(args.shell),
        'web_index_sha256': s2.sha256(args.web / 'index.html'),
        'input_source': input_source(),
    }
    with tempfile.TemporaryDirectory(prefix='aleph-s3-', dir='/tmp') as directory:
        root = Path(directory)
        home, aleph = root / 'home', root / 'aleph'
        home.mkdir(mode=0o700)
        aleph.mkdir(mode=0o700)
        port = s2.fixture_port()
        s2.write_config(aleph, port, args.web)
        env = {
            'PATH': os.environ['PATH'],
            'HOME': str(home),
            'ALEPH_HOME': str(aleph),
            'RUST_LOG': 'info',
            'NO_PROXY': '127.0.0.1,localhost',
        }
        shell_env = {**env, 'ALEPH_SHELL_PANEL': 'tsx', 'ALEPH_SHELL_HOTKEY': 'disabled'}
        desktop = s2.Desktop(args.shell, args.winlist, root, shell_env, out)
        with (out / 'gateway.log').open('w', encoding='utf-8') as log:
            gateway = s2.Gateway(args.server, root, env, port, log)
            try:
                gateway.start('gw-1.sock')
                ipc = {'ALEPH_SHELL_PANEL_IPC': str(gateway.endpoint)}
                phases = [desktop.run('connected', ipc, settle=6)]
                key, first = desktop.layout()
                s2.require(first['order'] == s2.tabs('overview', 'sessions', 'chat', 'capabilities'),
                           f'default layout lacks the Chat module: {first}')

                # Open Chat through the stored layout, type a message, Return.
                desktop.write_layout(key, {**first, 'active': s2.tabs('chat')[0]})
                phases.append(desktop.run('chat-sent', ipc, settle=6, action=type_and_send(desktop, DESKTOP_MESSAGE)))
                report['desktop'] = {'phases': phases, 'layout_key': key}
                print('PASS desktop: Chat opened, message typed and sent over Desktop IPC', flush=True)

                web = web_phase(args, gateway, root, env, port, aleph, out)
                report['web'] = web
                print('PASS web: cross-surface history, IME hold, live stream, Gateway restart', flush=True)
            finally:
                if gateway.child is not None:
                    report['gateway_exit'] = gateway.stop()
    (out / 'report.json').write_text(f'{json.dumps(report, indent=2, ensure_ascii=False)}\n', encoding='utf-8')
    print(json.dumps(report, ensure_ascii=False), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--shell', type=Path, required=True, help='aleph-nsh-shell built with --features tsx-panel')
    parser.add_argument('--web', type=Path, required=True, help='interfaces/panel/out-web/Aleph Panel')
    parser.add_argument('--winlist', type=Path, required=True, help='compiled qa/nsh_s2_shell/winlist.swift')
    parser.add_argument('--node-modules', type=Path, required=True, help='node_modules holding playwright')
    parser.add_argument('--out', type=Path, required=True, help='new directory for evidence')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        parser.error('the desktop surface is macOS-only')
    for name in ('server', 'shell', 'web', 'winlist', 'node_modules'):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    receipt(args)


if __name__ == '__main__':
    main()
