"""Focused receipt checks; these do not launch a native host."""
import copy
import unittest

from phase11_nsh_credentials import STEPS, evaluate


class ReceiptTests(unittest.TestCase):
    def candidate(self):
        return [
            {"kind": "credential-fixture", "documents": ["a" * 32, "b" * 32],
             "steps": [{"name": name, "ok": True} for name in STEPS], "cleanup": True},
            {"kind": "close-requested"},
            {"kind": "closed", "payload": {"owned_drained": True}},
        ]

    def test_complete_effects_and_natural_exit_pass(self):
        self.assertTrue(evaluate(self.candidate(), 0)["accepted"])

    def test_missing_effect_or_duplicate_effect_fails(self):
        for change in [lambda steps: steps.pop(), lambda steps: steps.append(steps[0])]:
            events = self.candidate()
            change(events[0]["steps"])
            self.assertFalse(evaluate(events, 0)["accepted"])

    def test_false_string_or_false_boolean_fails(self):
        for value in [False, "true", 1, None]:
            events = self.candidate()
            events[0]["steps"][2]["ok"] = value
            self.assertFalse(evaluate(events, 0)["accepted"])

    def test_one_document_or_duplicate_result_fails(self):
        events = self.candidate()
        events[0]["documents"][1] = events[0]["documents"][0]
        self.assertFalse(evaluate(events, 0)["accepted"])
        events = self.candidate()
        events.insert(1, copy.deepcopy(events[0]))
        self.assertFalse(evaluate(events, 0)["accepted"])

    def test_no_exit_ack_failure_exit_or_cleanup_failure_fails(self):
        events = self.candidate()
        self.assertFalse(evaluate(events[:-1], 0)["accepted"])
        self.assertFalse(evaluate(events, 1)["accepted"])
        events[0]["cleanup"] = False
        self.assertFalse(evaluate(events, 0)["accepted"])

    def test_reordered_or_malformed_observations_fail(self):
        events = self.candidate()
        events[0]["steps"].reverse()
        self.assertFalse(evaluate(events, 0)["accepted"])
        for malformed in [None, {}, [None], [{"kind": "credential-fixture"}]]:
            self.assertFalse(evaluate(malformed, 0)["accepted"])


if __name__ == "__main__":
    unittest.main()
