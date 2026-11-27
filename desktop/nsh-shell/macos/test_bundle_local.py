"""macOS regression for the local signed bundle publisher."""

import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import unittest


@unittest.skipUnless(sys.platform == 'darwin', 'requires macOS signing tools')
class BundlePublicationTest(unittest.TestCase):
    publisher = Path(__file__).with_name('bundle-local.sh').resolve()

    @staticmethod
    def compile_fixture(path, status):
        subprocess.run(
            ['/usr/bin/clang', '-x', 'c', '-o', path, '-'],
            input=f'int main(void) {{ return {status}; }}\n', text=True, check=True,
        )

    def test_full_bundle_publishes_executable_signed_server_and_bridge_siblings(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            host = root / 'host'
            server = root / 'server'
            bridge = root / 'private bridge fixture'
            for source, status in ((host, 0), (server, 17), (bridge, 23)):
                self.compile_fixture(source, status)
            original_bytes = {source: source.read_bytes() for source in (host, server, bridge)}

            destination = root / 'Full.app'
            bundled = subprocess.run(
                ['/bin/bash', self.publisher, '--server', server, '--bridge', bridge, host, destination],
                capture_output=True, text=True,
            )
            self.assertEqual(bundled.returncode, 0, bundled.stderr)
            self.assertEqual(bundled.stdout.splitlines()[-1], str(destination))
            for name, status in (('aleph-server', 17), ('AlephBridge', 23)):
                with self.subTest(sibling=name):
                    sibling = destination / 'Contents/MacOS' / name
                    subprocess.run(['/usr/bin/codesign', '--verify', '--strict', sibling], check=True)
                    self.assertEqual(subprocess.run([sibling]).returncode, status)
            subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', destination], check=True)
            for source, data in original_bytes.items():
                self.assertEqual(source.read_bytes(), data, f'signing changed {source}')
            with (destination / 'Contents/Info.plist').open('rb') as plist_file:
                plist = plistlib.load(plist_file)
            self.assertIn('microphone', plist['NSMicrophoneUsageDescription'])
            self.assertIn('on-device speech recognition', plist['NSSpeechRecognitionUsageDescription'])

    def test_legacy_server_only_bundle_does_not_stage_bridge(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            server = root / 'server'
            self.compile_fixture(server, 17)
            destination = root / 'Server.app'
            bundled = subprocess.run(
                ['/bin/bash', self.publisher, '--server', server, '/usr/bin/true', destination],
                capture_output=True, text=True,
            )
            self.assertEqual(bundled.returncode, 0, bundled.stderr)
            siblings = destination / 'Contents/MacOS'
            self.assertEqual(subprocess.run([siblings / 'aleph-server']).returncode, 17)
            self.assertFalse((siblings / 'AlephBridge').exists())
            subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', destination], check=True)

    def test_invalid_nested_inputs_do_not_publish_bundle(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            destination = root / 'Full.app'
            absent = root / 'absent'
            nonexec = root / 'nonexec'
            nonexec.write_text('not executable')
            nonexec.chmod(0o644)
            executable = '/usr/bin/true'
            for name, args, error in (
                ('bridge only', ['--bridge', executable, executable], 'Usage:'),
                ('reversed flags', ['--bridge', executable, '--server', executable, executable], 'Usage:'),
                ('missing flag argument', ['--server', executable, '--bridge', executable], 'Usage:'),
                ('missing server', ['--server', absent, executable], 'executable server'),
                ('nonexecutable server', ['--server', nonexec, '--bridge', executable, executable], 'executable server'),
                ('missing bridge', ['--server', executable, '--bridge', absent, executable], 'executable AlephBridge'),
                ('nonexecutable bridge', ['--server', executable, '--bridge', nonexec, executable], 'executable AlephBridge'),
                ('empty bridge path', ['--server', executable, '--bridge', '', executable], 'executable AlephBridge'),
            ):
                with self.subTest(name=name):
                    result = subprocess.run(
                        ['/bin/bash', self.publisher, *args, destination], capture_output=True, text=True,
                    )
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertIn(error, result.stderr)
                    self.assertFalse(destination.exists())
                    self.assertEqual(list(root.glob('.aleph-nsh-bundle.*')), [])

    def test_concurrent_publication_does_not_nest_or_replace(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            shim = root / 'codesign'
            shim.write_text('''#!/bin/bash
set -e
/usr/bin/codesign "$@"
if [[ $1 == --verify ]]; then
    touch "$BARRIER/$SLOT"
    for ((n=0; n<200; n++)); do
        if [[ -e $BARRIER/first && -e $BARRIER/second && ( $SLOT == first || -d $DESTINATION ) ]]; then exit 0; fi
        sleep 0.05
    done
    exit 3
fi
''')
            shim.chmod(0o755)
            destination = root / 'Aleph.app'
            jobs = []
            for index, slot in enumerate(('first', 'second')):
                binary = root / f'candidate-{slot}'
                self.compile_fixture(binary, index)
                env = os.environ | {'PATH': f'{root}:{os.environ["PATH"]}',
                                    'BARRIER': work, 'SLOT': slot, 'DESTINATION': str(destination)}
                jobs.append((["/bin/bash", self.publisher, binary, destination], env))
            with subprocess.Popen(jobs[0][0], env=jobs[0][1], stdout=subprocess.PIPE, stderr=subprocess.PIPE) as first, \
                 subprocess.Popen(jobs[1][0], env=jobs[1][1], stdout=subprocess.PIPE, stderr=subprocess.PIPE) as second:
                results = []
                try:
                    for process in (first, second):
                        stdout, stderr = process.communicate(timeout=20)
                        results.append((process.returncode, stdout, stderr))
                finally:
                    for process in (first, second):
                        if process.poll() is None:
                            process.kill()
                            process.wait()
            self.assertEqual(sorted(result[0] for result in results), [0, 2], results)
            winner = next(index for index, result in enumerate(results) if result[0] == 0)
            executable = destination / 'Contents/MacOS/aleph-nsh-shell'
            self.assertEqual(subprocess.run([executable]).returncode, winner)
            self.assertFalse((destination / 'Aleph.app').exists())
            self.assertFalse((executable.parent / 'aleph-server').exists())
            self.assertFalse((executable.parent / 'AlephBridge').exists())
            subprocess.run(['/usr/bin/codesign', '--verify', '--strict', destination], check=True)


if __name__ == '__main__':
    unittest.main()
