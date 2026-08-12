#!/usr/bin/env python3
"""Real-machine QA for the read-only planning phase and the plan→build handoff.

Three scenarios, each a separate claim that only a running server can settle.

`handoff` — the end-to-end. A session opened with `plan_phase: "planning"` and
  `exec_tier: "full"` (the WIDEST tier on purpose: if the floor holds here it
  holds everywhere) must:
    1. refuse `file_write` at dispatch, with the floor's own wording;
    2. not have offered `file_write` in the tool surface at all;
    3. run `scratchpad` normally;
    4. stop `scratchpad{action:"request_build"}` for a human, on a card whose
       decision set is allow-once/deny — no standing grant, even for an operator;
    5. on approval, unlock **inside the same run**: the next turn's tool surface
       contains `file_write` again (the `cache_generation` bump reaching a live
       run) and the call actually writes;
    6. leave `plan_phase = building` persisted on the session.

`deny` — the same card, declined. The floor must still be engaged afterwards:
  a declined plan is not a lifted latch.

`floor` — the floor's position in the permission stack. The config carries an
  explicit `[policies.tool_permissions.overrides] bash = "allow"`, which beats
  the tier by design; it must NOT beat the floor. Also pins the argument-aware
  half: `file_ops` list is admitted and `file_ops` delete is refused, same tool,
  same turn sequence.

Usage:  drive_plan_handoff.py WS_URL DB OBSERVATIONS SCENARIO
"""
import argparse
import asyncio
import json
import sys
import time

import websockets

sys.path.insert(0, __file__.rsplit("/", 1)[0] + "/../busy_input")
from lib import SessionLog, log, reply, rpc  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("url")
ap.add_argument("db")
ap.add_argument("observations")
ap.add_argument("scenario", choices=["handoff", "deny", "floor"])
args = ap.parse_args()

FLOOR_PHRASE = "read-only planning phase"
CARD_PHRASE = "finished planning"

failures = []


def check(ok, claim, evidence=""):
    """Record one claim. Never raises: a scenario reports every failure it
    finds, because the second one is usually the interesting one."""
    log(("  PASS  " if ok else "  FAIL  ") + claim)
    if evidence:
        log(f"          {evidence}")
    if not ok:
        failures.append(claim)
    return ok


def observations(path):
    out = []
    try:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if line:
                    out.append(json.loads(line))
    except FileNotFoundError:
        pass
    return out


def turns(path):
    return [o for o in observations(path) if o.get("kind") == "turn"]


def result_for(turn_obs, call_index):
    """The tool_result text produced by the `call_index`-th scripted call.

    Results accumulate in the conversation, so the Nth call's result is the
    Nth entry of the LAST turn that carries at least N of them. Reading it off
    a fixed turn would break the moment a side-channel call landed between.
    """
    for obs in turn_obs:
        rs = obs.get("tool_results") or []
        if len(rs) >= call_index:
            return rs[call_index - 1]
    return None


async def pending_cards(ws, rid):
    await rpc(ws, "exec.approvals.pending", {}, rid)
    r = await reply(ws, rid)
    if "result" not in r:
        return []
    return r["result"].get("pending", [])


async def await_card(ws, rid_base, budget=240):
    """Poll for the plan-handoff card. Identified by the rule's own prose, not
    by position: an `Auto`-tier turn can park other cards, and picking "the
    first pending one" would silently test whichever arrived first."""
    end = time.monotonic() + budget
    rid = rid_base
    while time.monotonic() < end:
        for p in await pending_cards(ws, rid):
            rec = p.get("record", {})
            if CARD_PHRASE in (rec.get("reason") or ""):
                return rec
        rid += 1
        await asyncio.sleep(0.5)
    return None


async def session_row(ws, rid, session_key):
    await rpc(ws, "sessions.list", {"limit": 50}, rid)
    r = await reply(ws, rid)
    rows = (r.get("result") or {}).get("sessions") or []
    for row in rows:
        if row.get("session_key") == session_key or row.get("key") == session_key:
            return row
    return {"__rows__": rows}


