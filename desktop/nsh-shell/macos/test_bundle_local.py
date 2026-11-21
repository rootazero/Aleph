"""macOS regression for the local signed bundle publisher."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class BundlePublicationTest(unittest.TestCase):
    @unittest.skipUnless(os.uname().sysname == 'Darwin', 'requires macOS signing tools')
    def test_concurrent_publication_does_not_nest_or_replace(self):
        publisher = Path(__file__).with_name('bundle-local.sh').resolve()
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
                subprocess.run(
                    ['/usr/bin/clang', '-x', 'c', '-o', binary, '-'],
                    input=f'int main(void) {{ return {index}; }}\n', text=True, check=True,
                )
                env = os.environ | {'PATH': f'{root}:{os.environ["PATH"]}',
                                    'BARRIER': work, 'SLOT': slot, 'DESTINATION': str(destination)}
                jobs.append((["/bin/bash", publisher, binary, destination], env))
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
            subprocess.run(['/usr/bin/codesign', '--verify', '--strict', destination], check=True)


if __name__ == '__main__':
    unittest.main()
