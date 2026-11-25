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
import re
import sqlite3
import subprocess
import sys
import tempfile
import time

import fixture_provider

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location('s2_shell', HERE.parent / 'nsh_s2_shell' / 'run.py')
s2 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(s2)

# No spaces: the Mac this runs on may have a Chinese input method selected,
# where space picks a candidate instead of typing one.
DESKTOP_MESSAGE = 'S3desktophello'
OLDEST_MARKER = 'Scrollback fixture start 000'



def keys(pid, body):
    script = (
        f'tell application "System Events" to tell (first process whose unix id is {pid})\n'
        f'{body}\n'
        'end tell'
    )
    subprocess.run(['osascript', '-e', script], check=True, capture_output=True, timeout=20)


def native_text(desktop, pid, shot, expected, timeout=15):
    """Wait for text actually rendered in the owned native window."""
    path = desktop.out / f'{shot}.png'
    deadline = time.monotonic() + timeout
    while True:
        window = desktop.main_window(pid)
        subprocess.run(['screencapture', '-x', '-o', f'-l{window["id"]}', str(path)],
                       check=True, timeout=20)
        scan = subprocess.check_output([str(desktop.native_view), 'read', str(path)], timeout=15)
        rows = json.loads(scan)
        match = next((row for row in rows if expected.search(row['text'])), None)
        if match:
            return match
        if time.monotonic() >= deadline:
            if timeout == 0:
                return None
            raise RuntimeError(f'{shot}: {expected.pattern} not found in {[row["text"] for row in rows]}')
        time.sleep(0.25)


def native_input(desktop, pid, kind, row):
    subprocess.run([str(desktop.native_view), kind, str(pid), str(row['x']), str(row['y'])],
                   check=True, timeout=15)


def seed_history(aleph):
    """Populate the isolated committed projection, with its Gateway stopped."""
    db = aleph / 'data/sessions.db'
    with sqlite3.connect(f'{db.as_uri()}?mode=rw', uri=True) as connection:
        matches = connection.execute(
            "SELECT DISTINCT session_key FROM messages WHERE role = 'user' AND content LIKE '%desktophello'",
        ).fetchall()
        s2.require(len(matches) == 1, f'expected one committed desktop session: {matches}')
        key = matches[0][0]
        floor = connection.execute('SELECT MIN(id) FROM messages').fetchone()[0]
        # Insert a legacy prefix before the existing projected rows rather than
        # editing their immutable identity or reseeding the desktop send.
        connection.executemany(
            "INSERT INTO messages (id, session_key, role, content, timestamp) VALUES (?, ?, 'assistant', ?, 1)",
            ((floor - 205 + index, key, OLDEST_MARKER if index == 0 else f'Fixture earlier answer {index:03d}')
             for index in range(205)),
        )
        return key


def native_scrollback(desktop, key, evidence):
    def action(pid):
        session = native_text(desktop, pid, 'desktop-session-row', re.compile(r'\d+ messages'))
        native_input(desktop, pid, 'click', session)
        marker = re.compile('desktophello$', re.IGNORECASE)
        oldest = re.compile(re.escape(OLDEST_MARKER), re.IGNORECASE)
        tail = native_text(desktop, pid, 'desktop-tail', marker)
        native_input(desktop, pid, 'up', tail)
        native_text(desktop, pid, 'desktop-away-from-tail', re.compile('Jump to bottom'))
        before = native_text(desktop, pid, 'desktop-before-prepend', re.compile(r'^Fixture earlier answer \d{3}$'))
        s2.require(native_text(desktop, pid, 'desktop-oldest-absent', oldest, timeout=0) is None,
                   'native oldest marker was already visible')
        button = native_text(desktop, pid, 'desktop-page-control', re.compile('Load older messages'))
        native_input(desktop, pid, 'click', button)
        native_text(desktop, pid, 'desktop-page-end', re.compile('No earlier messages'))
        after = native_text(desktop, pid, 'desktop-after-prepend', re.compile(f'^{re.escape(before["text"])}$'))
        height = desktop.main_window(pid)['height']
        delta = abs(before['y'] - after['y']) * height
        s2.require(delta <= 3, f'native prepend moved anchor by {delta:.2f} points')
        for _ in range(12):
            native_input(desktop, pid, 'up', after)
            time.sleep(0.3)
            if native_text(desktop, pid, 'desktop-oldest-marker', oldest, timeout=0):
                break
        else:
            raise RuntimeError('oldest fixture marker did not appear in the native list')
        evidence['scrollback'] = {'session_key': key, 'anchor': before['text'], 'before_y': before['y'] * height,
                                  'after_y': after['y'] * height, 'oldest_marker': OLDEST_MARKER}
    return action


