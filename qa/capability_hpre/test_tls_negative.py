import json
import unittest

import tls_negative as t


def denial(text):
    return {"error": {"code": -32000, "message": text}}


class LanIpTests(unittest.TestCase):
    def test_rejects_wildcard_loopback_ipv6_and_garbage(self):
        for bad in ("0.0.0.0", "127.0.0.1", "::1", "nope", "::"):
            self.assertIsNotNone(t.lan_ip_problem(bad), bad)

    def test_requires_assignment_when_known(self):
        self.assertIsNone(t.lan_ip_problem("10.0.0.5", {"10.0.0.5"}))
        self.assertIsNotNone(t.lan_ip_problem("10.0.0.6", {"10.0.0.5"}))


class PatchConfigTests(unittest.TestCase):
    def test_sets_one_host_and_tls_and_drops_insecure(self):
        src = '[gateway]\nhost = "127.0.0.1"\nport = 1\nallow_insecure_remote = true\n[gateway.tls]\nenabled = false\n[other]\nx = 1\n'
        out = t.patch_tls_config(src, "10.0.0.5")
        self.assertEqual(out.count("[gateway.tls]"), 1)
        self.assertIn('host = "10.0.0.5"', out)
        self.assertNotIn("127.0.0.1", out)
        self.assertNotIn("= true\n[other]", out)
        self.assertIn("allow_insecure_remote = false", out)
        self.assertNotIn("allow_insecure_remote = true", out)
        self.assertIn("[other]", out)

    def test_refuses_wildcard(self):
        with self.assertRaises(ValueError):
            t.patch_tls_config("[gateway]\n", "0.0.0.0")


class GateDenialTests(unittest.TestCase):
    def test_only_named_gate_counts(self):
        self.assertTrue(t.is_gate_denial(denial("caller role is not 'operator' (got Member)"), "operator"))
        self.assertTrue(t.is_gate_denial(denial("caller is not loopback"), "loopback"))
        self.assertFalse(t.is_gate_denial(denial("tool not found: operator"), "operator"))
        self.assertFalse(t.is_gate_denial(denial("invalid params operator"), "operator"))
        self.assertFalse(t.is_gate_denial({"result": {"ok": True, "result": "operator"}}, "operator"))
        self.assertFalse(t.is_gate_denial(denial("caller is not loopback"), "operator"))


def body(use, result=None, error=False):
    msgs = [{"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "capability_projection_diagnostics", "input": {}}]}]
    if result is not None:
        msgs.append({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "is_error": error, "content": result}]})
    return {"messages": msgs if use else []}


class AmbientTests(unittest.TestCase):
    def go(self, b):
        return t.ambient_verdict(*t.tool_results([b], "capability_projection_diagnostics"))[0]

    def test_pass_only_for_missing_role(self):
        self.assertEqual(self.go(body(True, "caller role is not 'operator' (got None)", True)), "pass")

    def test_missing_pieces_are_unverified(self):
        self.assertEqual(self.go(body(False)), "unverified")
        self.assertEqual(self.go(body(True)), "unverified")

    def test_wrong_denial_not_pass(self):
        self.assertEqual(self.go(body(True, "caller role is not 'operator' (got Some(Member))", True)), "unverified")
        self.assertEqual(self.go(body(True, "unknown tool", True)), "fail")

    def test_success_is_fail(self):
        self.assertEqual(self.go(body(True, json.dumps({"ok": True, "registry_cursor": 3}))), "fail")


class SnapshotTests(unittest.TestCase):
    def test_key_ignores_counter_noise_but_tracks_state(self):
        a = {"registry_cursor": 1, "applied_tool_ids": ["b", "a"], "applied_owner_generations": [["a", 1]]}
        b = dict(a, applied_tool_ids=["a", "b"], applied_invalidation_count=9)
        self.assertEqual(t.snapshot_key(a), t.snapshot_key(b))
        self.assertNotEqual(t.snapshot_key(a), t.snapshot_key(dict(a, registry_cursor=2)))


if __name__ == "__main__":
    unittest.main()
