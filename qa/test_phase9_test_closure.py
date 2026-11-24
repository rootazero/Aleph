"""Behavioural checks for the Phase9 process receipt boundary."""
import importlib.util
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location("closure", Path(__file__).with_name("phase9_test_closure.py"))
closure = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(closure)


class ClosureTests(unittest.TestCase):
    def run_child(self, code, timeout=5):
        with tempfile.TemporaryDirectory() as directory:
            return closure.run_command([sys.executable, "-c", code], Path(directory), {}, Path(directory) / "child", timeout)

    def test_group_census_distinguishes_an_owned_live_group_from_a_reaped_one(self):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"], start_new_session=True)
        try:
            self.assertIn(child.pid, closure.process_group_members(child.pid))
        finally:
            os.killpg(child.pid, signal.SIGTERM)
            child.wait()
        self.assertEqual(closure.process_group_members(child.pid), [])

    def test_terminal_pass_requires_nonzero_executed_tests(self):
        result = self.run_child("print('test result: ok. 1 passed; 0 failed; 0 ignored; 2 filtered out')")
        self.assertEqual(closure.classify(result), "PASS")
        result = self.run_child("print('test result: ok. 0 passed; 0 failed; 0 ignored; 2 filtered out')")
        self.assertEqual(closure.classify(result), "FAIL")

    def test_success_without_test_summary_is_not_pass(self):
        self.assertEqual(closure.classify(self.run_child("print('Finished compilation')")), "FAIL")

    def test_unexplained_timeout_remains_unresolved(self):
        result = self.run_child("import time; time.sleep(30)", 0.15)
        self.assertEqual(closure.classify(result), "UNRESOLVED_TIMEOUT")
        self.assertIsNotNone(result["exit_code"])

    def test_fixture_redirects_runtime_roots_but_keeps_toolchain_cache(self):
        with tempfile.TemporaryDirectory() as directory:
            environment = closure.fixture_environment(Path(directory), {"HOME": "/cache-account", "CARGO_HOME": "/cargo-cache", "RUSTUP_HOME": "/rustup-cache"})
            result = closure.run_command([sys.executable, "-c", "import os,pathlib; pathlib.Path.home().joinpath('.agents').mkdir(); pathlib.Path(os.environ['ALEPH_HOME']).joinpath('probe').touch()"], Path(directory), environment, Path(directory) / "roots", 5)
            self.assertEqual(result["exit_code"], 0)
            self.assertTrue((Path(directory) / "home/.agents").is_dir())
            self.assertTrue((Path(directory) / "aleph/probe").is_file())
            self.assertEqual(environment["CARGO_HOME"], "/cargo-cache")
            self.assertEqual(environment["RUSTUP_HOME"], "/rustup-cache")

    def test_home_can_be_owned_outside_the_sandbox_temp_grant(self):
        with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryDirectory() as home:
            environment = closure.fixture_environment(Path(directory), {"HOME": "/cache-account"}, home=Path(home))
            result = closure.run_command([sys.executable, "-c", "from pathlib import Path; Path.home().joinpath('probe').touch()"], Path(directory), environment, Path(directory) / "external-home", 5)
            self.assertEqual(result["exit_code"], 0)
            self.assertTrue((Path(home) / "probe").is_file())
            self.assertNotEqual(environment["ALEPH_HOME"], str(Path(home) / ".aleph"))
            if os.name != "nt":
                self.assertNotIn("USERPROFILE", environment)

    def test_successful_parent_with_unjoined_child_fails_and_cleans_group(self):
        result = self.run_child("import subprocess,sys; subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)']); print('test result: ok. 1 passed; 0 failed;')")
        self.assertTrue(result["unjoined_descendants"])
        self.assertEqual(closure.classify(result), "FAIL")

    def test_termination_of_runner_joins_its_direct_child(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            code = "import os,sys,time; from pathlib import Path; Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(30)"
            program = "import sys; from pathlib import Path; sys.path.insert(0,sys.argv[1]); import phase9_test_closure as closure; root=Path(sys.argv[2]); closure.run_command([sys.executable,'-c',sys.argv[3],str(root/'pid')],root,{},root/'cancel',30)"
            wrapper = subprocess.Popen([sys.executable, "-c", program, str(Path(__file__).parent.resolve()), directory, code])
            child = None
            try:
                for _ in range(200):
                    if (root / "pid").exists():
                        break
                    time.sleep(0.01)
                child = int((root / "pid").read_text())
                wrapper.send_signal(signal.SIGTERM)
                self.assertIsNotNone(wrapper.wait(timeout=5))
                with self.assertRaises(ProcessLookupError):
                    os.kill(child, 0)
            finally:
                if wrapper.poll() is None:
                    wrapper.kill()
                    wrapper.wait()
                if child is not None:
                    try:
                        os.killpg(child, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    def test_nested_runner_cancellation_allows_inner_owner_to_join(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            leaf = "import os,sys,time; from pathlib import Path; Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(30)"
            inner = "import sys; from pathlib import Path; sys.path.insert(0,sys.argv[1]); import phase9_test_closure as c; p=Path(sys.argv[2]); c.run_command([sys.executable,'-c',sys.argv[3],str(p/'pid')],p,{},p/'inner',30)"
            outer = "import sys; from pathlib import Path; sys.path.insert(0,sys.argv[1]); import phase9_test_closure as c; p=Path(sys.argv[2]); c.run_command([sys.executable,'-c',sys.argv[3],sys.argv[1],sys.argv[2],sys.argv[4]],p,{},p/'outer',30)"
            wrapper = subprocess.Popen([sys.executable, "-c", outer, str(Path(__file__).parent.resolve()), directory, inner, leaf])
            child = None
            try:
                for _ in range(200):
                    if (root / "pid").exists():
                        break
                    time.sleep(0.01)
                child = int((root / "pid").read_text())
                wrapper.send_signal(signal.SIGTERM)
                wrapper.wait(timeout=10)
                with self.assertRaises(ProcessLookupError):
                    os.kill(child, 0)
            finally:
                if wrapper.poll() is None:
                    wrapper.kill()
                    wrapper.wait()
                if child is not None:
                    try:
                        os.killpg(child, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    def test_dotted_prefix_preserves_distinct_memory_and_cargo_logs(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory) / "20261116T130000.123Z-memory"
            result = closure.run_command([sys.executable, "-c", "print('ALLOW')"], Path(directory), {}, prefix, 5)
            self.assertEqual(result["stdout"], str(prefix) + ".stdout.log")

    def test_cleanup_permission_denial_cannot_be_pass(self):
        result = self.run_child("print('test result: ok. 1 passed; 0 failed;')")
        result["cleanup_error"] = "[Errno 1] Operation not permitted"
        self.assertEqual(closure.classify(result), "FAIL")

    def test_assertion_failure_is_fail(self):
        result = self.run_child("raise AssertionError('owned worker was not joined')")
        self.assertEqual(closure.classify(result), "FAIL")

    def test_missing_offline_dependency_is_blocked(self):
        result = self.run_child("import sys; print('error: no matching package named `missing` found', file=sys.stderr); sys.exit(101)")
        self.assertEqual(closure.classify(result), "BLOCKED")


if __name__ == "__main__":
    unittest.main()
