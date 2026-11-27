#!/usr/bin/env python3
"""Real-machine QA for plan mode (`ExecTier::Plan`) and the plan→build handoff.

Three scenarios, each a separate claim that only a running server can settle.

`handoff` — the end-to-end. A session opened with `exec_tier: "plan"` must:
    1. keep `file_write` in the tool surface (plan mode refuses at dispatch,
       it does not hide), and refuse the call with `GateRule::PlanMode`'s own
       sentence;
    2. run `scratchpad set_plan` normally;
    3. stop `scratchpad{action:"request_approval"}` for a human through
       `clarification::ask` — answered here over `clarification.resolve`;
    4. on `approved`, unlock **inside the same run**: the next `file_write`
       actually writes, and the model is told `Plan approved`;
    5. leave the session's exec tier off `plan` (the gate writes the release
       through to the session, so attached clients see the same tier).

`deny` — the same question, answered `rejected`. Plan mode must still hold:
  a declined plan is not a lifted gate.

`floor` — plan mode's position in the permission stack. The config carries
  explicit `[policies.tool_permissions.overrides] bash = "allow"` and
  `file_write = "allow"`, which beat the tier by design; they must NOT beat plan
  mode. Also pins the argument-aware half: `file_ops` list is admitted and
  `file_ops` delete is refused — same tool, same run.

Every scenario finally checks that a `chat.send` carrying the removed
`plan_phase` field is rejected rather than silently ignored.

Usage:  drive_plan_handoff.py WS_URL DB OBSERVATIONS SCENARIO
"""
import argparse
import asyncio
import json
import os
import re
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

# The opening of `GateRule::PlanMode::reason` (src/tools/scoped/gate_chain.rs).
# The model-facing refusal carries the rule's sentence, not its id, so this is
# how a tool result names the gate that refused it.
PLAN_MODE_PHRASE = "This conversation is PLANNING"
APPROVAL_QUESTION_ID = "plan_approval"

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
    Nth entry of the first turn that carries at least N of them. Reading it off
    a fixed turn would break the moment a side-channel call landed between.
    """
    for obs in turn_obs:
        rs = obs.get("tool_results") or []
        if len(rs) >= call_index:
            return rs[call_index - 1]
    return None


async def await_approval_question(ws, session_key, rid_base, budget=240):
    """Poll `clarification.pending` for this session's plan-approval question.
    Matched by session and question id, not by position: another pending
    question must not be answered by accident."""
    end = time.monotonic() + budget
    rid = rid_base
    while time.monotonic() < end:
        await rpc(ws, "clarification.pending", {}, rid)
        r = await reply(ws, rid)
        for p in (r.get("result") or {}).get("pending") or []:
            ids = [q.get("id") for q in p.get("questions") or []]
            if p.get("session_key") == session_key and APPROVAL_QUESTION_ID in ids:
                return p
        rid += 1
        await asyncio.sleep(0.5)
    return None


async def session_exec_tier(ws, rid, session_key):
    """The session's exec tier as every attached client reads it."""
    await rpc(ws, "chat.history", {"session_key": session_key}, rid)
    r = await reply(ws, rid)
    session = (r.get("result") or {}).get("session") or {}
    return session.get("exec_tier"), session


async def control_turn(ws, slog):
    """One ordinary `full`-tier session, on this server, in this config.

    The A of the A/B. "Still listed while planning" is only worth anything
    next to a run where the tool is certainly listed.
    """
    await rpc(
        ws,
        "chat.send",
        {
            "message": "Say hello.",
            "channel": "gui:qa-plan-control",
            "exec_tier": "full",
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
    check(fin is not None, "the control (full tier) run finished")


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
            "exec_tier": "plan",
        },
        2,
    )
    r = await reply(ws, 2)
    if "result" not in r:
        log(f"FAIL: chat.send rejected: {r}")
        failures.append("chat.send accepted exec_tier plan")
        return
    session_key = r["result"]["session_key"]
    run_id = r["result"]["run_id"]
    log(f"run {run_id} session {session_key}")

    if args.scenario in ("handoff", "deny"):
        pending = await await_approval_question(ws, session_key, 100)
        if not check(
            pending is not None,
            "request_approval asks the human through clarification.pending",
        ):
            return
        log(f"pending: {json.dumps(pending)[:600]}")
        verdict = "approved" if args.scenario == "handoff" else "rejected"
        await rpc(
            ws,
            "clarification.resolve",
            {
                "session_key": session_key,
                "reply": verdict,
                "expected_request_id": pending["request_id"],
                "expected_question_id": APPROVAL_QUESTION_ID,
            },
            200,
        )
        rr = await reply(ws, 200)
        check(
            (rr.get("result") or {}).get("resolved") is True,
            f"clarification.resolve accepts the `{verdict}` reply",
            json.dumps(rr)[:240],
        )

    # #2 because the control run already committed one.
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

    tier, session = await session_exec_tier(ws, 300, session_key)
    if args.scenario == "handoff":
        check(
            tier != "plan",
            "after approval, the session's exec tier is no longer `plan`",
            f"exec_tier={tier!r}",
        )
    else:
        check(
            tier == "plan",
            "without approval, the session's exec tier is still `plan`",
            f"exec_tier={tier!r} session={json.dumps(session)[:300]}",
        )


