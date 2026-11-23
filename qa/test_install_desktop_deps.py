"""Exercise the standalone bootstrap with isolated loader and manager commands."""

import os
import pathlib
import pty
import subprocess
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / 'scripts/install-desktop-deps.sh'
BASH = '/bin/bash'


class InstallDesktopDepsTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = pathlib.Path(tmp.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.log = self.root / 'commands'
        self.os_release = self.root / 'os-release'
        self.os_release.write_text('ID=arch\n')
        self.before = self.root / 'before'
        self.after = self.root / 'after'
        self.before.write_text('')
        self.after.write_text(self.libs())
        self.command('uname', 'printf "%s\\n" "$ARCH"')
        self.command('ldconfig', '''
            [[ "$1" == -p ]] || exit 2
            printf 'probe\\n' >> "$LOG"
            [[ "$PROBE_FAIL" == 0 ]] || exit 9
            file=$BEFORE
            [[ ! -e "$LOG.installed" ]] || file=$AFTER
            while IFS= read -r row; do printf '%s\\n' "$row"; done < "$file"
        ''')
        self.command('sudo', 'printf "sudo\\n" >> "$LOG"; exec "$@"')
        for manager in ('pacman', 'apt-get', 'dnf'):
            self.command(manager, '''
                printf '%s %s\\n' "${0##*/}" "$*" >> "$LOG"
                [[ "$INSTALL_FAIL" == 0 ]] || exit 7
                : > "$LOG.installed"
            ''')
        self.env = dict(os.environ, PATH=str(self.bin), LC_ALL='C', LOG=str(self.log),
                        OS_RELEASE=str(self.os_release), BEFORE=str(self.before),
                        AFTER=str(self.after), ARCH='x86_64', PROBE_FAIL='0', INSTALL_FAIL='0')

    def libs(self, abi='x86-64', names=('libgtk-4.so.1', 'libwebkitgtk-6.0.so.4')):
        rows = []
        for name in names:
            path = self.root / name
            path.touch()
            rows.append(f'\t{name} (libc6,{abi}) => {path}\n')
        return ''.join(rows)

    def command(self, name, body):
        target = self.bin / name
        target.write_text(f'#!/bin/bash\n{body}\n')
        target.chmod(0o755)

    def run_script(self, *args, interactive=False, cli=False):
        master = slave = None
        try:
            if interactive:
                master, slave = pty.openpty()
            command = [BASH, str(SCRIPT), *args] if cli else [
                BASH, '-c', 'source "$1" || exit; shift; main "$OS_RELEASE" "$@"',
                'test', str(SCRIPT), *args,
            ]
            return subprocess.run(command, env=self.env, stdin=slave if interactive else subprocess.DEVNULL,
                                  capture_output=True, text=True, timeout=5)
        finally:
            if slave is not None:
                os.close(slave)
                os.close(master)

    def commands(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def test_cli_default_checks_only_even_when_missing(self):
        result = self.run_script(cli=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('libwebkitgtk-6.0.so.4', result.stderr)
        self.assertIn('--install', result.stderr)
        self.assertEqual(self.commands(), ['probe'])

    def test_already_installed_never_calls_privilege_or_package_manager(self):
        self.before.write_text(self.libs())
        result = self.run_script('--install', cli=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.commands(), ['probe'])

    def test_old_or_wrong_arch_libraries_do_not_satisfy_check(self):
        self.before.write_text(self.libs('AArch64') + self.libs(names=('libwebkit2gtk-4.1.so.0', 'libgtk-3.so.0')))
        result = self.run_script('--check')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('libgtk-4.so.1', result.stderr)
        self.assertIn('libwebkitgtk-6.0.so.4', result.stderr)
        self.assertEqual(self.commands(), ['probe'])

    def test_stale_cache_paths_do_not_satisfy_check(self):
        self.before.write_text(self.libs())
        (self.root / 'libwebkitgtk-6.0.so.4').unlink()
        result = self.run_script('--check')
        self.assertEqual(result.returncode, 1)
        self.assertIn('Missing runtime libraries: libwebkitgtk-6.0.so.4', result.stderr)

    def test_os_release_is_read_as_data_not_executed(self):
        sentinel = self.root / 'must-not-exist'
        self.os_release.write_text(f'ID="arch"\nUNTRUSTED=$(>"{sentinel}")\n')
        result = self.run_script('--install', interactive=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(sentinel.exists())

    def test_matching_aarch64_libraries_satisfy_check(self):
        self.env['ARCH'] = 'aarch64'
        self.before.write_text(self.libs('AArch64'))
        result = self.run_script('--check')
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_installs_through_exact_distro_manager_then_rechecks(self):
        expected = {
            'arch': 'pacman -S --needed gtk4 webkitgtk-6.0',
            'debian': 'apt-get install libgtk-4-1 libwebkitgtk-6.0-4',
            'ubuntu': 'apt-get install libgtk-4-1 libwebkitgtk-6.0-4',
            'fedora': 'dnf install gtk4 webkitgtk6.0',
        }
        for distro, invocation in expected.items():
            with self.subTest(distro=distro):
                self.os_release.write_text(f'NAME="Test"\nID="{distro}"\n')
                self.log.unlink(missing_ok=True)
                self.log.with_suffix('.installed').unlink(missing_ok=True)
                result = self.run_script('--install', interactive=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.commands(), ['probe', *(['sudo'] if os.geteuid() else []), invocation, 'probe'])

    def test_refuses_noninteractive_install(self):
        result = self.run_script('--install')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('interactive terminal', result.stderr)
        self.assertEqual(self.commands(), ['probe'])

    def test_unknown_distro_is_not_inferred_from_id_like(self):
        self.os_release.write_text('ID=other\nID_LIKE=arch\n')
        result = self.run_script('--install', interactive=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Unsupported distribution', result.stderr)
        self.assertEqual(self.commands(), ['probe'])

    def test_broken_probe_never_attempts_install(self):
        self.env['PROBE_FAIL'] = '1'
        result = self.run_script('--install', interactive=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('ldconfig', result.stderr)
        self.assertEqual(self.commands(), ['probe'])

    def test_missing_manager_never_invokes_sudo(self):
        (self.bin / 'pacman').unlink()
        result = self.run_script('--install', interactive=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('not found: pacman', result.stderr)
        self.assertEqual(self.commands(), ['probe'])

    @unittest.skipIf(os.geteuid() == 0, 'root does not require sudo')
    def test_missing_privilege_does_not_invoke_manager(self):
        (self.bin / 'sudo').unlink()
        result = self.run_script('--install', interactive=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('sudo', result.stderr)
        self.assertEqual(self.commands(), ['probe'])

    def test_install_failure_surfaces_manager_exit(self):
        self.env['INSTALL_FAIL'] = '7'
        result = self.run_script('--install', interactive=True)
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertIn('exit 7', result.stderr)
        self.assertEqual(self.commands().count('probe'), 1)

    def test_manager_success_without_libraries_is_failure(self):
        self.after.write_text('')
        result = self.run_script('--install', interactive=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('still missing', result.stderr)
        self.assertEqual(self.commands().count('probe'), 2)

    def test_help_and_bad_arguments_never_probe(self):
        self.assertEqual(self.run_script('--help', cli=True).returncode, 0)
        self.assertNotEqual(self.run_script('--check', '--install', cli=True).returncode, 0)
        self.assertEqual(self.commands(), [])
