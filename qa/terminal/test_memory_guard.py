"""Tests for qa/terminal/memory_guard.py (stdlib unittest, no real-machine dependence)."""

import ctypes
import io
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import memory_guard as mg  # noqa: E402

GIB4 = 4_294_967_296


def ok_probe(n, source="test"):
    return lambda: mg.Probe(True, source, n, None)


class Spy:
    def __init__(self, rc=0):
        self.calls = []
        self.rc = rc

    def __call__(self, cmd):
        self.calls.append(list(cmd))
        return self.rc


def run_main(argv, probe_fn, call_fn):
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        rc = mg.main(argv, probe_fn=probe_fn, call_fn=call_fn)
    return rc, out.getvalue(), err.getvalue()


class ThresholdTests(unittest.TestCase):
    def test_threshold_is_exactly_4gib(self):
        self.assertEqual(mg.THRESHOLD_BYTES, 4_294_967_296)

    def test_boundary(self):
        self.assertTrue(mg.admit(mg.Probe(True, "t", GIB4, None)))
        self.assertTrue(mg.admit(mg.Probe(True, "t", GIB4 + 1, None)))
        self.assertFalse(mg.admit(mg.Probe(True, "t", GIB4 - 1, None)))
        self.assertFalse(mg.admit(mg.Probe(True, "t", 0, None)))

    def test_failed_probe_never_admitted_even_with_big_number(self):
        self.assertFalse(mg.admit(mg.Probe(False, "t", 10**15, "boom")))

    def test_ok_probe_without_bytes_never_admitted(self):
        self.assertFalse(mg.admit(mg.Probe(True, "t", None, None)))


class ParseMeminfoTests(unittest.TestCase):
    def test_parses_kib_to_bytes(self):
        text = "MemTotal:       16000000 kB\nMemAvailable:    4194304 kB\nMemFree: 1 kB\n"
        self.assertEqual(mg.parse_meminfo(text), 4194304 * 1024)
        self.assertEqual(mg.parse_meminfo(text), GIB4)

    def test_missing_line(self):
        with self.assertRaises(ValueError):
            mg.parse_meminfo("MemTotal: 1 kB\nMemFree: 1 kB\n")

    def test_bad_value(self):
        for bad in ("MemAvailable: abc kB\n", "MemAvailable: -5 kB\n",
                    "MemAvailable: kB\n", "MemAvailable:\n", ""):
            with self.assertRaises(ValueError, msg=bad):
                mg.parse_meminfo(bad)

    def test_bad_unit(self):
        with self.assertRaises(ValueError):
            mg.parse_meminfo("MemAvailable: 100 MB\n")
        with self.assertRaises(ValueError):
            mg.parse_meminfo("MemAvailable: 100\n")

    def test_probe_linux_reads_file(self):
        with tempfile.NamedTemporaryFile("w", suffix=".meminfo", delete=False) as f:
            f.write("MemAvailable: 1024 kB\n")
        try:
            p = mg.probe_linux(f.name)
        finally:
            os.unlink(f.name)
        self.assertTrue(p.ok)
        self.assertEqual(p.available_bytes, 1024 * 1024)
        self.assertIn("MemAvailable", p.source)

    def test_probe_linux_missing_file_fails_closed(self):
        p = mg.probe_linux("/nonexistent/definitely/meminfo")
        self.assertFalse(p.ok)
        self.assertIsNone(p.available_bytes)

    def test_probe_linux_garbage_fails_closed(self):
        with tempfile.NamedTemporaryFile("w", delete=False) as f:
            f.write("MemTotal: 1 kB\n")
        try:
            p = mg.probe_linux(f.name)
        finally:
            os.unlink(f.name)
        self.assertFalse(p.ok)
        self.assertFalse(mg.admit(p))


