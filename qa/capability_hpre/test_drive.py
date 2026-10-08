#!/usr/bin/env python3
"""Small pure-Python regressions for the H-pre evidence helpers."""
from __future__ import annotations

import asyncio
import json
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

import drive
from drive import (
    HoldRaceError,
    applied_recovery_verdict,
    classify_hold_race,
    effect_marker_for,
    effect_markers,
    fixture_args,
    fixture_tool_names,
    is_canonical_closed_chat_terminal,
    is_canonical_host_closed,
    is_closed_status,
    is_hold_already_active,
    names,
    provider_bodies_exclude,
)


class _AsyncCase(unittest.IsolatedAsyncioTestCase):
    """Shared helper: cancel every still-pending task created by a test."""

    def _drain(self, *tasks):
        for t in tasks:
            if not t.done():
                t.cancel()
        return asyncio.gather(*tasks, return_exceptions=True)


class NamesTests(unittest.TestCase):
    def test_names_preserves_empty_set_identity_and_recurses(self):
        out: set[str] = set()
        result = names({"groups": [{"tools": [{"name": "x"}, {"name": "y"}]}]}, out)
        self.assertIs(result, out)
        self.assertEqual(result, {"x", "y"})


class EffectMarkerTests(unittest.TestCase):
    def test_effect_markers_requires_the_exact_tool_effect(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "effects.jsonl"
            rows = [
                {"method": "tools/call", "name": "qa_echo", "arguments": {"marker": "m"}},
                {"method": "tools/call", "name": "other", "arguments": {"marker": "m"}},
                {"method": "tools/list", "name": "qa_echo", "arguments": {"marker": "m"}},
                {"method": "tools/call", "name": "qa_echo", "arguments": {"marker": "wrong"}},
            ]
            path.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
            self.assertEqual(effect_markers(path, "m"), [rows[0]])

    def test_replacement_marker_contract_uses_portable_fixture(self):
        # Deterministic, portable fixture: no /tmp glob, no prior-run dependency.
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "replacement.effects.jsonl"
            row = {
                "method": "tools/call",
                "name": "qa_echo",
                "arguments": {"marker": "QA_HPRE_REPLACEMENT_test_run_id"},
            }
            path.write_text(json.dumps(row) + "\n")

            # Contract: effect_marker_for is the canonical derivation for the
            # replacement label, and the wire-shape we wrote matches it exactly.
            expected = effect_marker_for(SimpleNamespace(run_id="test_run_id"), "replacement")
            self.assertEqual(expected, "QA_HPRE_REPLACEMENT_test_run_id")
            self.assertEqual(effect_markers(path, expected), [row], "exact qa_echo replacement effects must match")

            # Negative: the old QA_HPRE_REPLACE_${RUN_ID} spelling (no '_MENT' suffix)
            # is not the replacement marker and must not match the wire shape.
            self.assertFalse(effect_markers(path, "QA_HPRE_REPLACE_test_run_id"))

        # run.sh still carries the same two-spec literal as a static-text contract.
        self.assertIn("QA_HPRE_REPLACEMENT_${RUN_ID}", Path(__file__).with_name("run.sh").read_text())


class FixtureContractTests(unittest.TestCase):
    def test_fixture_tool_count_and_argv_contract(self):
        self.assertEqual(fixture_tool_names(0), ["qa_echo"])
        names_300 = fixture_tool_names(300)
        self.assertEqual(len(names_300), 301)
        self.assertEqual(names_300[0], "qa_echo")
        self.assertEqual(names_300[-1], "temporaryqa_echo_299")
        self.assertEqual(fixture_args("fixture.py", "effects.jsonl"), ["fixture.py", "effects.jsonl", "1"])
        self.assertEqual(fixture_args("fixture.py", "effects.jsonl", 300), ["fixture.py", "effects.jsonl", "300"])


class PredicateTests(unittest.TestCase):
    def test_hold_and_close_predicates_require_canonical_wire_errors(self):
        self.assertTrue(is_hold_already_active(False, {"rpc_error": {"message": "diagnostics hold already active"}}))
        wrapped = "tool 'capability_projection_diagnostics' failed: Aleph error: capability_projection_diagnostics: diagnostics hold already active"
        self.assertTrue(is_hold_already_active(False, {"rpc_error": {"message": wrapped}}))
        self.assertFalse(is_hold_already_active(False, {"rpc_error": {"message": "other_tool: diagnostics hold already active"}}))
        self.assertFalse(is_hold_already_active(False, {"rpc_error": {"message": "some other error"}}))
        self.assertTrue(is_canonical_host_closed(False, {"rpc_error": {"message": "host_closed"}}))
        self.assertFalse(is_canonical_host_closed(False, {"rpc_error": {"message": "catalog absent"}}))
        self.assertTrue(is_closed_status(False, {"rpc_error": {"message": "diagnostics host is closed"}}))
        self.assertTrue(is_closed_status(True, {"lifecycle": "closed", "registry_cursor": 1, "applied_tool_ids": ["a"]}, ["a"]))
        self.assertFalse(is_closed_status(True, {"lifecycle": "active", "registry_cursor": 1, "applied_tool_ids": []}, []))
        self.assertTrue(is_canonical_closed_chat_terminal({"method": "stream.run_error", "params": {"error": "host_closed"}}))
        self.assertFalse(is_canonical_closed_chat_terminal({"method": "stream.run_complete", "params": {"error": "host_closed"}}))


class ProviderBodiesExcludeTests(unittest.TestCase):
    """Per-body identity exclusion predicate for the post-close provider log."""

    def test_empty_bodies_fail(self):
        self.assertFalse(provider_bodies_exclude([]))
        self.assertFalse(provider_bodies_exclude([], "anything"))

    def test_non_list_body_fails(self):
        self.assertFalse(provider_bodies_exclude(["not a dict"], "x"))
        self.assertFalse(provider_bodies_exclude([None], "x"))

    def test_excluded_id_present_fails(self):
        bodies = [{"tools": [{"name": "alpha"}, {"name": "beta"}]}]
        self.assertFalse(provider_bodies_exclude(bodies, "alpha"))
        self.assertFalse(provider_bodies_exclude(bodies, "beta"))

    def test_excluded_id_absent_passes(self):
        bodies = [{"tools": [{"name": "alpha"}]}]
        self.assertTrue(provider_bodies_exclude(bodies, "beta"))
        self.assertTrue(provider_bodies_exclude(bodies, "beta", "gamma"))

    def test_all_bodies_must_comply(self):
        # body[0] excludes beta, body[1] contains beta => failure on body[1]
        bodies = [{"tools": [{"name": "alpha"}]}, {"tools": [{"name": "beta"}]}]
        self.assertFalse(provider_bodies_exclude(bodies, "alpha", "beta"))

    def test_closed_state_canonical_builtins_only(self):
        # Closed-state runloop still exposes get_tool_schema + subagent, but
        # neither mounted fixture id may appear in ANY recorded body.
        bodies = [{"tools": [{"name": "get_tool_schema"}, {"name": "subagent"}]}]
        self.assertTrue(provider_bodies_exclude(bodies, "qa_hpre_close", "qa_hpre_post_close"))

    def test_old_present_fails_and_new_present_fails(self):
        # Mirror the user-requested pure predicates explicitly.
        old_body = [{"tools": [{"name": "get_tool_schema"}, {"name": "qa_hpre_close"}]}]
        self.assertFalse(provider_bodies_exclude(old_body, "qa_hpre_close", "qa_hpre_post_close"))
        new_body = [{"tools": [{"name": "subagent"}, {"name": "qa_hpre_post_close"}]}]
        self.assertFalse(provider_bodies_exclude(new_body, "qa_hpre_close", "qa_hpre_post_close"))

    def test_all_nonempty_builtins_passes(self):
        # Several bodies, each carrying only the closed-state canonical
        # builtins and omitting both fixture identities.
        bodies = [
            {"tools": [{"name": "get_tool_schema"}, {"name": "subagent"}]},
            {"tools": [{"name": "subagent"}]},
        ]
        self.assertTrue(provider_bodies_exclude(bodies, "qa_hpre_close", "qa_hpre_post_close"))


class HoldRaceTests(_AsyncCase):
    """Two-connection hold race: which side is currently host-owned?"""

    async def test_a_completes_first_as_loser_b_pending_wins(self):
        async def loser_a():
            return False, {"rpc_error": {"message": "diagnostics hold already active"}}

        async def winner_b():
            await asyncio.sleep(60)
            return True, {}

        task_a = asyncio.create_task(loser_a())
        task_b = asyncio.create_task(winner_b())
        try:
            winner_task, loser_task = await classify_hold_race(task_a, task_b, timeout=1.0)
        finally:
            await self._drain(task_a, task_b)
        self.assertIs(winner_task, task_b)
        self.assertIs(loser_task, task_a)

    async def test_b_completes_first_as_loser_a_pending_wins(self):
        async def winner_a():
            await asyncio.sleep(60)
            return True, {}

        async def loser_b():
            await asyncio.sleep(0.05)
            return False, {"rpc_error": {"message": "diagnostics hold already active"}}

        task_a = asyncio.create_task(winner_a())
        task_b = asyncio.create_task(loser_b())
        try:
            winner_task, loser_task = await classify_hold_race(task_a, task_b, timeout=1.0)
        finally:
            await self._drain(task_a, task_b)
        self.assertIs(winner_task, task_a)
        self.assertIs(loser_task, task_b)

    async def test_both_done_is_rejected(self):
        # Both tasks finish synchronously in the same scheduling step, so the
        # FIRST_COMPLETED wait observes both as already done. The contract is
        # that "both done without HoldAlreadyActive on both" cannot prove an
        # active armed hold.
        async def first():
            return False, {"rpc_error": {"message": "diagnostics hold already active"}}

        async def second():
            return True, {"released": True}

        task_a = asyncio.create_task(first())
        task_b = asyncio.create_task(second())
        # Yield once so both created tasks get scheduled and complete before
        # classify_hold_race enters its own wait.
        await asyncio.sleep(0)
        try:
            with self.assertRaises(HoldRaceError) as ctx:
                await classify_hold_race(task_a, task_b, timeout=1.0)
        finally:
            await self._drain(task_a, task_b)
        self.assertIn("both", str(ctx.exception))

    async def test_first_success_not_hold_already_active_is_rejected(self):
        async def first():
            return True, {"ok": True}

        async def winner():
            await asyncio.sleep(60)
            return True, {}

        task_a = asyncio.create_task(first())
        task_b = asyncio.create_task(winner())
        try:
            with self.assertRaises(HoldRaceError) as ctx:
                await classify_hold_race(task_a, task_b, timeout=1.0)
        finally:
            await self._drain(task_a, task_b)
        self.assertIn("HoldAlreadyActive", str(ctx.exception))

    async def test_neither_done_within_bounded_wait_is_rejected(self):
        async def slow_a():
            await asyncio.sleep(60)
            return True, {}

        async def slow_b():
            await asyncio.sleep(60)
            return True, {}

        task_a = asyncio.create_task(slow_a())
        task_b = asyncio.create_task(slow_b())
        try:
            with self.assertRaises(HoldRaceError) as ctx:
                await classify_hold_race(task_a, task_b, timeout=0.2)
        finally:
            await self._drain(task_a, task_b)
        self.assertIn("bounded wait", str(ctx.exception))


class _FakeHost:
    """Fake diagnostics host: first hold wins and never answers; mode decides the rest."""

    def __init__(self, mode="exclusive"):
        self.mode = mode
        self.active = False
        self.sockets = []


class _FakeWs:
    def __init__(self, host):
        self.host = host
        self.queue = asyncio.Queue()
        self.closed = False
        host.sockets.append(self)

    async def send(self, raw):
        msg = json.loads(raw)
        rid = msg["id"]
        if self.host.mode == "send_error":
            raise ConnectionError("send failed")
        if msg["method"] != "tools.invoke":
            await self.queue.put({"jsonrpc": "2.0", "id": rid, "result": {}})
            return
        if self.host.mode == "both_success":
            await self.queue.put({"jsonrpc": "2.0", "id": rid, "result": {"ok": True, "result": {}}})
        elif self.host.mode == "silent" or not self.host.active:
            self.host.active = True  # host-owned hold; RPC stays pending
        else:
            await self.queue.put(
                {"jsonrpc": "2.0", "id": rid, "error": {"message": "diagnostics hold already active"}}
            )

    async def recv(self):
        item = await self.queue.get()
        if item is None:
            raise ConnectionError("closed")
        return json.dumps(item)

    async def close(self):
        self.closed = True
        await self.queue.put(None)


class _FakeCm:
    def __init__(self, ws):
        self.ws = ws

    async def __aenter__(self):
        return self.ws


class _FakeQ:
    def __init__(self):
        self.checks = []

    def check(self, claim, ok, detail=""):
        self.checks.append((claim, ok, detail))


class ArmedHoldTests(_AsyncCase):
    """Run the REAL drive._armed_hold against fake sockets (no live gateway)."""

    def _patched(self, host, **kw):
        return mock.patch.object(drive, "ws_connect", lambda url: _FakeCm(_FakeWs(host)))

    async def _run(self, host, classify_timeout=3.5):
        q = _FakeQ()
        args = SimpleNamespace(ws="ws://fake")
        real = drive.classify_hold_race

        async def short(a, b, *, timeout=3.5):
            return await real(a, b, timeout=classify_timeout)

        with self._patched(host), mock.patch.object(drive, "classify_hold_race", short):
            start = time.monotonic()
            result = await drive._armed_hold(args, q, "delivery")
            return result, q, time.monotonic() - start

    async def test_one_physical_connection_wins_real_hold_is_pending(self):
        host = _FakeHost()
        result, q, elapsed = await self._run(host)
        try:
            self.assertIsNotNone(result)
            winner_ws, winner_task = result
            self.assertFalse(winner_task.done())  # real hold still active, not released
            self.assertIs(winner_ws, host.sockets[0])  # first hold RPC won the host
            self.assertFalse(winner_ws.closed)
            self.assertTrue(host.sockets[1].closed)  # loser socket released
            self.assertLessEqual(elapsed, 3.5)
        finally:
            await self._drain(winner_task)
            await winner_ws.close()

    async def test_both_orders_via_second_connection_winning(self):
        host = _FakeHost()
        # Delay the first socket's hold send so the second socket wins the host.
        orig_send = _FakeWs.send

        async def second_first(self_ws, raw):
            if self_ws is host.sockets[0] and json.loads(raw)["method"] == "tools.invoke":
                await asyncio.sleep(0.05)
            return await orig_send(self_ws, raw)

        with mock.patch.object(_FakeWs, "send", second_first):
            result, q, elapsed = await self._run(host)
        try:
            self.assertIsNotNone(result)
            winner_ws, winner_task = result
            self.assertIs(winner_ws, host.sockets[1])
            self.assertFalse(winner_task.done())
            self.assertTrue(host.sockets[0].closed)
        finally:
            await self._drain(winner_task)
            await winner_ws.close()

    async def test_both_success_retries_then_fails_and_cleans_up(self):
        host = _FakeHost("both_success")
        result, q, _ = await self._run(host)
        self.assertIsNone(result)
        self.assertEqual(q.checks[-1][1], False)
        self.assertEqual(len(host.sockets), 6)
        self.assertTrue(all(w.closed for w in host.sockets))

    async def test_timeout_cleans_up_pending_tasks_and_sockets(self):
        host = _FakeHost("silent")
        before = {t for t in asyncio.all_tasks()}
        result, q, _ = await self._run(host, classify_timeout=0.1)
        self.assertIsNone(result)
        self.assertTrue(all(w.closed for w in host.sockets))
        leftover = {t for t in asyncio.all_tasks() if not t.done()} - before - {asyncio.current_task()}
        self.assertEqual(leftover, set())

    async def test_unexpected_error_cancels_and_closes_everything(self):
        host = _FakeHost("send_error")
        # connect() RPCs also fail on send, so the error surfaces during setup.
        before = {t for t in asyncio.all_tasks()}
        with self.assertRaises(ConnectionError):
            await self._run(host)
        self.assertTrue(all(w.closed for w in host.sockets))
        leftover = {t for t in asyncio.all_tasks() if not t.done()} - before - {asyncio.current_task()}
        self.assertEqual(leftover, set())


class AppliedRecoveryVerdictTests(unittest.TestCase):
    BASE = "srv__qa_echo"

    def before(self, **kw):
        s = {
            "registry_cursor": 10,
            "applied_tool_ids": [self.BASE],
            "applied_invalidation_count": 1,
            "applied_replacement_count": 1,
            "last_replacement_registry_cursor": 7,
            "last_replacement_tool_ids": [self.BASE],
        }
        s.update(kw)
        return s

    def after(self, **kw):
        s = self.before(
            registry_cursor=14,
            applied_invalidation_count=2,
            applied_replacement_count=2,
            last_replacement_registry_cursor=12,
            last_replacement_tool_ids=[self.BASE, "srv__temporaryqa_echo_0"],
        )
        s.update(kw)
        return s

    def verdict(self, before=None, after=None, temp=("srv__temporaryqa_echo_0",)):
        return applied_recovery_verdict(before or self.before(), after or self.after(), self.BASE, temp)[0]

    def test_pass_allows_pair_older_than_final_cursor(self):
        self.assertEqual(self.verdict(), "pass")

    def test_missing_fields_are_unverified_not_pass(self):
        after = self.after()
        del after["applied_replacement_count"]
        self.assertEqual(self.verdict(after=after), "unverified")
        self.assertEqual(applied_recovery_verdict({"registry_cursor": 1}, {"registry_cursor": 2}, self.BASE)[0], "unverified")

    def test_source_side_counter_is_not_an_applied_receipt(self):
        after = self.after(applied_replacement_count=1, replacement_count=99)
        self.assertEqual(self.verdict(after=after), "fail")

    def test_counters_must_both_increase(self):
        self.assertEqual(self.verdict(after=self.after(applied_invalidation_count=1)), "fail")

    def test_pair_cursor_must_come_from_the_new_registry_window(self):
        self.assertEqual(self.verdict(after=self.after(last_replacement_registry_cursor=10)), "fail")
        self.assertEqual(self.verdict(after=self.after(last_replacement_registry_cursor=15)), "fail")
        self.assertEqual(self.verdict(after=self.after(last_replacement_registry_cursor=None)), "fail")

    def test_payload_must_name_base_and_final_must_drop_temporaries(self):
        self.assertEqual(self.verdict(after=self.after(last_replacement_tool_ids=[])), "fail")
        self.assertEqual(self.verdict(after=self.after(last_replacement_tool_ids=["x"])), "fail")
        leaked = self.after(applied_tool_ids=[self.BASE, "srv__temporaryqa_echo_0"])
        self.assertEqual(self.verdict(after=leaked), "fail")

    def test_final_must_keep_base(self):
        self.assertEqual(self.verdict(after=self.after(applied_tool_ids=[])), "fail")


if __name__ == "__main__":
    unittest.main()
