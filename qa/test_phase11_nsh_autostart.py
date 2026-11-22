import copy
import unittest

from phase11_nsh_autostart import evaluate


def observations():
    return [
        {"case": "unbundled", "returncode": 0, "stderr_empty": True,
         "value": {"status": "unavailable", "reason": "not_app_bundle"}},
        {"case": "isolated_first", "returncode": 0, "stderr_empty": True,
         "value": {"status": "disabled"}},
        {"case": "wrong_identity", "returncode": 0, "stderr_empty": True,
         "value": {"status": "unavailable", "reason": "identity_mismatch"}},
        {"case": "isolated_second", "returncode": 0, "stderr_empty": True,
         "value": {"status": "disabled"}},
    ]


class AutostartReceiptTests(unittest.TestCase):
    def test_real_status_and_refusal_controls_are_required(self):
        self.assertTrue(evaluate(observations())["accepted"])
        values = observations()
        for index in [1, 3]:
            values[index]["value"] = {"status": "unavailable", "reason": "not_found"}
        self.assertTrue(evaluate(values)["accepted"])

    def test_unavailable_is_not_a_blanket_success(self):
        for value in [{"status": "unavailable", "reason": "not_app_bundle"},
                      {"status": "unavailable", "reason": "api_unavailable"},
                      {"status": "enabled"}, {"status": "requires_approval"},
                      {"status": "failed"}, False, {"status": "disabled", "extra": True}]:
            values = observations()
            values[1]["value"] = value
            self.assertFalse(evaluate(values)["accepted"])

    def test_missing_reordered_duplicated_controls_fail(self):
        values = observations()
        for bad in [values[:-1], values[::-1], values + [values[-1]], None, [None]]:
            self.assertFalse(evaluate(bad)["accepted"])

    def test_changed_state_bad_exit_or_stderr_fails(self):
        for key, value in [("returncode", 1), ("returncode", False), ("stderr_empty", 1),
                           ("stderr_empty", False), ("value", {"status": "unavailable", "reason": "not_found"})]:
            values = copy.deepcopy(observations())
            values[-1][key] = value
            self.assertFalse(evaluate(values)["accepted"])

    def test_wrong_identity_and_unbundled_never_count_as_disabled(self):
        for index in [0, 2]:
            values = observations()
            values[index]["value"] = {"status": "disabled"}
            self.assertFalse(evaluate(values)["accepted"])


if __name__ == "__main__":
    unittest.main()