async def removed_field_is_rejected(ws):
    """`plan_phase` was removed with the first plan design. A request that
    still carries it must fail loudly; a silent ignore would let an old client
    believe it is planning while every tool runs."""
    await rpc(
        ws,
        "chat.send",
        {
            "message": "Say hello.",
            "channel": "gui:qa-plan-removed-field",
            "plan_phase": "planning",
        },
        400,
    )
    r = await reply(ws, 400)
    check(
        "error" in r,
        "a chat.send carrying the removed `plan_phase` field is rejected",
        json.dumps(r)[:240],
    )


def control_offered(obs, tool):
    """Was `tool` in the CONTROL (full tier) turn's surface?"""
    return bool(obs) and tool in obs[0]["tools_visible"]


def assert_listed_but_refused(obs, planning_turn, tool, call_index):
    """Plan mode keeps `tool` visible and refuses the call at dispatch."""
    check(
        control_offered(obs, tool),
        f"CONTROL: a full-tier session in this mode IS offered `{tool}`",
        f"control surface = {len(obs[0]['tools_visible']) if obs else 0} tools",
    )
    check(
        tool in obs[planning_turn]["tools_visible"],
        f"while planning, `{tool}` is still listed in the tool surface",
        f"planning surface = {len(obs[planning_turn]['tools_visible'])} tools",
    )
    result = result_for(obs, call_index)
    check(
        result is not None and PLAN_MODE_PHRASE in (result["text"] or ""),
        f"while planning, calling `{tool}` is refused by plan mode",
        f"result={(result or {}).get('text', '')[:240]!r}",
    )


def written_path(result_text):
    """The path out of `file_write`'s `Wrote N bytes to PATH`. The tool result
    is a JSON object, so the path ends at the closing quote of that string."""
    m = re.search(r"Wrote \d+ bytes to ([^\"'\s]+)", result_text or "")
    return m.group(1) if m else None


def assert_handoff(obs):
    if not check(len(obs) >= 5, f"at least 5 scripted turns ran (saw {len(obs)})"):
        return
    assert_listed_but_refused(obs, 1, "file_write", 1)
    r2 = result_for(obs, 2)
    check(
        r2 is not None and not r2["is_error"],
        "`scratchpad set_plan` runs normally while planning",
        f"result={(r2 or {}).get('text', '')[:160]!r}",
    )
    r3 = result_for(obs, 3)
    check(
        r3 is not None and "Plan approved" in (r3["text"] or ""),
        "the approved handoff tells the model `Plan approved`",
        f"result={(r3 or {}).get('text', '')[:300]!r}",
    )
    r4 = result_for(obs, 4)
    text = (r4 or {}).get("text", "")
    check(
        r4 is not None and "Wrote" in text and PLAN_MODE_PHRASE not in text,
        "after approval, the SAME RUN's next `file_write` writes",
        f"result={text[:200]!r}",
    )
    path = written_path(text)
    check(
        path is not None and os.path.exists(os.path.expanduser(path)),
        "the post-approval file is on disk",
        f"path={path!r}",
    )


def assert_deny(obs):
    if not check(len(obs) >= 4, f"at least 4 scripted turns ran (saw {len(obs)})"):
        return
    r2 = result_for(obs, 2)
    check(
        r2 is not None and "Plan rejected" in (r2["text"] or ""),
        "a rejected plan is reported to the model as a rejection",
        f"result={(r2 or {}).get('text', '')[:240]!r}",
    )
    assert_listed_but_refused(obs, 3, "file_write", 3)


def assert_floor(obs):
    if not check(len(obs) >= 4, f"at least 4 scripted turns ran (saw {len(obs)})"):
        return
    assert_listed_but_refused(obs, 1, "bash", 1)
    r2 = result_for(obs, 2)
    # The claim is that plan mode let it through, so the evidence is the
    # absence of the refusal — not the tool's success ("Directory not found"
    # is a call that got past the gate exactly as claimed).
    check(
        r2 is not None and PLAN_MODE_PHRASE not in (r2["text"] or ""),
        "`file_ops` list — a read-only operation — is NOT refused by plan mode",
        f"result={(r2 or {}).get('text', '')[:160]!r}",
    )
    r3 = result_for(obs, 3)
    check(
        r3 is not None and PLAN_MODE_PHRASE in (r3["text"] or ""),
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
        # Last on purpose: if the field were accepted it would start a run,
        # and that run must not consume the scenario's scripted turns.
        await removed_field_is_rejected(ws)

    log("")
    if failures:
        log(f"VERDICT: FAIL ({len(failures)} claim(s))")
        for f in failures:
            log(f"  - {f}")
        return 1
    log("VERDICT: PASS")
    return 0


sys.exit(asyncio.run(main()))
