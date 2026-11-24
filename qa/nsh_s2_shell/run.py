#!/usr/bin/env python3
"""S2 customizable shell receipt: one isolated Gateway, the embedded TSX Panel
in the NSH shell (macOS) and the same TSX package in Chrome.

Never touches an installed service, the default port 18790 or the user's home:
the Gateway, the shell and the browser all run against a temporary root.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
PANEL_APP_ID = 'com.aleph.panel'
LAYOUT_PREFIX = 'aleph.panel.layout.v1:desktop-ipc:'


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


def write_config(aleph, port, panel_web_dir, memory_enabled=False):
    (aleph / 'config.toml').write_text(f'''[gateway]
host = "127.0.0.1"
port = {port}
panel_web_dir = {json.dumps(str(panel_web_dir))}
[gateway.tls]
enabled = true
[agents]
[[agents.list]]
id = "main"
default = true
name = "Main Agent"
[memory]
enabled = {'true' if memory_enabled else 'false'}
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


def seed_nebula(aleph, dense):
    root = aleph / 'memory/note'
    categories = ('main', 'main__u-owner', 'main__u-nebula-foreign')
    frontmatter = '---\ncategory: reference\ntags: [nebulaqa]\ncreated: 2026-04-01\nupdated: 2026-04-10\n---\n\n'
    for partition in categories:
        (root / partition / 'reference').mkdir(parents=True)
    own = root / 'main__u-owner/reference'
    for source in range(100):
        count = 24 if dense else 1
        links = ' '.join(f'[[reference/nebula-{(source + offset) % 100:03d}]]'
                         for offset in range(1, count + 1))
        body = 'Personal note fixture only.\n' if source == 0 else ''
        (own / f'nebula-{source:03d}.md').write_text(frontmatter + body + links, encoding='utf-8')
    for partition, body in [('main', 'Organization twin.'), ('main__u-nebula-foreign', 'Foreign twin.')]:
        (root / partition / 'reference/nebula-000.md').write_text(frontmatter + body, encoding='utf-8')
    oversized = frontmatter + ('星云 "\\" ' * 160_000)
    (own / 'nebula-oversize.md').write_text(oversized, encoding='utf-8')


def stop(child, timeout=15):
    if child.poll() is None:
        child.send_signal(signal.SIGTERM)
        try:
            return child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)
            return 'killed'
    return child.returncode


class Gateway:
    def __init__(self, server, root, env, port, log):
        self.server, self.root, self.env, self.port, self.log = server, root, env, port, log
        self.child = None
        self.endpoint = None

    def start(self, name=None):
        self.endpoint = self.root / name if name else None
        argv = [str(self.server), '--bind', '127.0.0.1', '--port', str(self.port), '--bundled-content-only']
        if self.endpoint is not None:
            argv += ['--desktop-ipc', str(self.endpoint)]
        argv.append('start')
        self.child = subprocess.Popen(
            argv, cwd=self.root, env=self.env, stdout=self.log, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline and self.child.poll() is None:
            if (self.endpoint is None or self.endpoint.exists()) and self.https_ready():
                return
            time.sleep(0.2)
        raise RuntimeError('isolated Gateway did not mount required listeners')

    def https_ready(self):
        try:
            with socket.create_connection(('127.0.0.1', self.port), timeout=0.5):
                return True
        except OSError:
            return False

    def stop(self):
        return stop(self.child)


class Desktop:
    """Launches the shell binary and inspects its windows and storage."""

    def __init__(self, shell, winlist, root, env, out):
        self.shell, self.winlist, self.root, self.env, self.out = shell, winlist, root, env, out
        self.storage = Path(env['HOME']) / 'Library/Application Support' / PANEL_APP_ID / 'storage.json'

    def main_window(self, pid):
        result = subprocess.run([str(self.winlist), str(pid)], capture_output=True, text=True, timeout=10)
        windows = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        return next((w for w in windows if w['layer'] == 0 and w['name'] == 'Aleph'), None)

    def bring_to_front(self, pid):
        # Stage Manager shows a background window as a small thumbnail; the
        # capture needs the real window.
        subprocess.run(
            ['osascript', '-e',
             f'tell application "System Events" to set frontmost of (first process whose unix id is {pid}) to true'],
            check=True, capture_output=True, timeout=20,
        )

    def threads(self, pid):
        result = subprocess.run(['ps', '-M', '-p', str(pid)], capture_output=True, text=True, timeout=10)
        return max(len(result.stdout.splitlines()) - 1, 0)

    def run(self, name, extra_env, settle, action=None):
        """One launch: wait for the main window, bring it to the front, let it
        settle, run `action(pid)` if given (S3 types into it), capture it."""
        log = (self.out / f'desktop-{name}.log').open('w', encoding='utf-8')
        child = subprocess.Popen(
            [str(self.shell)], cwd=self.root, env={**self.env, **extra_env},
            stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
        )
        try:
            deadline = time.monotonic() + 60
            window = None
            while time.monotonic() < deadline and child.poll() is None and window is None:
                window = self.main_window(child.pid)
                time.sleep(0.3)
            require(window is not None, f'{name}: the shell opened no main window')
            self.bring_to_front(child.pid)
            time.sleep(settle)
            if action is not None:
                action(child.pid)
            window = self.main_window(child.pid)
            require(window and window['onscreen'] and window['width'] >= 400,
                    f'{name}: the main window is not in front: {window}')
            require(child.poll() is None, f'{name}: the shell exited early')
            image = self.out / f'desktop-{name}.png'
            subprocess.run(['screencapture', '-x', '-o', f'-l{window["id"]}', str(image)], check=True, timeout=20)
            return {
                'phase': name,
                'window': {k: window[k] for k in ('width', 'height', 'name')},
                'threads': self.threads(child.pid),
                'screenshot': image.name,
            }
        finally:
            result = stop(child)
            log.close()
            print(f'desktop {name}: shell exit {result}', flush=True)

    def layout(self):
        entries = json.loads(self.storage.read_text(encoding='utf-8'))
        keys = [key for key in entries if key.startswith(LAYOUT_PREFIX)]
        require(len(keys) == 1, f'expected one desktop layout entry, found {keys}')
        return keys[0], json.loads(entries[keys[0]])

    def write_layout(self, key, layout):
        """Replace the stored layout while the shell is not running."""
        entries = json.loads(self.storage.read_text(encoding='utf-8'))
        entries[key] = json.dumps(layout, separators=(',', ':'))
        self.storage.write_text(json.dumps(entries), encoding='utf-8')


def tabs(*tab_ids):
    return [{'plugin_id': 'aleph', 'tab_id': tab_id} for tab_id in tab_ids]


def pairing_ticket(server, root, env):
    result = subprocess.run(
        [str(server), 'pair', '--user', 'u-owner'], cwd=root, env=env,
        capture_output=True, text=True, timeout=60,
    )
    match = re.search(r'aleph-bt-[0-9a-f-]{36}', result.stdout)
    require(result.returncode == 0 and match, 'could not mint a pairing ticket')
    return match.group(0)


def receipt(args):
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {
        'platform': sys.platform,
        'server_sha256': sha256(args.server),
        'shell_sha256': sha256(args.shell) if args.shell else None,
        'nebula_fixture': args.nebula,
        'web_index_sha256': sha256(args.web / 'index.html'),
    }
    with tempfile.TemporaryDirectory(prefix='aleph-s2-', dir=None if args.web_only else '/tmp') as directory:
        root = Path(directory)
        home, aleph = root / 'home', root / 'aleph'
        home.mkdir(mode=0o700)
        aleph.mkdir(mode=0o700)
        port = fixture_port()
        write_config(aleph, port, args.web, memory_enabled=args.nebula in ('normal', 'dense'))
        if args.nebula in ('normal', 'dense'):
            seed_nebula(aleph, args.nebula == 'dense')
        env = {
            'PATH': os.environ['PATH'],
            'HOME': str(home),
            'ALEPH_HOME': str(aleph),
            'RUST_LOG': 'info',
            'NO_PROXY': '127.0.0.1,localhost',
        }
        if os.name == 'nt':
            for name in ('SystemRoot', 'WINDIR', 'TEMP', 'TMP', 'COMSPEC'):
                if name in os.environ:
                    env[name] = os.environ[name]
            for name in ('USERPROFILE', 'LOCALAPPDATA', 'APPDATA'):
                env[name] = str(home)
        shell_env = {**env, 'ALEPH_SHELL_PANEL': 'tsx', 'ALEPH_SHELL_HOTKEY': 'disabled'}
        desktop = None if args.web_only else Desktop(args.shell, args.winlist, root, shell_env, out)
        with (out / 'gateway.log').open('w', encoding='utf-8') as log:
            gateway = Gateway(args.server, root, env, port, log)
            phases = []
            try:
                if desktop is None:
                    gateway.start()
                    report['desktop'] = 'not run (web-only)'
                else:
                    key = desktop_phases(desktop, gateway, phases, report, args.nebula)

                # Web: the same TSX source from /v1/panel/ in Chrome.
                fixture = {
                    'origin': f'https://127.0.0.1:{port}',
                    'certificate': (aleph / 'data/tls/cert.pem').read_text(encoding='utf-8'),
                    'ticket': pairing_ticket(args.server, root, env),
                    'out': str(out),
                    'nebula': args.nebula,
                }
                fixture_path = root / 'web-fixture.json'
                fixture_path.write_text(json.dumps(fixture), encoding='utf-8')
                web = subprocess.run(
                    ['node', str(HERE / 'web-panel.cjs'), str(fixture_path)],
                    env={**os.environ, 'NODE_PATH': str(args.node_modules)},
                    capture_output=True, text=True, timeout=300,
                )
                (out / 'web.log').write_text(web.stdout + web.stderr, encoding='utf-8')
                require(web.returncode == 0, 'web driver failed; see web.log')
                web_report = json.loads((out / 'web-report.json').read_text(encoding='utf-8'))
                web_key = next(step['key'] for step in web_report['steps'] if step['name'] == 'layout_edited')
                if desktop is not None:
                    report['subject_matches_desktop'] = web_key.rsplit(':', 1)[1] == key.rsplit(':', 1)[1]
                print('PASS web: pairing, modules, layout restore, cookie loss', flush=True)
            finally:
                if gateway.child is not None:
                    report['gateway_exit'] = gateway.stop()
    (out / 'report.json').write_text(f'{json.dumps(report, indent=2)}\n', encoding='utf-8')
    print(json.dumps(report), flush=True)


def desktop_phases(desktop, gateway, phases, report, nebula):
    # Desktop without a Desktop IPC target: an honest unavailable state.
    phases.append(desktop.run('unavailable', {}, settle=4))

    gateway.start('gw-1.sock')
    phases.append(desktop.run('connected', {'ALEPH_SHELL_PANEL_IPC': str(gateway.endpoint)}, settle=8))
    key, first = desktop.layout()
    shell_tabs = tabs('overview', 'sessions', 'chat', 'capabilities')
    require([tab for tab in first['order'] if tab in shell_tabs] == shell_tabs,
            'default desktop layout is not the shell module order')
    if nebula:
        require(tabs('memory-nebula')[0] in first['order'], 'Nebula capability was not published')
        desktop.write_layout(key, {**first, 'active': tabs('memory-nebula')[0]})
        phases.append(desktop.run('memory-nebula', {'ALEPH_SHELL_PANEL_IPC': str(gateway.endpoint)}, settle=8))

    # Open the Sessions module through the stored layout (the edit UI
    # itself is driven in Chrome and by `nsh test`).
    sessions = {**first, 'active': tabs('sessions')[0]}
    desktop.write_layout(key, sessions)
    phases.append(desktop.run('sessions', {'ALEPH_SHELL_PANEL_IPC': str(gateway.endpoint)}, settle=8))
    require(desktop.layout() == (key, sessions), 'the shell rewrote the stored layout')

    # Reorder and hide while the shell is down, restart the Gateway on a
    # fresh socket path (as every Desktop IPC start does), and relaunch.
    edited = {
        'version': 1,
        'order': tabs('capabilities', 'overview', 'sessions', 'chat')
        + [tab for tab in first['order'] if tab not in shell_tabs],
        'hidden': tabs('sessions'),
        'active': tabs('capabilities')[0],
    }
    desktop.write_layout(key, edited)
    require(gateway.stop() == 0, 'Gateway did not stop cleanly between desktop runs')
    gateway.start('gw-2.sock')
    phases.append(desktop.run('restored', {'ALEPH_SHELL_PANEL_IPC': str(gateway.endpoint)}, settle=8))
    require(desktop.layout() == (key, edited),
            'the layout key or the stored layout changed across a Gateway restart')
    report['desktop'] = {
        'phases': phases,
        'layout_key': key,
        'default_layout': first,
        'restored_layout': edited,
    }
    print('PASS desktop: unavailable, connected, restored after relaunch and Gateway restart', flush=True)
    return key


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--shell', type=Path, help='aleph-nsh-shell built with --features tsx-panel')
    parser.add_argument('--web-only', action='store_true', help='skip macOS desktop; run real Chromium Web v1')
    parser.add_argument('--nebula', choices=('normal', 'dense', 'off'), help='exercise Memory Nebula with isolated files')
    parser.add_argument('--web', type=Path, required=True, help='interfaces/panel/out-web/Aleph Panel')
    parser.add_argument('--winlist', type=Path, help='compiled winlist.swift')
    parser.add_argument('--node-modules', type=Path, required=True, help='node_modules holding playwright')
    parser.add_argument('--out', type=Path, required=True, help='new directory for evidence')
    args = parser.parse_args()
    if not args.web_only and (sys.platform != 'darwin' or not (args.shell and args.winlist)):
        parser.error('desktop QA requires macOS, --shell and --winlist; use --web-only otherwise')
    for name in ('server', 'shell', 'web', 'winlist', 'node_modules'):
        if path := getattr(args, name):
            setattr(args, name, path.resolve(strict=True))
    receipt(args)


if __name__ == '__main__':
    main()