class FakeKernel32:
    """Mimics kernel32.GlobalMemoryStatusEx by filling the real ctypes struct."""

    def __init__(self, avail=None, ret=1, fill_length_check=True):
        self.avail = avail
        self.ret = ret
        self.seen_length = None
        self.GlobalMemoryStatusEx = self._call

    def _call(self, ref):
        s = ref._obj
        self.seen_length = s.dwLength
        if self.ret:
            s.ullAvailPhys = self.avail
            s.ullTotalPhys = self.avail * 2
        return self.ret


class WindowsProbeTests(unittest.TestCase):
    def test_struct_has_real_members_and_size(self):
        s = mg.MEMORYSTATUSEX()
        for name in ("dwLength", "dwMemoryLoad", "ullTotalPhys", "ullAvailPhys",
                     "ullTotalPageFile", "ullAvailPageFile",
                     "ullTotalVirtual", "ullAvailVirtual", "ullAvailExtendedVirtual"):
            self.assertTrue(hasattr(s, name), name)
        self.assertEqual(ctypes.sizeof(mg.MEMORYSTATUSEX), 64)

    def test_reads_ullAvailPhys_and_sets_length(self):
        k = FakeKernel32(avail=GIB4)
        p = mg.probe_windows(k)
        self.assertTrue(p.ok)
        self.assertEqual(p.available_bytes, GIB4)
        self.assertIn("GlobalMemoryStatusEx", p.source)
        self.assertEqual(k.seen_length, 64)

    def test_not_total_phys(self):
        k = FakeKernel32(avail=123456)
        self.assertEqual(mg.probe_windows(k).available_bytes, 123456)

    def test_api_failure_fails_closed(self):
        p = mg.probe_windows(FakeKernel32(avail=GIB4 * 10, ret=0))
        self.assertFalse(p.ok)
        self.assertIsNone(p.available_bytes)

    def test_exception_fails_closed(self):
        class Boom:
            def GlobalMemoryStatusEx(self, ref):
                raise OSError("nope")

        p = mg.probe_windows(Boom())
        self.assertFalse(p.ok)

    def test_zero_avail_is_not_trusted_as_big(self):
        p = mg.probe_windows(FakeKernel32(avail=0))
        self.assertFalse(mg.admit(p))


class DispatchTests(unittest.TestCase):
    def test_unsupported_platform(self):
        for plat in ("darwin", "freebsd13", "", "weird"):
            p = mg.probe(platform=plat)
            self.assertFalse(p.ok, plat)
            self.assertFalse(mg.admit(p))
            self.assertIn("unsupported", p.reason.lower())

    def test_linux_dispatch_uses_injected_reader(self):
        p = mg.probe(platform="linux", linux_probe=lambda: mg.Probe(True, "x", 5, None))
        self.assertEqual(p.available_bytes, 5)

    def test_windows_dispatch_uses_injected_reader(self):
        p = mg.probe(platform="win32", windows_probe=lambda: mg.Probe(True, "w", 7, None))
        self.assertEqual(p.available_bytes, 7)

    def test_probe_exception_is_fail_closed(self):
        def boom():
            raise RuntimeError("x")

        p = mg.probe(platform="linux", linux_probe=boom)
        self.assertFalse(p.ok)
        self.assertFalse(mg.admit(p))

    def test_unknown_probe_result_denied(self):
        self.assertFalse(mg.admit(mg.probe(platform="plan9")))


