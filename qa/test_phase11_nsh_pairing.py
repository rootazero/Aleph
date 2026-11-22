import copy
import unittest
from phase11_nsh_pairing import evaluate, helper_stopped


def complete():
    return [{"kind": "protected-panel-fixture", "documents": ["a" * 32, "b" * 32],
             "user": "alice", "effects": {key: True for key in ["initial_empty", "ticket_submitted",
             "pair_authorized", "save_ack", "reload_stored", "reload_protected", "no_reload_ticket",
             "cleanup_deleted", "cleanup_empty"]}}, {"kind": "close-requested"},
            {"kind": "closed", "payload": {"owned_drained": True}}]


class ProtectedPanelTests(unittest.TestCase):
    def helper(self):
        return {"confirmed": True, "listener_closed": True, "returncode": 0,
                "forced": False, "root_removed": True, "assets_unchanged": True}

    def test_actual_protected_reload_and_both_owners_must_finish(self):
        self.assertTrue(evaluate(complete(), 0, self.helper())["accepted"])

    def test_missing_false_and_nonboolean_effects_fail(self):
        for key in complete()[0]["effects"]:
            for value in [False, 1, "true", None]:
                events = complete(); events[0]["effects"][key] = value
                self.assertFalse(evaluate(events, 0, self.helper())["accepted"])
            events = complete(); del events[0]["effects"][key]
            self.assertFalse(evaluate(events, 0, self.helper())["accepted"])

    def test_reload_requires_distinct_real_documents_and_gateway_user(self):
        for documents in [["a" * 32], ["a" * 32, "a" * 32], ["bad", "b" * 32]]:
            events = complete(); events[0]["documents"] = documents
            self.assertFalse(evaluate(events, 0, self.helper())["accepted"])
        for user in ["", "bob", None]:
            events = complete(); events[0]["user"] = user
            self.assertFalse(evaluate(events, 0, self.helper())["accepted"])

    def test_order_native_return_and_cleanup_are_not_optional(self):
        for events in [complete()[:-1], complete()[1:], complete() + [complete()[0]], list(reversed(complete()))]:
            self.assertFalse(evaluate(events, 0, self.helper())["accepted"])
        for rc in [1, None, False]:
            self.assertFalse(evaluate(complete(), rc, self.helper())["accepted"])
        for key in self.helper():
            helper = self.helper(); helper[key] = 1 if key != "returncode" else False
            self.assertFalse(evaluate(complete(), 0, helper)["accepted"])
        events = complete(); events[-1]["payload"]["owned_drained"] = False
        self.assertFalse(evaluate(events, 0, self.helper())["accepted"])

    def test_shutdown_uses_the_real_helper_wire_fields(self):
        self.assertTrue(helper_stopped({"type": "stopped", "listenerClosed": True, "fixtureClosed": True}))
        for value in [None, {}, {"type": "stopped", "listener_closed": True, "fixture_closed": True},
                      {"type": "stopped", "listenerClosed": "true", "fixtureClosed": True},
                      {"type": "stopped", "listenerClosed": True, "fixtureClosed": False}]:
            self.assertFalse(helper_stopped(value))

    def test_malformed_envelopes_fail_closed(self):
        for events in [None, {}, [None], [1]]:
            self.assertFalse(evaluate(events, 0, self.helper())["accepted"])
        self.assertFalse(evaluate(complete(), 0, None)["accepted"])


if __name__ == "__main__":
    unittest.main()
