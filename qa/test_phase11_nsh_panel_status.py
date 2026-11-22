import copy
import unittest

from pathlib import Path

from phase11_nsh_panel_status import evaluate, gateway_sample, native_environment


class PanelStatusAcceptance(unittest.TestCase):
    def valid(self):
        return [
            {"kind": "panel-status-fixture", "document": "ab" * 16,
             "route": "/settings/general",
             "native_status": {"status": "unavailable", "reason": "not_app_bundle"},
             "effects": {"native_marker": True, "credential_load_empty": True,
                         "connect_authorized_without_credential": True, "component_visible": True,
                         "status_text_correct": True, "readonly_text_correct": True, "toggle_absent": True,
                         "document_bound": True}},
            {"kind": "close-requested"},
            {"kind": "closed", "payload": {"owned_drained": True}},
        ], {"status": "READY", "panel_assets_unchanged": True, "fixture_removed": True,
            "children": [{"name": "model", "exit_code": -15, "forced": False},
                         {"name": "gateway", "exit_code": 0, "forced": False}]}

    def test_native_environment_preserves_the_os_login_context(self):
        base = {"HOME": "/synthetic/login", "ALEPH_HOME": "/must-not-use",
                "ALEPH_PANEL_QA_START_GATE": "/must-not-use-gate"}
        env = native_environment(base, Path("/owned/profile"), "http://127.0.0.1:29999/",
                                 "ab" * 32, "cd" * 16, [])
        self.assertEqual(env["HOME"], "/synthetic/login")
        self.assertEqual(env["ALEPH_HOME"], "/owned/profile/aleph")
        self.assertEqual(env["ALEPH_NSH_QA_WINDOW_DATA_DIR"], "/owned/profile")
        self.assertNotIn("ALEPH_PANEL_QA_START_GATE", env)
        self.assertEqual(env["ALEPH_SHELL_HOTKEY"], "disabled")
        self.assertEqual(base["ALEPH_HOME"], "/must-not-use")

    def test_actual_effects_native_return_and_owned_cleanup_are_required(self):
        events, cleanup = self.valid()
        self.assertTrue(evaluate(events, 0, cleanup)["accepted"])

    def test_quit_requires_the_same_ready_gateway_before_and_after(self):
        events, cleanup = self.valid()
        cleanup["children"][1]["pid"] = 42
        sample = {"listener_pids": [42], "instance_id": "the-instance", "started_at_unix": 17,
                  "version": "2026.11.22", "ready": True}
        self.assertTrue(evaluate(events, 0, cleanup, gateway_survival=[sample, dict(sample)])["accepted"])
        for key, value in [("listener_pids", [43]), ("listener_pids", [42, 43]),
                           ("listener_pids", [True]), ("instance_id", "replacement"),
                           ("started_at_unix", 18), ("version", "changed"), ("ready", 1)]:
            for index in [0, 1]:
                samples = [dict(sample), dict(sample)]
                samples[index][key] = value
                self.assertFalse(evaluate(events, 0, cleanup, gateway_survival=samples)["accepted"], (index, key, value))
        for samples in [[], [sample], {}, 42, [None, sample], [sample, sample, sample]]:
            self.assertFalse(evaluate(events, 0, cleanup, gateway_survival=samples)["accepted"])

    def test_survival_probe_refuses_the_default_port_and_external_origins(self):
        for url in ["http://127.0.0.1:18790/", "http://192.0.2.1:20001/",
                    "http://qa@127.0.0.1:20001/", "https://127.0.0.1:20001/"]:
            with self.subTest(url=url), self.assertRaises(ValueError):
                gateway_sample(url)

    def test_every_effect_is_a_strict_boolean(self):
        events, cleanup = self.valid()
        for key in events[0]["effects"]:
            for value in [False, 1, "true", None]:
                bad = copy.deepcopy(events)
                bad[0]["effects"][key] = value
                self.assertFalse(evaluate(bad, 0, cleanup)["accepted"], (key, value))
        bad = copy.deepcopy(events)
        del bad[0]["effects"]["credential_load_empty"]
        self.assertFalse(evaluate(bad, 0, cleanup)["accepted"])

    def test_unknown_disabled_failed_or_wrong_document_is_not_the_observed_status(self):
        events, cleanup = self.valid()
        for key, value in [("document", "wrong"), ("route", "/settings"),
                           ("native_status", False), ("native_status", {"status": "disabled"}),
                           ("native_status", {"status": "failed"}),
                           ("native_status", {"status": "unavailable", "reason": "not_found"})]:
            bad = copy.deepcopy(events)
            bad[0][key] = value
            self.assertFalse(evaluate(bad, 0, cleanup)["accepted"])

    def test_duplicate_unordered_or_missing_native_terminal_fails(self):
        events, cleanup = self.valid()
        for bad in [events[:2], events[1:], events + [events[0]], list(reversed(events)), [None]]:
            self.assertFalse(evaluate(bad, 0, cleanup)["accepted"])
        self.assertFalse(evaluate(events, 1, cleanup)["accepted"])
        self.assertFalse(evaluate(events, False, cleanup)["accepted"])
        events[-1]["payload"]["owned_drained"] = 1
        self.assertFalse(evaluate(events, 0, cleanup)["accepted"])

    def test_changed_assets_forced_children_and_unremoved_fixture_fail(self):
        events, cleanup = self.valid()
        for key in ["panel_assets_unchanged", "fixture_removed"]:
            bad = copy.deepcopy(cleanup)
            bad[key] = 1
            self.assertFalse(evaluate(events, 0, bad)["accepted"])
        for child in [0, 1]:
            for key, value in [("forced", True), ("forced", 0), ("exit_code", None), ("exit_code", -9)]:
                bad = copy.deepcopy(cleanup)
                bad["children"][child][key] = value
                self.assertFalse(evaluate(events, 0, bad)["accepted"])
        cleanup["children"] = []
        self.assertFalse(evaluate(events, 0, cleanup)["accepted"])


if __name__ == "__main__":
    unittest.main()