class CliTests(unittest.TestCase):
    def test_denied_does_not_spawn(self):
        spy = Spy()
        rc, out, err = run_main(["--", "echo", "hi"], ok_probe(GIB4 - 1), spy)
        self.assertNotEqual(rc, 0)
        self.assertEqual(spy.calls, [])
        self.assertIn("DENIED", out + err)

    def test_unknown_probe_denied_no_spawn(self):
        spy = Spy()
        rc, out, err = run_main(
            ["--", "echo"], lambda: mg.Probe(False, "none", None, "unknown"), spy)
        self.assertNotEqual(rc, 0)
        self.assertEqual(spy.calls, [])

    def test_admitted_runs_command_with_args(self):
        spy = Spy(rc=0)
        rc, out, err = run_main(["--", "cmd", "-x", "--flag", "a b"], ok_probe(GIB4), spy)
        self.assertEqual(rc, 0)
        self.assertEqual(spy.calls, [["cmd", "-x", "--flag", "a b"]])

    def test_exit_code_propagates(self):
        for code in (1, 7, 42):
            spy = Spy(rc=code)
            rc, _, _ = run_main(["--", "c"], ok_probe(GIB4 * 2), spy)
            self.assertEqual(rc, code)

    def test_output_states_not_a_reservation(self):
        rc, out, err = run_main(["--", "c"], ok_probe(GIB4, "src-x"), Spy())
        text = out + err
        self.assertIn("src-x", text)
        self.assertIn(str(GIB4), text)
        self.assertIn("probe", text.lower())
        self.assertIn("4294967296", text)
        self.assertIn("not a reservation", text.lower())
        self.assertIn("watchdog", text.lower())

    def test_check_only(self):
        spy = Spy()
        rc, _, _ = run_main(["--check-only"], ok_probe(GIB4), spy)
        self.assertEqual(rc, 0)
        self.assertEqual(spy.calls, [])
        rc, _, _ = run_main(["--check-only"], ok_probe(GIB4 - 1), spy)
        self.assertNotEqual(rc, 0)

    def test_no_command_is_check_only(self):
        spy = Spy()
        rc, _, _ = run_main([], ok_probe(GIB4), spy)
        self.assertEqual(rc, 0)
        self.assertEqual(spy.calls, [])
        rc, _, _ = run_main([], ok_probe(GIB4 - 1), spy)
        self.assertEqual(rc, 3)
        self.assertEqual(spy.calls, [])

    def test_empty_separator_is_usage_error_not_admitted(self):
        for argv in (["--"], ["--check-only", "--"]):
            for n in (GIB4, GIB4 - 1):
                probes = []

                def pf(n=n):
                    probes.append(1)
                    return mg.Probe(True, "s", n, None)

                spy = Spy()
                rc, out, err = run_main(argv, pf, spy)
                self.assertEqual(rc, 2, (argv, n))
                self.assertEqual(probes, [], (argv, n))  # usage error: no probe
                self.assertEqual(spy.calls, [], (argv, n))
                self.assertNotIn("ADMITTED", (out + err).replace("result=USAGE", ""))
                self.assertIn("usage", err.lower())

    def test_check_only_with_command_is_usage_error(self):
        for argv in (["--check-only", "--", "cargo", "check"],
                     ["--check-only", "--", "c"]):
            probes = []

            def pf():
                probes.append(1)
                return mg.Probe(True, "s", GIB4, None)

            spy = Spy()
            rc, out, err = run_main(argv, pf, spy)
            self.assertEqual(rc, 2, argv)
            self.assertEqual(probes, [], argv)
            self.assertEqual(spy.calls, [], argv)
            self.assertIn("usage", err.lower())

    def test_command_flags_not_swallowed(self):
        spy = Spy()
        run_main(["--", "c", "--check-only"], ok_probe(GIB4), spy)
        self.assertEqual(spy.calls, [["c", "--check-only"]])

    def test_unknown_option_is_usage_error(self):
        spy = Spy()
        rc, _, _ = run_main(["--bogus"], ok_probe(GIB4), spy)
        self.assertEqual(rc, 2)
        self.assertEqual(spy.calls, [])
        rc, _, _ = run_main(["cmd"], ok_probe(GIB4), spy)  # command without `--`
        self.assertEqual(rc, 2)
        self.assertEqual(spy.calls, [])

    def test_probe_called_once_per_run(self):
        n = []

        def p():
            n.append(1)
            return mg.Probe(True, "s", GIB4, None)

        run_main(["--", "c"], p, Spy())
        self.assertEqual(len(n), 1)