def type_and_send(desktop, text, live_capture=None, continuity=None):
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
        if live_capture:
            ready, release = live_capture
            try:
                deadline = time.monotonic() + 20
                while not ready.exists() and time.monotonic() < deadline:
                    time.sleep(0.1)
                if not ready.exists():
                    window = desktop.main_window(pid)
                    subprocess.run(['screencapture', '-x', '-o', f'-l{window["id"]}',
                                    str(desktop.out / 'desktop-unobserved-reply.png')], check=True, timeout=15)
                    s2.require(False, 'provider did not reach the held desktop reply')
                time.sleep(0.6)
                window = desktop.main_window(pid)
                subprocess.run(['screencapture', '-x', '-o', f'-l{window["id"]}',
                                str(desktop.out / 'desktop-live-tools.png')], check=True, timeout=15)
            finally:
                release.touch()
        # Let the transcript refetch committed history after the run closes.
        time.sleep(4)
        if continuity is not None:
            marker = re.compile('desktophello$', re.IGNORECASE)
            native_text(desktop, pid, 'desktop-before-settings', marker)
            keys(pid, '  keystroke "," using command down')
            native_text(desktop, pid, 'desktop-settings-open', re.compile(r'^Appearance$'))
            keys(pid, '  keystroke "," using command down')
            restored = native_text(desktop, pid, 'desktop-after-settings', marker)
            continuity['settings_restore'] = {'pid': pid, 'restored_marker': restored['text']}

    return action


def input_source():
    result = subprocess.run(
        ['defaults', 'read', str(Path.home() / 'Library/Preferences/com.apple.HIToolbox.plist'),
         'AppleSelectedInputSources'],
        capture_output=True, text=True, timeout=10,
    )
    return result.stdout.strip()


