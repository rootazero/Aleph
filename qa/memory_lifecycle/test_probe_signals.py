"""Check lifecycle dispatch without RPC dependencies or a server build."""
import importlib.util
from pathlib import Path
import signal
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, Mock, call, patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "memory_lifecycle_probe", Path(__file__).with_name("probe.py")
)
probe = importlib.util.module_from_spec(SPEC)
# Signal dispatch never uses RPC; keep this regression stdlib-only.
with patch.dict(sys.modules, {"qa_rpc": Mock()}):
    SPEC.loader.exec_module(probe)


class ProbeSignalTests(unittest.TestCase):
    def run_dispatch(self, windows: bool) -> tuple[list[tuple], Mock, Mock]:
        observed = []
        process = Mock(pid=12345)
        runner = SimpleNamespace(
            prepare=Mock(), cleanup=Mock(), check=Mock(),
            binary=ROOT / "unused-server", process=process,
        )
        deliver = probe.Probe.deliver_shutdown_signal

        async def scenario(name: str, sig: int, first: bool) -> None:
            observed.append((name, sig, first))
            deliver(runner, sig)

        runner.scenario = AsyncMock(side_effect=scenario)
        with (
            patch.object(probe, "IS_WINDOWS", windows),
            patch.object(probe, "Probe", return_value=runner),
            patch.object(probe, "digest", return_value="test-digest"),
            patch.object(
                probe, "deliver_control_event", create=True,
                return_value={"generated": True},
            ) as console,
            patch.object(sys, "argv", [
                "probe.py", "--binary", str(runner.binary),
                "--sha256", "test-digest",
            ]),
        ):
            probe.main()
        self.assertTrue(runner.completed)
        runner.prepare.assert_called_once_with()
        runner.cleanup.assert_called_once_with()
        return observed, process, console

    def test_posix_dispatch_reaches_process_with_sigterm_then_sigint(self) -> None:
        observed, process, console = self.run_dispatch(windows=False)
        self.assertEqual(observed, [
            ("sigterm", signal.SIGTERM, True),
            ("sigint-restart", signal.SIGINT, False),
        ])
        self.assertEqual(process.send_signal.call_args_list, [
            call(signal.SIGTERM), call(signal.SIGINT),
        ])
        console.assert_not_called()

    def test_windows_dispatch_requires_ctrl_c_on_both_boots(self) -> None:
        observed, process, console = self.run_dispatch(windows=True)
        self.assertEqual(observed, [
            ("ctrlc", signal.SIGINT, True),
            ("ctrlc-restart", signal.SIGINT, False),
        ])
        process.send_signal.assert_not_called()
        self.assertEqual(console.call_args_list, [
            call(process.pid, "ctrl_c"), call(process.pid, "ctrl_c"),
        ])

    @unittest.skipIf(probe.IS_WINDOWS, "POSIX signal reception only")
    def test_posix_child_receives_each_requested_signal(self) -> None:
        code = (
            "import signal\n"
            "def received(sig, frame):\n"
            "    print(sig, flush=True)\n"
            "    raise SystemExit(0)\n"
            "signal.signal(signal.SIGTERM, received)\n"
            "signal.signal(signal.SIGINT, received)\n"
            "print('ready', flush=True)\n"
            "signal.pause()\n"
        )
        for sig in (signal.SIGTERM, signal.SIGINT):
            with self.subTest(signal=sig):
                with subprocess.Popen(
                    [sys.executable, "-I", "-B", "-c", code], cwd=ROOT,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                ) as process:
                    try:
                        self.assertEqual(process.stdout.readline().strip(), "ready")
                        result = probe.Probe.deliver_shutdown_signal(
                            SimpleNamespace(process=process), sig
                        )
                        stdout, stderr = process.communicate(timeout=5)
                        self.assertEqual(result, {"signal": int(sig)})
                        self.assertEqual(process.returncode, 0, stderr)
                        self.assertEqual(stdout.strip(), str(int(sig)))
                    finally:
                        if process.poll() is None:
                            process.kill()
                            process.wait()


if __name__ == "__main__":
    unittest.main()