class ResultSignalTests(unittest.TestCase):
    """Exit code 3 is ambiguous (denied vs. command's own 3); stderr disambiguates."""

    def test_denied_has_machine_readable_result(self):
        spy = Spy()
        rc, out, err = run_main(["--", "c"], ok_probe(GIB4 - 1), spy)
        self.assertEqual(rc, 3)
        self.assertIn("result=DENIED", err)
        self.assertIn("DENIED", err)
        self.assertNotIn("command_exit=", err)
        self.assertNotIn("result=ADMITTED", err)

    def test_admitted_check_only_has_result(self):
        rc, out, err = run_main(["--check-only"], ok_probe(GIB4), Spy())
        self.assertEqual(rc, 0)
        self.assertIn("result=ADMITTED", err)
        self.assertNotIn("command_exit=", err)

    def test_command_exit_3_is_admitted_command_error(self):
        spy = Spy(rc=3)
        rc, out, err = run_main(["--", "c"], ok_probe(GIB4), spy)
        self.assertEqual(rc, 3)
        self.assertEqual(spy.calls, [["c"]])
        self.assertIn("result=ADMITTED", err)
        self.assertIn("command_exit=3", err)
        self.assertNotIn("result=DENIED", err)
        self.assertEqual(out, "")  # command stdout stays uncontaminated

    def test_command_exit_other_codes_reported(self):
        rc, out, err = run_main(["--", "c"], ok_probe(GIB4), Spy(rc=42))
        self.assertEqual(rc, 42)
        self.assertIn("command_exit=42", err)
        self.assertEqual(out, "")

    def test_result_line_precedes_command_exit(self):
        _, _, err = run_main(["--", "c"], ok_probe(GIB4), Spy(rc=3))
        self.assertIn("result=ADMITTED", err)
        self.assertIn("command_exit=3", err)
        self.assertLess(err.index("result=ADMITTED"), err.index("command_exit=3"))

    def test_module_doc_states_exit_code_ambiguity(self):
        self.assertIn("result=", mg.__doc__)
        self.assertIn("command_exit=", mg.__doc__)
        self.assertIn("ambiguous", mg.__doc__.lower())


class DefaultCallErrorTests(unittest.TestCase):
    def test_generic_oserror_is_126(self):
        with mock.patch.object(mg.subprocess, "call", side_effect=OSError(8, "Exec format error")):
            with redirect_stderr(io.StringIO()):
                try:
                    rc = mg.default_call(["x"])
                except OSError as e:
                    self.fail("generic OSError escaped default_call: %r" % e)
                self.assertEqual(rc, 126)

    def test_not_found_and_permission_unchanged(self):
        with mock.patch.object(mg.subprocess, "call", side_effect=FileNotFoundError("x")):
            with redirect_stderr(io.StringIO()):
                self.assertEqual(mg.default_call(["x"]), 127)
        with mock.patch.object(mg.subprocess, "call", side_effect=PermissionError("x")):
            with redirect_stderr(io.StringIO()):
                self.assertEqual(mg.default_call(["x"]), 126)

    def test_keyboard_interrupt_not_swallowed(self):
        with mock.patch.object(mg.subprocess, "call", side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
                mg.default_call(["x"])

        def interrupted(cmd):
            raise KeyboardInterrupt

        with redirect_stderr(io.StringIO()):
            with self.assertRaises(KeyboardInterrupt):
                mg.main(["--", "c"], probe_fn=ok_probe(GIB4), call_fn=interrupted)


class RealSubprocessTests(unittest.TestCase):
    def test_default_call_propagates_exit_code(self):
        rc = mg.default_call([sys.executable, "-c", "import sys; sys.exit(5)"])
        self.assertEqual(rc, 5)

    def test_default_call_missing_binary(self):
        self.assertEqual(mg.default_call(["/nonexistent/bin/zzz"]), 127)

    def test_main_end_to_end_with_fake_probe(self):
        rc, _, _ = run_main(["--", sys.executable, "-c", "import sys; sys.exit(9)"],
                            ok_probe(GIB4), mg.default_call)
        self.assertEqual(rc, 9)


if __name__ == "__main__":
    unittest.main()
