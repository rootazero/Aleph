import copy
import unittest

from pathlib import Path

from phase11_nsh_panel_status import evaluate, native_environment


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
