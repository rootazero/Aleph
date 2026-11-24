#!/usr/bin/env python3
"""Two-column shell and personal preferences receipt: one isolated Gateway,
the TSX Panel in the NSH shell over Desktop IPC (macOS) and in Chrome over
Web v1.

Desktop is driven with System Events keystrokes and clicks; its outcome is
read back from the shell's storage file and from window screenshots (NSH
views expose no accessibility tree this script can query). Chrome is driven
by `web-prefs.cjs`.

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

PREFS_KEY = 'aleph.panel.prefs.v1'

# Settings buttons, in points from the window's top left corner (title bar
# included), as laid out at the default text size, density and corners. Only
# changes that move nothing (mode, accent, corners) are clicked before text
# size, which moves everything below it.
SETTINGS_CLICKS = [
    ('Mode Dark', 409, 115),
    ('Accent Ocean', 346, 167),
    ('Corners Round', 485, 271),
    ('Text size Large', 495, 219),
]

# The palette lists the shell's commands, then one "Open X" per menu entry:
# palette, settings, scheme, reconnect, nine slots, Overview, Sessions, Chat,
# Capabilities.
OPEN_CAPABILITIES_ROW = 16

KEY_ESCAPE, KEY_RETURN, KEY_DOWN, KEY_COMMA = 53, 36, 125, 43


def system_events(pid, body):
    script = (
        f'tell application "System Events" to tell (first process whose unix id is {pid})\n'
        f'{body}\n'
        'end tell'
    )
    result = subprocess.run(['osascript', '-e', script], check=True, capture_output=True, text=True, timeout=30)
    return result.stdout.strip()


def command(pid, key):
    """mod+<key>; a number is a key code (`,` must be sent as one)."""
    if isinstance(key, int):
        system_events(pid, f'  key code {key} using command down')
    else:
        system_events(pid, f'  keystroke {json.dumps(key)} using command down')
    time.sleep(1.5)


def key_code(pid, code, times=1):
    for _ in range(times):
        system_events(pid, f'  key code {code}')
        time.sleep(0.15)
    time.sleep(1)


def capture(desktop, pid, name):
    window = desktop.main_window(pid)
    subprocess.run(['screencapture', '-x', '-o', f'-l{window["id"]}', str(desktop.out / f'desktop-{name}.png')],
                   check=True, timeout=20)
    return f'desktop-{name}.png'


def stored_prefs(desktop):
    entries = json.loads(desktop.storage.read_text(encoding='utf-8'))
    raw = entries.get(PREFS_KEY)
    return None if raw is None else json.loads(raw)


def write_prefs(desktop, prefs):
    """Replace the stored preferences while the shell is not running."""
    entries = json.loads(desktop.storage.read_text(encoding='utf-8'))
    entries[PREFS_KEY] = json.dumps(prefs, separators=(',', ':'))
    desktop.storage.write_text(json.dumps(entries), encoding='utf-8')


def edit_from_ui(desktop, clicker, shots):
    """Palette by keyboard, then Settings by mod+, and clicks on its buttons."""

    def action(pid):
        shots.append(capture(desktop, pid, 'edit-start'))
        command(pid, 'k')
        shots.append(capture(desktop, pid, 'edit-palette'))
        key_code(pid, KEY_DOWN, OPEN_CAPABILITIES_ROW)
        key_code(pid, KEY_RETURN)
        time.sleep(1)
        shots.append(capture(desktop, pid, 'edit-palette-ran'))
        command(pid, KEY_COMMA)
        shots.append(capture(desktop, pid, 'edit-settings'))
        x, y = (int(v) for v in system_events(pid, '  get position of window 1').split(','))
        for _label, dx, dy in SETTINGS_CLICKS:
            subprocess.run([str(clicker), str(x + dx), str(y + dy)], check=True, timeout=10)
            time.sleep(1)

    return action


def keys_after_restore(desktop, shots):
    """Stored overrides: palette on mod+j, a conflict, a reserved key."""

    def action(pid):
        shots.append(capture(desktop, pid, 'restored-start'))
        command(pid, 'k')
        shots.append(capture(desktop, pid, 'restored-mod-k'))
        command(pid, 'j')
        shots.append(capture(desktop, pid, 'restored-mod-j'))
        key_code(pid, KEY_ESCAPE)
        command(pid, KEY_COMMA)

    return action


def offline_settings(desktop, shots):
    def action(pid):
        shots.append(capture(desktop, pid, 'offline-start'))
        command(pid, KEY_COMMA)

    return action


def type_in_chat(desktop, shots):
    """Letters typed into the focused composer stay text; under an input
    method they compose first. mod+k still opens the palette from the field."""

    def action(pid):
        system_events(pid, '  keystroke "kkjj"')
        time.sleep(1)
        shots.append(capture(desktop, pid, 'chat-typed'))
        key_code(pid, KEY_RETURN)
        command(pid, 'k')

    return action


def input_source():
    result = subprocess.run(
        ['defaults', 'read', str(Path.home() / 'Library/Preferences/com.apple.HIToolbox.plist'),
         'AppleSelectedInputSources'],
        capture_output=True, text=True, timeout=10,
    )
    return result.stdout.strip()


def desktop_phase(desktop, clicker, gateway, report):
    ipc = {'ALEPH_SHELL_PANEL_IPC': str(gateway.endpoint)}
    shots = []
    phases = [desktop.run('edit', ipc, settle=6, action=edit_from_ui(desktop, clicker, shots))]
    edited = stored_prefs(desktop)
    s2.require(edited is not None, 'Desktop stored no preferences after the Settings clicks')
    expected = {'mode': 'dark', 'accent': 'ocean', 'roundness': 'round', 'fontScale': 'large'}
    got = {axis: edited['appearance'].get(axis) for axis in expected}
    s2.require(got == expected, f'Desktop Settings clicks stored {got}, expected {expected}')
    print('PASS desktop: palette ran Open Capabilities; Settings clicks stored mode, accent, corners, text size',
          flush=True)

    # Density and shortcut overrides go in while the shell is down: the next
    # launch must restore and apply all of it.
    write_prefs(desktop, {
        **edited,
        'appearance': {**edited['appearance'], 'density': 'cozy'},
        'keybindings': {
            'shell.palette': {'key': 'mod+j'},
            'shell.scheme': {'key': 'mod+2'},
            'shell.reconnect': {'key': 'mod+q'},
        },
    })
    phases.append(desktop.run('restored', ipc, settle=6, action=keys_after_restore(desktop, shots)))

    s2.require(gateway.stop() == 0, 'Gateway did not stop cleanly')
    phases.append(desktop.run('offline', ipc, settle=6, action=offline_settings(desktop, shots)))
    gateway.start('gw-2.sock')
    ipc = {'ALEPH_SHELL_PANEL_IPC': str(gateway.endpoint)}

    key, layout = desktop.layout()
    desktop.write_layout(key, {**layout, 'active': s2.tabs('chat')[0]})
    write_prefs(desktop, {**stored_prefs(desktop), 'keybindings': {}})
    phases.append(desktop.run('chat', ipc, settle=6, action=type_in_chat(desktop, shots)))
    report['desktop'] = {'phases': phases, 'shots': shots, 'edited_prefs': edited, 'final_prefs': stored_prefs(desktop)}
    print('PASS desktop: restore, rebinding, offline Settings and Chat typing captured (see screenshots)',
          flush=True)


def web_phase(args, gateway, root, env, port, aleph, out):
    fixture = {
        'origin': f'https://127.0.0.1:{port}',
        'certificate': (aleph / 'data/tls/cert.pem').read_text(encoding='utf-8'),
        'ticket': s2.pairing_ticket(args.server, root, env),
        'out': str(out),
    }
    fixture_path = root / 'web-fixture.json'
    fixture_path.write_text(json.dumps(fixture), encoding='utf-8')
    driver = subprocess.Popen(
        ['node', str(HERE / 'web-prefs.cjs'), str(fixture_path)],
        env={**os.environ, 'NODE_PATH': str(args.node_modules)},
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    lines = []
    for line in driver.stdout:
        lines.append(line)
        if line.strip() == 'STOP_GATEWAY':
            s2.require(gateway.stop() == 0, 'Gateway did not stop cleanly for the offline step')
            driver.stdin.write('stopped\n')
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
    with tempfile.TemporaryDirectory(prefix='aleph-prefs-', dir='/tmp') as directory:
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
                desktop_phase(desktop, args.click, gateway, report)
                report['web'] = web_phase(args, gateway, root, env, port, aleph, out)
                print('PASS web: appearance restore, palette, rebinding, conflict, invalid key, offline Settings',
                      flush=True)
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
    parser.add_argument('--click', type=Path, required=True, help='compiled qa/nsh_prefs/click.swift')
    parser.add_argument('--node-modules', type=Path, required=True, help='node_modules holding playwright')
    parser.add_argument('--out', type=Path, required=True, help='new directory for evidence')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        parser.error('the desktop surface is macOS-only')
    for name in ('server', 'shell', 'web', 'winlist', 'click', 'node_modules'):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    receipt(args)


if __name__ == '__main__':
    main()