async def control_turn(ws, slog):
    """One ordinary `building` session, on this server, in this config.

    The A of the A/B. Every "X is absent while planning" claim below is only
    worth anything next to a run where X was present, and the cheapest thing
    that can be wrong about this whole fixture is that the tool was never
    offered in this session mode to begin with.
    """
    await rpc(
        ws,
        "chat.send",
        {
            "message": "Say hello.",
            "channel": "gui:qa-plan-control",
            "exec_tier": "full",
            "plan_phase": "building",
        },
        50,
    )
    r = await reply(ws, 50)
    if "result" not in r:
        failures.append("the control run started")
        log(f"FAIL: control chat.send rejected: {r}")
        return
    log(f"control run {r['result']['run_id']} session {r['result']['session_key']}")
    fin = await slog.wait_for("run_finished", 1, 180)
    check(fin is not None, "the control (building) run finished")


async def run_scenario(ws, slog):
    opening = {
        "handoff": "Plan the work, then ask me to approve it.",
        "deny": "Plan the work, then ask me to approve it.",
        "floor": "Plan the work.",
    }[args.scenario]

    await rpc(
        ws,
        "chat.send",
        {
            "message": opening,
            "channel": f"gui:qa-plan-{args.scenario}",
            # The widest tier on purpose — see the module doc.
            "exec_tier": "full",
            "plan_phase": "planning",
        },
        2,
    )
    r = await reply(ws, 2)
    if "result" not in r:
        log(f"FAIL: chat.send rejected: {r}")
        failures.append("chat.send accepted plan_phase")
        return
    session_key = r["result"]["session_key"]
    run_id = r["result"]["run_id"]
    log(f"run {run_id} session {session_key}")

    if args.scenario in ("handoff", "deny"):
        card = await await_card(ws, 100)
        if not check(card is not None, "the handoff raises an approval card"):
            return
        log(f"card: {json.dumps(card, indent=2)[:900]}")
        check(
            card.get("allowed_decisions") == ["allow-once", "deny"],
            "the card offers allow-once/deny only (no standing grant, "
            "even though this connection is operator)",
            f"allowed_decisions={card.get('allowed_decisions')}",
        )
        check(
            "scratchpad" not in (card.get("reason") or ""),
            "the card asks about the PLAN, not about the scratchpad tool",
            f"reason={(card.get('reason') or '')[:120]!r}",
        )
        decision = "allow-once" if args.scenario == "handoff" else "deny"
        await rpc(
            ws,
            "exec.approval.resolve",
            {"id": card["id"], "decision": decision, "resolved_by": "qa"},
            200,
        )
        rr = await reply(ws, 200)
        log(f"resolved {decision} -> {json.dumps(rr)[:200]}")

    # The run ends when the mock returns end_turn (or, for `deny`, when the
    # refused handoff has been reported and the plan runs out). #2 because the
    # control run already committed one.
    fin = await slog.wait_for("run_finished", 2, 300)
    check(fin is not None, "the planning run finished")
    await asyncio.sleep(2)  # let the last observation flush

    obs = turns(args.observations)
    log(f"{len(obs)} scripted turns observed")
    for o in obs:
        log(
            f"  turn {o['turn']}: file_write_visible="
            f"{'file_write' in o['tools_visible']} "
            f"bash_visible={'bash' in o['tools_visible']} "
            f"tools={len(o['tools_visible'])} -> {o['will_call']}"
        )

    if args.scenario == "handoff":
        assert_handoff(obs)
    elif args.scenario == "deny":
        assert_deny(obs)
    else:
        assert_floor(obs)

    row = await session_row(ws, 300, session_key)
    expected = "building" if args.scenario == "handoff" else "planning"
    check(
        row.get("plan_phase") == expected,
        f"the session's persisted plan_phase is {expected!r}",
        f"row={json.dumps(row)[:300]}",
    )


def control_offered(obs, tool):
    """Was `tool` in the CONTROL (building) turn's surface? Every absence claim
    below is read against this, so a tool that this session mode never offers
    cannot masquerade as a tool the floor removed."""
    return bool(obs) and tool in obs[0]["tools_visible"]


def assert_hidden(obs, planning_turn, tool):
    """`tool` was offered to a building session and is gone from a planning one."""
    check(
        control_offered(obs, tool),
        f"CONTROL: a building session in this mode IS offered `{tool}`",
        f"control surface = {len(obs[0]['tools_visible']) if obs else 0} tools",
    )
    check(
        tool not in obs[planning_turn]["tools_visible"],
        f"while planning, `{tool}` is not in the tool surface at all",
        f"planning surface = {len(obs[planning_turn]['tools_visible'])} tools",
    )


