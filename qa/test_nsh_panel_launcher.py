#!/usr/bin/env python3
"""Runtime evidence boundary tests; fixtures do not claim live Gateway proof."""
import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import nsh_panel_launcher as launcher


class RuntimeBoundaryTests(unittest.TestCase):
    def fixture(self):
        return json.loads((Path(__file__).parent / "fixtures/nsh-panel-parity/runtime-valid.json").read_text())

    def test_native_close_requires_owned_marker_and_actual_process_exit(self):
        import subprocess
        import sys
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "stdout.log"
            event = {"kind": "closed", "nonce": "owned", "qa_run_id": "run", "payload": {"owned_drained": True}}
            capture.write_text("ALEPH_PANEL_QA " + json.dumps(event) + "\n")
            process = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(.15)"])
            self.assertFalse(launcher.native_capture_closed(capture, process.pid, "owned", "run"))
            process.wait(timeout=5)
            self.assertFalse(launcher.native_capture_closed(capture, process.pid, "foreign", "run"))
            self.assertFalse(launcher.native_capture_closed(capture, process.pid, "owned", "foreign"))
            self.assertFalse(launcher.native_capture_closed(capture, -1, "owned", "run"))
            self.assertTrue(launcher.native_capture_closed(capture, process.pid, "owned", "run"))
            event["payload"]["owned_drained"] = False
            capture.write_text("ALEPH_PANEL_QA " + json.dumps(event) + "\n")
            self.assertFalse(launcher.native_capture_closed(capture, process.pid, "owned", "run"))

    def test_terminal_resolution_waits_only_within_sample_coverage(self):
        import subprocess
        import sys
        import threading
        import time
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "stdout.log"
            capture.write_text("ALEPH_PANEL_QA " + json.dumps({"kind": "closed", "nonce": "owned", "qa_run_id": "run", "payload": {"owned_drained": True}}) + "\n")
            process = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(.04)"])
            waiter = threading.Thread(target=process.wait)
            waiter.start()
            try:
                self.assertTrue(launcher.await_native_capture_closed(capture, process.pid, "owned", "run", time.monotonic() + .175))
                self.assertFalse(launcher.await_native_capture_closed(capture, process.pid, "foreign", "run", time.monotonic() + .01))
                self.assertFalse(launcher.await_native_capture_closed(capture, process.pid, "owned", "run", time.monotonic() - 1))
            finally:
                waiter.join(timeout=5)

    def test_departed_host_is_distinct_from_missing_coalition_metadata(self):
        listing = ('1) "Aleph" ASN:1\n    bundleID="ai.aleph.desktop"\n'
                   '    pid = 79442\n    coalition: 13769 { 79524 79525 79526 79529 }\n')
        with self.assertRaisesRegex(ValueError, "host_left_coalition:79442"):
            launcher.parse_launchservices_coalitions(listing, {79442}, require_webkit=False)
        with self.assertRaisesRegex(ValueError, "incomplete_host_coalition:79442"):
            launcher.parse_launchservices_coalitions(listing.replace("coalition: 13769 { 79524 79525 79526 79529 }", ""), {79442}, require_webkit=False)

    def test_bare_coalition_is_a_single_process_before_webkit_launch(self):
        listing = '1) "Owned" ASN:1\n    bundleID="org.aleph.qa"\n    pid = 101\n    coalition: 51\n'
        self.assertEqual(launcher.parse_launchservices_coalitions(listing, {101}, require_webkit=False)[101]["pids"], (101,))
        with self.assertRaisesRegex(ValueError, "incomplete_webkit_process_set"):
            launcher.parse_launchservices_coalitions(listing, {101})

    def test_metrics_keep_resource_peaks_and_native_event_durations(self):
        document = self.fixture()
        document["resources"][1]["rss_mb"] = 999
        document["resources"][1]["threads"] = 99
        for index, event in enumerate(document["events"]):
            event["native_at_ms"] = index * 17.0
        result = launcher.evaluate_runtime(document, require_live=False)
        events = document["events"]
        start = next(e for e in events if e["kind"] == "close-requested")
        end = next(e for e in events if e["kind"] == "closed")
        self.assertEqual(result["metrics"]["rss_mb"], 999)
        self.assertEqual(result["metrics"]["threads"], 99)
        self.assertEqual(result["metrics"]["close_ms"], end["native_at_ms"] - start["native_at_ms"])
        end["native_at_ms"] = -1
        with self.assertRaises(ValueError):
            launcher.evaluate_runtime(document, require_live=False)

    def test_browser_metric_durations_ignore_native_delivery_queue_delay(self):
        document = self.fixture()
        for index, event in enumerate(document["events"]):
            event["browser_at_ms"] = event["at_ms"]
            event["browser_time_origin_ms"] = 1000000
            event["native_at_ms"] = index * 17.0
        first = launcher.evaluate_runtime(document, require_live=False)["metrics"]
        for index, event in enumerate(document["events"]):
            event["native_at_ms"] = index * index * 17.0
        second = launcher.evaluate_runtime(document, require_live=False)["metrics"]
        for metric in ("route_ms", "input_ms", "reconnect_ms"):
            self.assertEqual(first[metric], second[metric])
        self.assertEqual(first["input_ms"], 250)
        next(event for event in document["events"] if event["kind"] == "route-ready")["browser_time_origin_ms"] += 1
        with self.assertRaisesRegex(ValueError, "invalid_browser_event_clock"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_input_metric_uses_addressed_receipt_independently_of_acceptance(self):
        document = self.fixture()
        document["events"] = [event for event in document["events"] if event["kind"] != "send-received"]
        index = next(i for i, event in enumerate(document["events"]) if event["kind"] == "accepted")
        receipt = copy.deepcopy(document["events"][index])
        receipt.update(kind="send-received", at_ms=700, payload={"request_id": 7, "request_session": receipt["session_key"]})
        document["events"].insert(index, receipt)
        self.assertEqual(launcher.evaluate_runtime(document, require_live=False)["metrics"]["input_ms"], 100)
        document["events"].remove(receipt)
        receipt["at_ms"] = 850
        document["events"].insert(index + 1, receipt)
        self.assertEqual(launcher.evaluate_runtime(document, require_live=False)["metrics"]["input_ms"], 250)
        receipt["payload"]["request_session"] = None
        self.assertEqual(launcher.evaluate_runtime(document, require_live=False)["metrics"]["input_ms"], 250)
        receipt["payload"]["request_session"] = "foreign"
        with self.assertRaisesRegex(ValueError, "send_receipt_identity"):
            launcher.evaluate_runtime(document, require_live=False)
        receipt["payload"]["request_session"] = None
        receipt["run_id"] = "foreign"
        with self.assertRaisesRegex(ValueError, "send_receipt_identity"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_queued_run_must_reach_acceptance_with_same_identity(self):
        document = self.fixture()
        index = next(i for i, event in enumerate(document["events"]) if event["kind"] == "accepted")
        queued = copy.deepcopy(document["events"][index])
        queued["kind"] = "queued"
        document["events"].insert(index, queued)
        self.assertTrue(launcher.evaluate_runtime(document, require_live=False)["queued_transition_observed"])
        queued["run_id"] = "foreign"
        with self.assertRaisesRegex(ValueError, "queued_acceptance_identity_changed"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_real_reducer_retains_valid_threshold_failure(self):
        baseline = launcher.evaluate_runtime(self.fixture(), require_live=False)
        candidate = copy.deepcopy(baseline)
        candidate["metrics"]["first_frame_ms"] *= 2
        with tempfile.TemporaryDirectory() as directory:
            result = launcher.export_parity([baseline] * 40, [candidate] * 40, Path(directory), "protocol-only", allow_protocol_fixture=True)
        self.assertEqual(result["exit_code"], 3)
        self.assertEqual(result["result"]["status"], "VALID")
        self.assertEqual(result["result"]["threshold_status"], "NOT_ACCEPTED")

    def test_fixture_never_claims_live_runtime_pass(self):
        result = launcher.evaluate_runtime(self.fixture())
        self.assertEqual(result["status"], "BLOCKED")
        self.assertEqual(launcher.evaluate_runtime(self.fixture(), require_live=False)["status"], "PROTOCOL_VALID")

    def test_stream_cannot_change_the_accepted_run(self):
        document = self.fixture()
        next(e for e in document["events"] if e["kind"] == "streamed")["run_id"] = "other-run"
        with self.assertRaisesRegex(ValueError, "run"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_artifacts_cannot_change_the_session(self):
        document = self.fixture()
        next(e for e in document["events"] if e["kind"] == "artifact-refreshed")["session_key"] = "other-session"
        with self.assertRaisesRegex(ValueError, "session"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_old_epoch_cannot_be_applied(self):
        document = self.fixture()
        next(e for e in document["events"] if e["kind"] == "streamed")["connection_epoch"] = 0
        with self.assertRaisesRegex(ValueError, "epoch"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_reconnect_requires_a_new_hello_and_fresh_snapshots(self):
        document = self.fixture()
        document["events"] = [e for e in document["events"] if e["kind"] != "snapshot"]
        with self.assertRaisesRegex(ValueError, "snapshot"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_rejected_event_must_leave_projection_unchanged(self):
        document = self.fixture()
        next(e for e in document["events"] if e["kind"] == "rejected")["payload"]["after"] = "changed"
        with self.assertRaisesRegex(ValueError, "projection"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_forced_exit_is_a_failure(self):
        document = self.fixture()
        document["direct_child"]["forced"] = True
        with self.assertRaisesRegex(ValueError, "child"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_event_metrics_reach_the_existing_reducer(self):
        result = launcher.evaluate_runtime(self.fixture(), require_live=False)
        metrics = result["metrics"]
        self.assertEqual(metrics["first_frame_ms"], 100)
        self.assertEqual(metrics["input_ms"], 250)
        self.assertEqual(metrics["reconnect_ms"], 800)
        self.assertEqual(metrics["close_ms"], 100)
        self.assertEqual(set(metrics), {"first_frame_ms", "route_ms", "input_ms", "reconnect_ms", "close_ms", "rss_mb", "threads"})
        baseline = launcher.metric_document([result] * 40, allow_protocol_fixture=True)
        candidate = copy.deepcopy(baseline)
        candidate["metrics"]["first_frame_ms"] = [1000] * 40
        import nsh_panel_parity
        _, failures = nsh_panel_parity.evaluate(nsh_panel_parity.samples(baseline, "baseline"), nsh_panel_parity.samples(candidate, "candidate"))
        self.assertIn("first_frame_ms:median", failures)

    def test_launchservices_coalitions_map_webkit_to_unique_instances(self):
        listing = '''
1) "NSH Probe" ASN:0x0-0x1
    bundleID="org.aleph.nsh-probe"
    pid = 101
    coalition: 11  { 101 110 111 112 }
2) "NSH Probe Graphics and Media" ASN:0x0-0x2
    bundleID="com.apple.WebKit.GPU"
    pid = 110
3) "NSH Probe Networking" ASN:0x0-0x3
    bundleID="com.apple.WebKit.Networking"
    pid = 111
4) "NSH Probe Web Content" ASN:0x0-0x4
    bundleID="com.apple.WebKit.WebContent"
    pid = 112
5) "NSH Probe" ASN:0x0-0x5
    bundleID="org.aleph.nsh-probe"
    pid = 201
    coalition: 12  { 201 210 211 212 }
6) "NSH Probe Graphics and Media" ASN:0x0-0x6
    bundleID="com.apple.WebKit.GPU"
    pid = 210
7) "NSH Probe Networking" ASN:0x0-0x7
    bundleID="com.apple.WebKit.Networking"
    pid = 211
8) "NSH Probe Web Content" ASN:0x0-0x8
    bundleID="com.apple.WebKit.WebContent"
    pid = 212
'''
        owners = launcher.parse_launchservices_coalitions(listing, {101, 201})
        self.assertEqual(owners[101], {
            "coalition_id": 11,
            "owner_name": "NSH Probe",
            "pids": (101, 110, 111, 112),
            "webkit": {"com.apple.WebKit.GPU": (110,), "com.apple.WebKit.Networking": (111,), "com.apple.WebKit.WebContent": (112,)},
        })
        self.assertEqual(owners[201]["pids"], (201, 210, 211, 212))

    def test_launchservices_coalition_rejects_overlapping_instance_members(self):
        listing = '''
1) "NSH Probe" ASN:0x0-0x1
    bundleID="org.aleph.nsh-probe"
    pid = 101
    coalition: 11  { 101 110 111 112 }
2) "NSH Probe Graphics and Media" ASN:0x0-0x2
    bundleID="com.apple.WebKit.GPU"
    pid = 110
3) "NSH Probe Networking" ASN:0x0-0x3
    bundleID="com.apple.WebKit.Networking"
    pid = 111
4) "NSH Probe Web Content" ASN:0x0-0x4
    bundleID="com.apple.WebKit.WebContent"
    pid = 112
5) "NSH Probe" ASN:0x0-0x5
    bundleID="org.aleph.nsh-probe"
    pid = 201
    coalition: 12  { 201 110 211 212 }
6) "NSH Probe Networking" ASN:0x0-0x6
    bundleID="com.apple.WebKit.Networking"
    pid = 211
7) "NSH Probe Web Content" ASN:0x0-0x7
    bundleID="com.apple.WebKit.WebContent"
    pid = 212
'''
        with self.assertRaisesRegex(ValueError, "overlapping_coalition_members"):
            launcher.parse_launchservices_coalitions(listing, {101, 201})

    def test_launchservices_coalition_rejects_an_unattributed_webkit_process(self):
        listing = '''
1) "NSH Probe" ASN:0x0-0x1
    bundleID="org.aleph.nsh-probe"
    pid = 101
    coalition: 11  { 101 110 111 }
2) "NSH Probe Graphics and Media" ASN:0x0-0x2
    bundleID="com.apple.WebKit.GPU"
    pid = 110
3) "NSH Probe Networking" ASN:0x0-0x3
    bundleID="com.apple.WebKit.Networking"
    pid = 111
4) "NSH Probe Web Content" ASN:0x0-0x4
    bundleID="com.apple.WebKit.WebContent"
    pid = 112
'''
        with self.assertRaisesRegex(ValueError, "webkit_process_not_in_unique_coalition"):
            launcher.parse_launchservices_coalitions(listing, {101})

    def test_collector_binds_markers_to_its_nonce_and_reaps_its_child(self):
        import sys
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / "producer.py"
            script.write_text("import json,time\nprint('ALEPH_PANEL_QA '+json.dumps({'nonce':'foreign','qa_run_id':'run','kind':'ready'}),flush=True)\nprint('ALEPH_PANEL_QA '+json.dumps({'nonce':'owned','qa_run_id':'run','kind':'ready','session_key':None,'run_id':None,'connection_epoch':0,'payload':{}}),flush=True)\ntime.sleep(0.25)\n")
            result = launcher.collect_process([sys.executable, str(script)], root, {}, "run", "owned", "nsh", None, root / "capture", 3)
            self.assertEqual(result["direct_child"], {"exit_code": 0, "forced": False})
            self.assertEqual([e["kind"] for e in result["events"]], ["ready"])
            self.assertEqual(result["origin"], "protocol_fixture")
            self.assertFalse(result["resource_scope"]["complete"])
            self.assertGreaterEqual(len(result["resources"]), 2)

    def test_saved_events_must_match_the_owned_raw_markers(self):
        import sys
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / "producer.py"
            script.write_text("import json,time\nprint('ALEPH_PANEL_QA '+json.dumps({'nonce':'owned','qa_run_id':'run','kind':'ready','session_key':None,'run_id':None,'connection_epoch':0,'native_clock_origin_ns':1000000000,'native_clock_ns':1000000000,'native_at_ms':0,'browser_at_ms':0,'browser_time_origin_ms':1000000,'payload':{}}),flush=True)\ntime.sleep(0.2)\n")
            document = launcher.collect_process([sys.executable, str(script)], root, {}, "run", "owned", "nsh",
                                                {"binary": sys.executable, "binary_sha256": launcher.file_sha256(sys.executable)}, root / "capture", 3)
            assets = {}
            for name in ["index.html", "aleph_panel.js", "aleph_panel_bg.wasm", "tailwind.css"]:
                path = root / name
                path.write_text("owned HTTP bytes: " + name)
                assets[name] = {"capture": str(path), "sha256": launcher.file_sha256(path), "bytes": path.stat().st_size}
            document["gateway"]["panel_assets"] = assets
            listing = '''
1) "Aleph NSH Phase9 Probe" ASN:0x0-0x1
    bundleID="org.aleph.phase9.nsh.probe"
    pid = 101
    coalition: 51 { 101 110 111 112 }
2) "Aleph NSH Phase9 Probe Graphics and Media" ASN:0x0-0x2
    bundleID="com.apple.WebKit.GPU"
    pid = 110
3) "Aleph NSH Phase9 Probe Networking" ASN:0x0-0x3
    bundleID="com.apple.WebKit.Networking"
    pid = 111
4) "Aleph NSH Phase9 Probe Web Content" ASN:0x0-0x4
    bundleID="com.apple.WebKit.WebContent"
    pid = 112
'''
            document["origin"] = "live_gateway"
            document["capture"]["pid"] = 101
            document["resource_scope"] = {"complete": True, "kind": "launchservices_coalition", "owner_pid": 101,
                                           "coalition_id": 51, "ownership_sample_count": 2, "settled_at_ns": 1_100_000_000, "settled_at_ms": 100}
            clock_file = root / "clock-origin-ns"
            clock_file.write_text("1000000000")
            pid_file = root / "native-pid.json"
            pid_file.write_text(json.dumps({"pid": 101, "nonce": "owned"}))
            resources = [{"at_ms": at_ms, "sample_monotonic_ns": 1_000_000_000 + at_ms * 1_000_000,
                          "owner_pid": 101, "coalition_id": 51, "pids": [101, 110, 111, 112],
                          "rss_mb": 40, "threads": 4,
                          "per_process": [{"pid": pid, "rss_mb": 10, "threads": 1} for pid in [101, 110, 111, 112]]}
                         for at_ms in [0, 100]]
            document["resources"] = resources
            ownership_file = root / "ownership.jsonl"
            ownership_file.write_text("".join(json.dumps({"at_ms": sample["at_ms"],
                                                          "listing_sha256": hashlib.sha256(listing.encode()).hexdigest(),
                                                          "listing": listing, "resource": sample}) + "\n" for sample in resources))
            launcher_stdout = root / "launcher.stdout.jsonl"
            launcher_stdout.write_text(json.dumps({"pid": 101}) + "\n" + json.dumps({"pid": 101, "terminated": True,
                                                                                       "closed_marker_seen": True, "timed_out": False,
                                                                                       "forced": False}) + "\n")
            launcher_stderr = root / "launcher.stderr.log"
            launcher_stderr.write_text("")
            terminal_file = root / "terminal.json"
            terminal_file.write_text(json.dumps({"native_exit": {"pid": 101, "fflags": 0x84000000, "wait_status": 0, "exit_code": 0},
                                                  "tail_ownership": [{"listing": listing,
                                                                      "listing_sha256": hashlib.sha256(listing.encode()).hexdigest(),
                                                                      "observed_at_ns": 1_100_000_000}],
                                                  "tail_ownership": [{"listing": listing,
                                                                       "listing_sha256": hashlib.sha256(listing.encode()).hexdigest(),
                                                                       "observed_at_ns": 1_100_000_000}],
                                                  "settlement": {"members": [101, 110, 111, 112], "survivors": [],
                                                                 "snapshots": [{"at_monotonic_ns": 1_100_000_000, "survivors": []}]}}))
            pid_file = root / "native-pid.json"
            pid_file.write_text(json.dumps({"pid": 101, "nonce": "owned"}))
            document["capture"].update({"pid": 101, "helper_exit_code": 0,
                                         "clock_file": str(clock_file), "clock_sha256": launcher.file_sha256(clock_file),
                                         "pid_file": str(pid_file), "pid_sha256": launcher.file_sha256(pid_file),
                                         "terminal_file": str(terminal_file), "terminal_sha256": launcher.file_sha256(terminal_file),
                                         "ownership_file": str(ownership_file),
                                        "ownership_sha256": launcher.file_sha256(ownership_file),
                                        "launcher_stdout": str(launcher_stdout),
                                        "launcher_stdout_sha256": launcher.file_sha256(launcher_stdout),
                                        "launcher_stderr": str(launcher_stderr),
                                        "launcher_stderr_sha256": launcher.file_sha256(launcher_stderr)})
            self.assertTrue(launcher.verified_capture(document))
            original_terminal = terminal_file.read_text()
            terminal = json.loads(original_terminal)
            unknown_listing = ('1) "Aleph NSH Phase9 Probe Graphics and Media"\n    bundleID="com.apple.WebKit.GPU"\n'
                               '    pid = 113\n')
            terminal["tail_ownership"] = [{"listing": unknown_listing,
                                           "listing_sha256": hashlib.sha256(unknown_listing.encode()).hexdigest(),
                                           "observed_at_ns": 1_100_000_000}]
            terminal_file.write_text(json.dumps(terminal))
            document["capture"]["terminal_sha256"] = launcher.file_sha256(terminal_file)
            self.assertFalse(launcher.verified_capture(document))
            terminal_file.write_text(original_terminal)
            document["capture"]["terminal_sha256"] = launcher.file_sha256(terminal_file)
            terminal = json.loads(original_terminal)
            absent_owner_listing = '1) "Owned XPC" ASN:1\n    pid = 113\n    coalition: 51 { 113 }\n'
            terminal["tail_ownership"] = [{"listing": absent_owner_listing,
                                           "listing_sha256": hashlib.sha256(absent_owner_listing.encode()).hexdigest(),
                                           "observed_at_ns": 1_100_000_000}]
            terminal_file.write_text(json.dumps(terminal))
            document["capture"]["terminal_sha256"] = launcher.file_sha256(terminal_file)
            self.assertFalse(launcher.verified_capture(document))
            terminal = json.loads(original_terminal)
            tail_listing = listing.replace("101 110 111 112", "110 111 112 113")
            terminal["tail_ownership"] = [{"listing": tail_listing,
                                           "listing_sha256": hashlib.sha256(tail_listing.encode()).hexdigest(),
                                           "observed_at_ns": 1_100_000_000}]
            terminal_file.write_text(json.dumps(terminal))
            document["capture"]["terminal_sha256"] = launcher.file_sha256(terminal_file)
            self.assertFalse(launcher.verified_capture(document))
            terminal_file.write_text(original_terminal)
            document["capture"]["terminal_sha256"] = launcher.file_sha256(terminal_file)
            terminal = json.loads(original_terminal)
            tail_listing = '1) "Owned XPC" ASN:1\n    pid = 113\n    coalition: 51 { 113 }\n'
            terminal["tail_ownership"] = [{"listing": tail_listing,
                                           "listing_sha256": hashlib.sha256(tail_listing.encode()).hexdigest(),
                                           "observed_at_ns": 1_100_000_000}]
            terminal_file.write_text(json.dumps(terminal))
            document["capture"]["terminal_sha256"] = launcher.file_sha256(terminal_file)
            self.assertFalse(launcher.verified_capture(document))
            terminal_file.write_text(original_terminal)
            document["capture"]["terminal_sha256"] = launcher.file_sha256(terminal_file)
            document["resources"][0]["rss_mb"] += 1
            self.assertFalse(launcher.verified_capture(document))
            document["resources"][0]["rss_mb"] -= 1
            document["resources"][0]["per_process"][0]["rss_mb"] += 1
            self.assertFalse(launcher.verified_capture(document))
            document["resources"][0]["per_process"][0]["rss_mb"] -= 1
            ownership_file.write_text(ownership_file.read_text().replace("coalition: 51", "coalition: 52"))
            self.assertFalse(launcher.verified_capture(document))
            ownership_file.write_text("".join(json.dumps({"at_ms": sample["at_ms"],
                                                          "listing_sha256": hashlib.sha256(listing.encode()).hexdigest(),
                                                          "listing": listing, "resource": sample}) + "\n" for sample in resources))
            document["capture"]["ownership_sha256"] = launcher.file_sha256(ownership_file)
            document["gateway"]["panel_assets"] = {}
            self.assertFalse(launcher.verified_capture(document))
            document["gateway"]["panel_assets"] = assets
            document["events"][0]["kind"] = "accepted"
            self.assertFalse(launcher.verified_capture(document))
            document["events"] = []
            self.assertFalse(launcher.verified_capture(document))

    def test_old_epoch_projection_alone_does_not_prove_ingress_rejection(self):
        document = self.fixture()
        event = next(e for e in document["events"] if e["kind"] == "rejected" and e["payload"]["reason"] == "old_epoch")
        event["payload"].pop("handler_invoked", None)
        with self.assertRaisesRegex(ValueError, "old_epoch_receiver"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_detached_socket_noop_cannot_replace_actual_ingress(self):
        document = self.fixture()
        event = next(e for e in document["events"] if e["kind"] == "rejected" and e["payload"]["reason"] == "old_epoch")
        event["payload"].update(boundary="disposed_transport", previous_receiver_observed=True, receiver_detached=True, event_dispatched=True)
        with self.assertRaisesRegex(ValueError, "old_epoch_receiver"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_missing_metric_boundaries_fail_closed(self):
        for kind in ("route-requested", "route-ready", "input", "send-received", "close-requested"):
            with self.subTest(kind=kind):
                document = self.fixture()
                document["events"] = [event for event in document["events"] if event["kind"] != kind]
                with self.assertRaisesRegex(ValueError, f"missing_metric_event:{kind}"):
                    launcher.evaluate_runtime(document, require_live=False)

    def test_reconnect_start_requires_the_production_state_commit(self):
        document = self.fixture()
        event = next(event for event in document["events"] if event["kind"] == "disconnected")
        event["payload"] = {}
        with self.assertRaisesRegex(ValueError, "reconnect_start_not_production_state"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_close_latency_waits_for_all_member_settlement(self):
        document = self.fixture()
        document["resource_scope"] = {"complete": False, "kind": "protocol_fixture", "settled_at_ms": 2300, "settled_at_ns": 2300000000}
        result = launcher.evaluate_runtime(document, require_live=False)
        self.assertEqual(result["metrics"]["close_ms"], 200)
        document["resource_scope"]["settled_at_ms"] = 2400
        document["resource_scope"]["settled_at_ns"] = 2400000000
        with self.assertRaisesRegex(ValueError, "resource_sampling_does_not_cover_lifecycle"):
            launcher.evaluate_runtime(document, require_live=False)

    def test_changing_origin_and_claiming_a_gateway_does_not_turn_a_fixture_live(self):
        document = self.fixture()
        document["origin"] = "live_gateway"
        document["gateway"] = {"kind": "production_gateway", "binary_sha256": "a" * 64}
        next(e for e in document["events"] if e["kind"] == "reconnected")["payload"].update(
            rendered=True, applied_epoch=2, projection="claimed render")
        self.assertEqual(launcher.evaluate_runtime(document)["status"], "BLOCKED")

    def test_incomplete_webkit_resource_scope_cannot_reach_performance_acceptance(self):
        result = launcher.evaluate_runtime(self.fixture(), require_live=False)
        with self.assertRaisesRegex(ValueError, "resource"):
            launcher.metric_document([result] * 40)

    def test_launchservices_coalitions_assign_each_webkit_process_to_one_instance(self):
        listing = '''
0) "Aleph NSH Phase9 Probe" ASN:0x0-0x1001
    bundleID="org.aleph.phase9.nsh-probe"
    pid = 101
    coalition: 51  { 101 102 110 111 112 }
1) "Aleph NSH Phase9 Probe Graphics and Media" ASN:0x0-0x1002
    bundleID="com.apple.WebKit.GPU"
    pid = 110
2) "Aleph NSH Phase9 Probe Networking" ASN:0x0-0x1003
    bundleID="com.apple.WebKit.Networking"
    pid = 111
3) "Aleph NSH Phase9 Probe Web Content" ASN:0x0-0x1004
    bundleID="com.apple.WebKit.WebContent"
    pid = 112
4) "Aleph NSH Phase9 Probe" ASN:0x0-0x2001
    bundleID="org.aleph.phase9.nsh-probe"
    pid = 201
    coalition: 52  { 201 202 210 211 212 }
5) "Aleph NSH Phase9 Probe Graphics and Media" ASN:0x0-0x2002
    bundleID="com.apple.WebKit.GPU"
    pid = 210
6) "Aleph NSH Phase9 Probe Networking" ASN:0x0-0x2003
    bundleID="com.apple.WebKit.Networking"
    pid = 211
7) "Aleph NSH Phase9 Probe Web Content" ASN:0x0-0x2004
    bundleID="com.apple.WebKit.WebContent"
    pid = 212
'''
        instances = launcher.parse_launchservices_coalitions(listing, {101, 201})
        self.assertEqual(instances[101]["coalition_id"], 51)
        self.assertEqual(instances[101]["pids"], (101, 102, 110, 111, 112))
        self.assertEqual(instances[101]["webkit"], {
            "com.apple.WebKit.GPU": (110,),
            "com.apple.WebKit.Networking": (111,),
            "com.apple.WebKit.WebContent": (112,),
        })
        self.assertEqual(instances[201]["coalition_id"], 52)
        self.assertEqual(instances[201]["webkit"]["com.apple.WebKit.WebContent"], (212,))

    def test_launchservices_coalitions_reject_cross_instance_pid_overlap(self):
        listing = '''
0) "Aleph NSH Phase9 Probe" ASN:0x0-0x1001
    bundleID="org.aleph.phase9.nsh-probe"
    pid = 101
    coalition: 51  { 101 110 111 112 }
1) "Aleph NSH Phase9 Probe" ASN:0x0-0x2001
    bundleID="org.aleph.phase9.nsh-probe"
    pid = 201
    coalition: 52  { 201 110 211 212 }
'''
        with self.assertRaisesRegex(ValueError, "overlap"):
            launcher.parse_launchservices_coalitions(listing, {101, 201})

    def test_launchservices_coalitions_reject_unattributed_webkit_process(self):
        listing = '''
0) "Aleph NSH Phase9 Probe" ASN:0x0-0x1001
    bundleID="org.aleph.phase9.nsh-probe"
    pid = 101
    coalition: 51  { 101 110 111 112 }
1) "Aleph NSH Phase9 Probe Graphics and Media" ASN:0x0-0x1002
    bundleID="com.apple.WebKit.GPU"
    pid = 110
2) "Aleph NSH Phase9 Probe Networking" ASN:0x0-0x1003
    bundleID="com.apple.WebKit.Networking"
    pid = 111
3) "Aleph NSH Phase9 Probe Web Content" ASN:0x0-0x1004
    bundleID="com.apple.WebKit.WebContent"
    pid = 112
4) "Aleph NSH Phase9 Probe Web Content" ASN:0x0-0x3001
    bundleID="com.apple.WebKit.WebContent"
    pid = 312
'''
        with self.assertRaisesRegex(ValueError, "unowned WebKit"):
            launcher.parse_launchservices_coalitions(listing, {101})

    def test_nan_and_sparse_sampling_are_rejected(self):
        for replacement in [float("nan"), 500]:
            document = self.fixture()
            document["resources"][1]["at_ms"] = replacement
            with self.assertRaises(ValueError):
                launcher.evaluate_runtime(document, require_live=False)


    def test_launchservices_member_sampler_counts_all_owned_threads_and_rss(self):
        import os
        sample = launcher.sample_launchservices_members([os.getpid()])
        self.assertEqual(sample["pids"], (os.getpid(),))
        self.assertGreater(sample["rss_mb"], 0)
        self.assertGreaterEqual(sample["threads"], 1)

    def test_launchservices_member_sampler_fails_closed_on_missing_pid(self):
        with self.assertRaisesRegex(ValueError, "missing_processes"):
            launcher.sample_launchservices_members([2147483647])

    def test_launchservices_sample_sequence_rejects_coalition_drift(self):
        expected = {"coalition_id": 51, "pids": (101, 102, 110, 111, 112)}
        samples = [
            {"at_ms": 0, "owner_pid": 101, "coalition_id": 51, "pids": expected["pids"], "rss_mb": 80, "threads": 12, "per_process": tuple({"pid": pid, "rss_mb": 10, "threads": 2} for pid in expected["pids"])},
            {"at_ms": 100, "owner_pid": 101, "coalition_id": 52, "pids": expected["pids"], "rss_mb": 75, "threads": 11, "per_process": tuple({"pid": pid, "rss_mb": 10, "threads": 2} for pid in expected["pids"])},
        ]
        with self.assertRaisesRegex(ValueError, "ownership_changed"):
            launcher.validate_launchservices_samples(samples, 101, expected)

    def test_launchservices_sample_sequence_requires_complete_100ms_coverage(self):
        expected = {"coalition_id": 51, "pids": (101, 102, 110, 111, 112)}
        samples = [
            {"at_ms": 0, "owner_pid": 101, "coalition_id": 51, "pids": expected["pids"], "rss_mb": 80, "threads": 12, "per_process": tuple({"pid": pid, "rss_mb": 10, "threads": 2} for pid in expected["pids"])},
            {"at_ms": 176, "owner_pid": 101, "coalition_id": 51, "pids": expected["pids"], "rss_mb": 75, "threads": 11, "per_process": tuple({"pid": pid, "rss_mb": 10, "threads": 2} for pid in expected["pids"])},
        ]
        with self.assertRaisesRegex(ValueError, "sampling_not_100ms"):
            launcher.validate_launchservices_samples(samples, 101, expected)

    def test_launchservices_sample_sequence_accepts_stable_complete_coverage(self):
        expected = {"coalition_id": 51, "pids": (101, 102, 110, 111, 112)}
        samples = [
            {"at_ms": 10, "owner_pid": 101, "coalition_id": 51, "pids": expected["pids"], "rss_mb": 80, "threads": 12, "per_process": tuple({"pid": pid, "rss_mb": 10, "threads": 2} for pid in expected["pids"])},
            {"at_ms": 110, "owner_pid": 101, "coalition_id": 51, "pids": expected["pids"], "rss_mb": 75, "threads": 11, "per_process": tuple({"pid": pid, "rss_mb": 10, "threads": 2} for pid in expected["pids"])},
        ]
        self.assertEqual(launcher.validate_launchservices_samples(samples, 101, expected), samples)

    def test_closed_marker_requires_matching_identity_and_actual_process_exit(self):
        import subprocess
        import sys
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "markers.log"
            event = {"kind": "closed", "nonce": "owned", "qa_run_id": "run", "payload": {"owned_drained": True}}
            capture.write_text("ALEPH_PANEL_QA " + json.dumps(event) + "\n")
            child = subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.read()"], stdin=subprocess.PIPE)
            try:
                self.assertFalse(launcher.native_capture_closed(capture, child.pid, "owned", "run"))
                child.communicate(timeout=5)
                self.assertFalse(launcher.native_capture_closed(capture, child.pid, "foreign", "run"))
                self.assertFalse(launcher.native_capture_closed(capture, child.pid, "owned", "other"))
                self.assertTrue(launcher.native_capture_closed(capture, child.pid, "owned", "run"))
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()

    def test_launchservices_member_sampler_rejects_duplicate_pid(self):
        import os
        with self.assertRaisesRegex(ValueError, "duplicate_process_ids"):
            launcher.sample_launchservices_members([os.getpid(), os.getpid()])


if __name__ == "__main__":
    unittest.main()