def web_phase(args, gateway, root, env, port, aleph, out, web_release):
    """Runs the Chrome driver; restarts the Gateway when the driver asks."""
    fixture = {
        'origin': f'https://127.0.0.1:{port}',
        'certificate': (aleph / 'data/tls/cert.pem').read_text(encoding='utf-8'),
        'ticket': s2.pairing_ticket(args.server, root, env),
        'desktop_marker': 'desktophello',
        'rich': args.rich,
        'continuity': args.continuity,
        'oldest_marker': OLDEST_MARKER,
        'reply': fixture_provider.REPLY if args.rich else 'This response is from',
        'file': fixture_provider.FILE if args.rich else None,
        'live_tool': {
            'file': fixture_provider.WEB_FILE,
            'before': fixture_provider.WEB_BEFORE,
            'after': fixture_provider.WEB_AFTER,
            'release': str(web_release),
        } if args.rich else None,
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
        desktop_ready = root / 'desktop-reply-ready'
        desktop_release = root / 'release-desktop-reply'
        web_release = root / 'release-web-reply'
        provider = fixture_provider.install(
            aleph, desktop_ready, desktop_release, web_release, out / 'provider.log',
        ) if args.rich else None
        env = {
            'PATH': os.environ['PATH'],
            'HOME': str(home),
            'ALEPH_HOME': str(aleph),
            'RUST_LOG': 'info',
            'NO_PROXY': '127.0.0.1,localhost',
        }
        shell_env = {**env, 'ALEPH_SHELL_PANEL': 'tsx', 'ALEPH_SHELL_HOTKEY': 'disabled'}
        desktop = s2.Desktop(args.shell, args.winlist, root, shell_env, out)
        if args.continuity:
            desktop.native_view = root / 'native-view'
            subprocess.run(['swiftc', str(HERE / 'native-view.swift'), '-o', str(desktop.native_view)],
                           check=True, timeout=90)
        with (out / 'gateway.log').open('w', encoding='utf-8') as log:
            gateway = s2.Gateway(args.server, root, env, port, log)
            try:
                gateway.start('gw-1.sock')
                ipc = {'ALEPH_SHELL_PANEL_IPC': str(gateway.endpoint)}
                phases = [desktop.run('connected', ipc, settle=6)]
                key, first = desktop.layout()
                expected = s2.tabs('overview', 'sessions', 'chat', 'capabilities')
                s2.require([tab for tab in first['order'] if tab in expected] == expected,
                           f'default layout lacks the Chat module: {first}')

                # Open Chat through the stored layout, type a message, Return.
                desktop.write_layout(key, {**first, 'active': s2.tabs('chat')[0]})
                capture = (desktop_ready, desktop_release) if args.rich else None
                continuity = {} if args.continuity else None
                action = type_and_send(desktop, DESKTOP_MESSAGE, capture, continuity)
                phases.append(desktop.run('chat-sent', ipc, settle=6, action=action))
                report['desktop'] = {'phases': phases, 'layout_key': key}
                print('PASS desktop: Chat opened, message typed and sent over Desktop IPC', flush=True)
                if args.rich:
                    files = list(root.rglob(fixture_provider.FILE))
                    s2.require(len(files) == 1 and files[0].read_text(encoding='utf-8') == 's3 edited line\n',
                               f'real Write/Edit did not produce the expected fixture file: {files}')
                    report['desktop']['updated_fixture'] = str(files[0].relative_to(root))

                if args.continuity:
                    s2.require(gateway.stop() == 0, 'Gateway did not stop cleanly for the fixture seed')
                    session_key = seed_history(aleph)
                    gateway.start('gw-1.sock')
                    ipc = {'ALEPH_SHELL_PANEL_IPC': str(gateway.endpoint)}
                    desktop.write_layout(key, {**first, 'active': s2.tabs('sessions')[0]})
                    phases.append(desktop.run('scrollback', ipc, settle=6,
                                              action=native_scrollback(desktop, session_key, continuity)))
                    report['desktop']['continuity'] = continuity

                web = web_phase(args, gateway, root, env, port, aleph, out, web_release)
                report['web'] = web
                if args.rich:
                    files = list(root.rglob(fixture_provider.WEB_FILE))
                    s2.require(len(files) == 1 and files[0].read_text(encoding='utf-8') == 's3 web line\n',
                               f'real Web Write did not produce the expected fixture file: {files}')
                    report['web']['updated_fixture'] = str(files[0].relative_to(root))
                print('PASS web: cross-surface history, IME hold, live stream, Gateway restart', flush=True)
            finally:
                if gateway.child is not None:
                    report['gateway_exit'] = gateway.stop()
                if provider is not None:
                    provider.shutdown()
                    provider.server_close()
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
    parser.add_argument('--rich', action='store_true', help='exercise actual Write/Edit via a loopback scripted provider')
    parser.add_argument('--continuity', action='store_true', help='verify Chat survives reconnect and preserves the selected session without a send')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        parser.error('the desktop surface is macOS-only')
    for name in ('server', 'shell', 'web', 'winlist', 'node_modules'):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    receipt(args)


if __name__ == '__main__':
    main()