def assert_handoff(obs):
    if not check(len(obs) >= 5, f"at least 5 scripted turns ran (saw {len(obs)})"):
        return
    assert_hidden(obs, 1, "file_write")
    assert_hidden(obs, 1, "bash")
    check(
        "scratchpad" in obs[1]["tools_visible"]
        and "file_ops" in obs[1]["tools_visible"],
        "the argument-dependent tools stay visible while planning",
    )

    r1 = result_for(obs, 1)
    check(
        r1 is not None and FLOOR_PHRASE in r1["text"],
        "a `file_write` that arrives anyway is refused by the floor at dispatch",
        f"result={(r1 or {}).get('text', '')[:200]!r}",
    )
    r2 = result_for(obs, 2)
    check(
        r2 is not None and not r2["is_error"],
        "`scratchpad set_plan` runs normally while planning",
        f"result={(r2 or {}).get('text', '')[:160]!r}",
    )
    r3 = result_for(obs, 3)
    check(
        r3 is not None and "approv" in (r3["text"] or "").lower(),
        "the approved handoff tells the model execution is unlocked",
        f"result={(r3 or {}).get('text', '')[:300]!r}",
    )

    check(
        "file_write" in obs[4]["tools_visible"],
        "AFTER approval, the SAME RUN's next tool surface contains `file_write` "
        "again (the cache_generation bump reached a live run)",
        f"post-approval surface = {len(obs[4]['tools_visible'])} tools",
    )
    r4 = result_for(obs, 4)
    check(
        r4 is not None and "Wrote" in (r4["text"] or ""),
        "after approval, `file_write` actually writes",
        f"result={(r4 or {}).get('text', '')[:200]!r}",
    )


def assert_deny(obs):
    if not check(len(obs) >= 4, f"at least 4 scripted turns ran (saw {len(obs)})"):
        return
    r2 = result_for(obs, 2)
    check(
        r2 is not None and "not approve" in (r2["text"] or ""),
        "a declined handoff is reported as a refusal, not as a success",
        f"result={(r2 or {}).get('text', '')[:240]!r}",
    )
    assert_hidden(obs, 3, "file_write")
    r3 = result_for(obs, 3)
    check(
        r3 is not None and FLOOR_PHRASE in (r3["text"] or ""),
        "after a declined handoff, a mutating call is still refused",
        f"result={(r3 or {}).get('text', '')[:200]!r}",
    )


def assert_floor(obs):
    if not check(len(obs) >= 4, f"at least 4 scripted turns ran (saw {len(obs)})"):
        return
    assert_hidden(obs, 1, "bash")
    check(
        "file_write" not in obs[1]["tools_visible"],
        'an explicit `file_write = "allow"` override does not put it back either',
    )
    r1 = result_for(obs, 1)
    check(
        r1 is not None and FLOOR_PHRASE in (r1["text"] or ""),
        "a `bash` call is refused by the floor despite an explicit allow entry "
        "and the `full` tier",
        f"result={(r1 or {}).get('text', '')[:200]!r}",
    )
    r2 = result_for(obs, 2)
    # The claim is that the FLOOR let it through, so the evidence is the
    # absence of the floor's refusal — not the tool's success. A first pass
    # asserted `not is_error` and failed on "Directory not found", which is a
    # call that got past the floor exactly as claimed.
    check(
        r2 is not None and FLOOR_PHRASE not in (r2["text"] or ""),
        "`file_ops` list — a read-only operation — is NOT refused by the floor",
        f"result={(r2 or {}).get('text', '')[:160]!r}",
    )
    r3 = result_for(obs, 3)
    check(
        r3 is not None and FLOOR_PHRASE in (r3["text"] or ""),
        "`file_ops` delete — same tool, mutating operation — IS refused",
        f"result={(r3 or {}).get('text', '')[:200]!r}",
    )


async def main():
    slog = SessionLog(args.db)
    async with websockets.connect(args.url, max_size=None) as ws:
        await rpc(ws, "connect", {"client_info": {"name": "qa-plan-handoff"}}, 1)
        r = await reply(ws, 1)
        log("connect ->", r["result"]["role"])
        await control_turn(ws, slog)
        await run_scenario(ws, slog)

    log("")
    if failures:
        log(f"VERDICT: FAIL ({len(failures)} claim(s))")
        for f in failures:
            log(f"  - {f}")
        return 1
    log("VERDICT: PASS")
    return 0


sys.exit(asyncio.run(main()))
