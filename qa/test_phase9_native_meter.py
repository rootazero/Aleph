"""Public macOS exit/settlement evidence uses actual non-child processes."""
import os
from contextlib import closing
import select
import subprocess
import sys
import threading
import unittest
from unittest.mock import patch

import nsh_panel_launcher as launcher


@unittest.skipUnless(sys.platform == "darwin", "macOS kqueue contract")
class NativeMeterTests(unittest.TestCase):
    def test_final_catalog_rejects_new_unowned_webkit_after_owner_record_disappears(self):
        known = subprocess.Popen(["/bin/sleep", "10"])
        unknown = subprocess.Popen(["/bin/sleep", "10"])
        instance = {"owner_pid": os.getpid(), "coalition_id": 51, "owner_name": "Owned"}
        listing = (f'1) "Owned Graphics and Media"\n    bundleID="com.apple.WebKit.GPU"\n'
                   f'    pid = {unknown.pid}\n')
        try:
            with self.assertRaisesRegex(ValueError, "unowned WebKit"):
                launcher.validate_terminal_catalog(listing, instance, {known.pid}, ())
            unknown.terminate()
            unknown.wait(timeout=5)
            with self.assertRaisesRegex(ValueError, "unowned WebKit"):
                launcher.validate_terminal_catalog(listing, instance, {known.pid}, ())
            known.terminate()
            known.wait(timeout=5)
            stale = listing.replace(str(unknown.pid), str(known.pid))
            self.assertEqual(launcher.validate_terminal_catalog(stale, instance, {known.pid}, [known.pid])["pids"], ())
            with self.assertRaisesRegex(ValueError, "unowned WebKit"):
                launcher.validate_terminal_catalog(stale, instance, {known.pid}, ())
        finally:
            for child in (known, unknown):
                if child.poll() is None:
                    child.terminate()
                    child.wait(timeout=5)

    def test_terminal_catalog_remnant_requires_previously_owned_absent_process(self):
        owner = os.getpid()
        child = subprocess.Popen(["/bin/sleep", "10"])
        listing = (f'1) "Owned"\n    pid = {owner}\n    coalition: 51\n'
                   f'2) "Owned Graphics and Media"\n    bundleID="com.apple.WebKit.GPU"\n    pid = {child.pid}\n')
        instance = {"owner_pid": owner, "coalition_id": 51}
        try:
            with self.assertRaisesRegex(ValueError, "unowned WebKit"):
                launcher.confirm_terminal_departures(listing, instance, {owner, child.pid})
            child.terminate()
            child.wait(timeout=5)
            with self.assertRaisesRegex(ValueError, "unowned WebKit"):
                launcher.parse_launchservices_coalitions(listing, {owner}, require_webkit=False)
            self.assertEqual(launcher.confirm_terminal_departures(listing, instance, {owner, child.pid}), [child.pid])
            with self.assertRaisesRegex(ValueError, "unowned WebKit"):
                launcher.confirm_terminal_departures(listing, instance, {owner})
            unknown = subprocess.Popen(["/bin/sleep", "10"])
            try:
                combined = listing + (f'3) "Owned Networking"\n    bundleID="com.apple.WebKit.Networking"\n    pid = {unknown.pid}\n')
                with self.assertRaisesRegex(ValueError, "unowned WebKit"):
                    launcher.confirm_terminal_departures(combined, instance, {owner, child.pid})
            finally:
                unknown.terminate()
                unknown.wait(timeout=5)
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)

    def test_new_member_requires_positive_identity_after_transient_eperm(self):
        owner = launcher.native_process_identity(os.getpid())
        child = subprocess.Popen(["/bin/sleep", "10"])
        query = launcher.native_process_identity
        attempts, observations = [], []
        def transient(pid):
            if pid == child.pid:
                attempts.append(pid)
                if len(attempts) == 1:
                    raise ValueError(f"native_process_identity_unproven:pid={pid}:bytes=0:errno=1")
            return query(pid)
        members, identities = {owner["pid"]}, {owner["pid"]: owner}
        try:
            with patch.object(launcher, "native_process_identity", transient):
                launcher.accept_owned_coalition(
                    {"coalition_id": 51, "pids": (owner["pid"], child.pid)},
                    {"coalition_id": 51, "owner_pid": owner["pid"]},
                    owner, members, identities, identity_observations=observations)
            self.assertEqual(identities[child.pid], query(child.pid))
            self.assertEqual(len(attempts), 2)
            self.assertEqual(observations[0]["reason"], f"native_process_identity_unproven:pid={child.pid}:bytes=0:errno=1")
            self.assertEqual(observations[-1]["identity"], identities[child.pid])
        finally:
            child.terminate()
            child.wait(timeout=5)

    def test_identity_retry_never_accepts_persistent_or_other_permission_failures(self):
        owner = launcher.native_process_identity(os.getpid())
        child = subprocess.Popen(["/bin/sleep", "10"])
        query = launcher.native_process_identity
        try:
            for denied_errno, expected_attempts in [(1, 10), (13, 1)]:
                attempts, observations = [], []
                def denied(pid):
                    if pid == child.pid:
                        attempts.append(pid)
                        raise ValueError(f"native_process_identity_unproven:pid={pid}:bytes=0:errno={denied_errno}")
                    return query(pid)
                members, identities = {owner["pid"]}, {owner["pid"]: owner}
                with patch.object(launcher, "native_process_identity", denied):
                    with self.assertRaisesRegex(ValueError, f"errno={denied_errno}"):
                        launcher.accept_owned_coalition(
                            {"coalition_id": 51, "pids": (owner["pid"], child.pid)},
                            {"coalition_id": 51, "owner_pid": owner["pid"]},
                            owner, members, identities, identity_observations=observations)
                self.assertEqual(len(attempts), expected_attempts)
                self.assertNotIn(child.pid, identities)
                self.assertTrue(all("identity" not in observation for observation in observations))
        finally:
            child.terminate()
            child.wait(timeout=5)

    def test_exec_transition_preserves_successful_kernel_zero_rss(self):
        observed_zero = False
        for _ in range(40):
            child = subprocess.Popen(["/bin/sh", "-c", "exec /usr/bin/true"])
            try:
                while child.poll() is None:
                    try:
                        sample = launcher.sample_launchservices_members([child.pid])
                    except ValueError as error:
                        if str(error).startswith("missing_processes:"):
                            break
                        raise
                    observed_zero |= sample["rss_mb"] == 0 and sample["threads"] > 0
            finally:
                child.wait(timeout=5)
            if observed_zero:
                break
        self.assertTrue(observed_zero, "controlled exec never exposed the kernel zero-RSS transition")

    def test_task_info_tracks_real_resident_memory_and_live_threads(self):
        before = launcher.sample_launchservices_members([os.getpid()])
        stop = threading.Event()
        ready = [threading.Event() for _ in range(4)]
        workers = [threading.Thread(target=lambda signal=signal: (signal.set(), stop.wait())) for signal in ready]
        memory = bytearray(32 * 1024 * 1024)
        for offset in range(0, len(memory), 4096):
            memory[offset] = 1
        try:
            for worker in workers:
                worker.start()
            for signal in ready:
                self.assertTrue(signal.wait(2))
            after = launcher.sample_launchservices_members([os.getpid()])
            self.assertGreaterEqual(after["rss_mb"] - before["rss_mb"], 30)
            self.assertGreaterEqual(after["threads"] - before["threads"], 4)
        finally:
            stop.set()
            for worker in workers:
                worker.join(timeout=2)
        self.assertEqual(launcher.sample_launchservices_members([os.getpid()])["threads"], before["threads"])

    def test_nonchild_nonzero_exit_is_not_helper_success(self):
        parent = subprocess.Popen([sys.executable, "-c", 'import subprocess,sys; p=subprocess.Popen([sys.executable,"-c","import time; time.sleep(.15); raise SystemExit(42)"]); print(p.pid,flush=True); p.wait()'], stdout=subprocess.PIPE, text=True)
        try:
            pid = int(parent.stdout.readline())
            with closing(launcher.watch_native_exit(pid)) as watcher:
                record = launcher.native_exit_status(watcher, pid, 3)
            self.assertEqual(record["pid"], pid)
            self.assertEqual(record["exit_code"], 42)
            self.assertEqual(record["wait_status"], 42 << 8)
        finally:
            parent.communicate(timeout=5)

    def test_task_sampler_rejects_a_joined_real_process(self):
        child = subprocess.Popen(["/usr/bin/true"])
        child.wait(timeout=5)
        with self.assertRaisesRegex(ValueError, "missing_processes"):
            launcher.sample_launchservices_members([child.pid])

    def test_failure_cleanup_settles_members_after_helper_has_exited(self):
        helper = subprocess.Popen(["/usr/bin/true"])
        helper.wait(timeout=5)
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        waiter = threading.Thread(target=child.wait)
        waiter.start()
        try:
            identity = launcher.native_process_identity(child.pid)
            self.assertIsNotNone(identity)
            result = launcher.cleanup_owned_members({child.pid: identity}, timeout=1)
            waiter.join(timeout=2)
            self.assertTrue(result["complete"], result)
            self.assertEqual(result["survivors"], [])
            self.assertEqual(child.returncode, -15)
            self.assertEqual(result["actions"][0]["identity"], identity)
        finally:
            if child.poll() is None:
                child.kill()
            waiter.join(timeout=5)

    def test_failure_cleanup_never_signals_a_changed_process_identity(self):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        try:
            identity = launcher.native_process_identity(child.pid)
            self.assertIsNotNone(identity)
            identity["start_usec"] += 1
            result = launcher.cleanup_owned_members({child.pid: identity}, timeout=.01)
            self.assertEqual(result["actions"], [])
            self.assertEqual(result["survivors"], [])
            self.assertEqual(result["snapshots"][0]["identity_changed"][0]["expected"], identity)
            self.assertIsNone(child.poll())
        finally:
            child.terminate()
            child.wait(timeout=5)

    def test_rejected_coalition_cannot_expand_cleanup_authority(self):
        members, identities = {os.getpid()}, {}
        expected = launcher.native_process_identity(os.getpid())
        with self.assertRaisesRegex(ValueError, "ownership_changed:coalition_id"):
            launcher.accept_owned_coalition(
                {"coalition_id": 52, "pids": (os.getpid(), 1)},
                {"owner_pid": os.getpid(), "coalition_id": 51}, expected,
                members, identities,
            )
        self.assertEqual(members, {os.getpid()})
        self.assertEqual(identities, {})

    def test_changed_owner_identity_cannot_expand_cleanup_authority(self):
        members, identities = {os.getpid()}, {}
        expected = launcher.native_process_identity(os.getpid())
        expected["start_usec"] += 1
        with self.assertRaisesRegex(ValueError, "ownership_changed:owner_identity"):
            launcher.accept_owned_coalition(
                {"coalition_id": 51, "pids": (os.getpid(), 1)},
                {"owner_pid": os.getpid(), "coalition_id": 51}, expected,
                members, identities,
            )
        self.assertEqual(members, {os.getpid()})
        self.assertEqual(identities, {})

    def test_terminal_listing_adds_a_live_member_after_host_departure(self):
        host = subprocess.Popen(["/bin/sleep", ".1"])
        identity = launcher.native_process_identity(host.pid)
        host.wait(timeout=5)
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        try:
            listing = f'1) "Owned" ASN:1\n    pid = {host.pid}\n    coalition: 51 {{ {child.pid} }}\n'
            parsed = launcher.parse_launchservices_coalitions(
                listing, {host.pid}, require_webkit=False, allow_departed_host=True,
            )[host.pid]
            members, identities = {host.pid}, {host.pid: identity}
            launcher.accept_owned_coalition(parsed, {"owner_pid": host.pid, "coalition_id": 51},
                                            identity, members, identities, departing=True)
            self.assertIn(child.pid, members)
            result = launcher.settle_native_members(members, timeout=.01)
            self.assertIn(child.pid, result["survivors"])
        finally:
            child.terminate()
            child.wait(timeout=5)

    def test_missing_owner_record_still_tracks_a_live_pinned_coalition_member(self):
        host = subprocess.Popen(["/bin/sleep", ".1"])
        identity = launcher.native_process_identity(host.pid)
        host.wait(timeout=5)
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        try:
            listing = f'1) "Owned XPC" ASN:1\n    pid = {child.pid}\n    coalition: 51 {{ {child.pid} }}\n'
            parsed = launcher.parse_terminal_coalition(listing, host.pid, 51)
            members, identities = {host.pid}, {host.pid: identity}
            launcher.accept_owned_coalition(parsed, {"owner_pid": host.pid, "coalition_id": 51},
                                            identity, members, identities, departing=True)
            self.assertIn(child.pid, members)
            self.assertIn(child.pid, launcher.settle_native_members(members, timeout=.01)["survivors"])
            self.assertEqual(launcher.parse_terminal_coalition("", host.pid, 51)["pids"], ())
        finally:
            child.terminate()
            child.wait(timeout=5)

    def test_settlement_does_not_accept_a_surviving_member(self):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(5)"])
        try:
            record = launcher.settle_native_members({child.pid}, .02)
            self.assertEqual(record["survivors"], [child.pid])
        finally:
            child.terminate()
            child.wait(timeout=5)
        self.assertEqual(launcher.settle_native_members({child.pid}, .02)["survivors"], [])
