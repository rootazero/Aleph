"""Behavior tests for the fail-closed build admission guard."""

import contextlib
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import memory_guard


class ParseMeminfoTests(unittest.TestCase):
    def test_kib_to_bytes(self):
        self.assertEqual(
            memory_guard.parse_meminfo("MemAvailable: 4194304 kB\n"),
            4_294_967_296,
        )

    def test_missing_memavailable_is_unknown(self):
        self.assertIsNone(memory_guard.parse_meminfo("MemFree: 99999999 kB\n"))

    def test_untrusted_memavailable_is_unknown(self):
        for text in (
            "MemAvailable: -1 kB\n",
            "MemAvailable: 4194304 MB\n",
            "MemAvailable: unknown kB\n",
            "MemAvailable: 4 kB\nMemAvailable: 9 kB\n",
            "MemAvailable: 4 kB extra\n",
        ):
            with self.subTest(text=text):
                self.assertIsNone(memory_guard.parse_meminfo(text))

    def test_zero_is_a_valid_low_memory_sample(self):
        self.assertEqual(memory_guard.parse_meminfo("MemAvailable: 0 kB\n"), 0)


class GuardExecutionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.marker = Path(self.temp.name) / "child-ran"
        self.command = [
            sys.executable,
            "-c",
            "from pathlib import Path; import sys; "
            "Path(sys.argv[1]).write_text('ran', encoding='utf-8'); "
            "raise SystemExit(int(sys.argv[2]))",
            str(self.marker),
            "0",
        ]
        self.stderr = io.StringIO()

    def run_guard(self, samples):
        with (
            patch.object(memory_guard, "available_memory_bytes", side_effect=samples),
            contextlib.redirect_stderr(self.stderr),
        ):
            return memory_guard.run_guarded(self.command)

    def test_probe_failure_never_spawns(self):
        self.assertNotEqual(self.run_guard([None]), 0)
        self.assertFalse(self.marker.exists())
        self.assertIn("probe unavailable", self.stderr.getvalue())

    def test_exact_threshold_admits_real_child(self):
        self.assertEqual(self.run_guard([4_294_967_296]), 0)
        self.assertEqual(self.marker.read_text(encoding="utf-8"), "ran")

    def test_low_memory_waits_then_rechecks_before_child(self):
        with patch.object(memory_guard.time, "sleep") as sleep:
            self.assertEqual(self.run_guard([4_294_967_295, 4_294_967_296]), 0)
        sleep.assert_called_once_with(5)
        self.assertEqual(self.marker.read_text(encoding="utf-8"), "ran")

    def test_probe_lost_after_wait_does_not_spawn(self):
        with patch.object(memory_guard.time, "sleep"):
            self.assertNotEqual(self.run_guard([1, None]), 0)
        self.assertFalse(self.marker.exists())

    def test_child_failure_is_not_reported_as_success(self):
        self.command[-1] = "7"
        self.assertEqual(self.run_guard([4_294_967_296]), 7)
        self.assertTrue(self.marker.exists())

    def test_interrupt_before_admission_never_spawns(self):
        with patch.object(memory_guard.time, "sleep", side_effect=KeyboardInterrupt):
            self.assertEqual(self.run_guard([0]), 130)
        self.assertFalse(self.marker.exists())

    def test_invalid_probe_values_do_not_admit(self):
        for value in (True, -1, 4_294_967_296.0):
            with self.subTest(value=value):
                self.assertNotEqual(self.run_guard([value]), 0)
                self.assertFalse(self.marker.exists())

    def test_missing_executable_fails_closed(self):
        self.command = [str(Path(self.temp.name) / "missing-executable")]
        self.assertNotEqual(self.run_guard([4_294_967_296]), 0)
        self.assertFalse(self.marker.exists())


class ProbeAndCliTests(unittest.TestCase):
    def test_unsupported_host_is_unknown(self):
        with patch.object(memory_guard.sys, "platform", "unsupported"):
            self.assertIsNone(memory_guard.available_memory_bytes())

    def test_linux_probe_failure_is_unknown(self):
        with (
            patch.object(memory_guard.sys, "platform", "linux"),
            patch.object(Path, "read_text", side_effect=OSError("probe unavailable")),
        ):
            self.assertIsNone(memory_guard.available_memory_bytes())

    def test_cli_rejects_missing_command(self):
        result = subprocess.run(
            [sys.executable, str(Path(memory_guard.__file__)), "--"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("command", result.stderr)


if __name__ == "__main__":
    unittest.main()
