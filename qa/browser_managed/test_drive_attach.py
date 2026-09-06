#!/usr/bin/env python3
"""Unit test for `chrome_main_pid`'s "pid already gone" handling.

No real machine and no subprocess race needed — a stubbed `ps` result is
enough to reproduce the bug: a pid that exits between `pgrep` (chrome_pids)
and this module's own `ps -p` call gives `ps` an EMPTY command line, and
empty does not contain "--type=" either, so the old code read that as
"found the main process" instead of "this pid is gone, learned nothing"
(drive_attach.py's `chrome_main_pid` docstring has the full account).

Plain `unittest`, not `pytest`: nothing else under `qa/` uses pytest, and
these scripts run against the machine's own `python3` with no install step —
adding a new dependency here would break that for every future run of this
suite.
"""
import os
import subprocess
import sys
import unittest
from unittest.mock import patch

# drive_attach.py is a script, not a library: it parses argv and reads
# $ALEPH_HOME at import time. Stand both up with harmless dummy values
# purely so the import succeeds — nothing below touches a real process, a
# real gateway, or a real HOME.
sys.argv = [
    "drive_attach.py",
    "ws://127.0.0.1:0",
    "--page-url", "http://example.invalid",
    "--marker", "unit-test",
    "--home", "/tmp/drive-attach-unit-test-home",
    "--cli", "/bin/true",
    "--expect-user-data-dir", "/tmp/drive-attach-unit-test-udd",
    "--server-pid", "1",
]
os.environ.setdefault("ALEPH_HOME", "/tmp/drive-attach-unit-test-home")
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import drive_attach  # noqa: E402


class ChromeMainPidTests(unittest.TestCase):
    def test_a_pid_that_exited_between_pgrep_and_ps_is_skipped_not_matched(self):
        with patch.object(drive_attach, "chrome_pids", return_value=["4242"]), \
                patch.object(drive_attach.subprocess, "run") as mock_run:
            mock_run.return_value = subprocess.CompletedProcess(
                args=["ps"], returncode=1, stdout="", stderr="",
            )
            pid, argv_text = drive_attach.chrome_main_pid("/tmp/drive-attach-unit-test-udd")
        self.assertIsNone(
            pid,
            "an empty ps result (the pid exited between pgrep and ps) must "
            "be read as unknown, not as a match",
        )
        self.assertEqual(argv_text, "")

    def test_a_real_main_process_with_no_type_flag_is_still_matched(self):
        """Non-vacuity: the fix must not turn every pid into a non-match —
        only ever treat this pid as a match when a process is asked."""
        with patch.object(drive_attach, "chrome_pids", return_value=["4242"]), \
                patch.object(drive_attach.subprocess, "run") as mock_run:
            mock_run.return_value = subprocess.CompletedProcess(
                args=["ps"],
                returncode=0,
                stdout="/path/to/chrome --user-data-dir=/tmp/drive-attach-unit-test-udd "
                       "--use-mock-keychain\n",
                stderr="",
            )
            pid, argv_text = drive_attach.chrome_main_pid("/tmp/drive-attach-unit-test-udd")
        self.assertEqual(pid, "4242")
        self.assertIn("--use-mock-keychain", argv_text)

    def test_a_helper_process_carrying_type_is_still_skipped(self):
        """Non-vacuity for the pre-existing --type= filter, not just the new
        empty-output check: the two must not cancel each other out."""
        with patch.object(drive_attach, "chrome_pids", return_value=["4242"]), \
                patch.object(drive_attach.subprocess, "run") as mock_run:
            mock_run.return_value = subprocess.CompletedProcess(
                args=["ps"],
                returncode=0,
                stdout="/path/to/chrome --type=renderer "
                       "--user-data-dir=/tmp/drive-attach-unit-test-udd\n",
                stderr="",
            )
            pid, argv_text = drive_attach.chrome_main_pid("/tmp/drive-attach-unit-test-udd")
        self.assertIsNone(pid, "a --type= helper process must not be matched")
        self.assertEqual(argv_text, "")


if __name__ == "__main__":
    unittest.main()
