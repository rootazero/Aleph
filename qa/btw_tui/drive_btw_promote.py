#!/usr/bin/env python3
"""Real-machine QA for `/btw promote` — the one crossing back into the main
conversation.

Everything in-process about promote runs on an engine with **no orchestrator**,
because building one in a unit test is not practical: those tests can therefore
prove which branch of `execute()` ran and what the carrier looks like, but the
success path of `serve_btw_promote` — the one that actually resolves the session
service, reads the side log and appends to the main one — has never executed
under them. That is the "两半各自有测试，中间那根线没接" shape, and this is the
probe that refuses it.

What only a real gateway can say:

  1. `/btw promote` is SERVED, not asked. Its one `stream.run_accepted` names
     the MAIN session key. An ordinary side turn's names the derived
     `…:ephemeral:btw-…` key (the sibling probe asserts exactly that), so this
     single field separates "the crossing happened" from "the model was asked
     about the word `promote`".
  2. No model was asked. The side session's own log gains no new turn, and the
     receipt reports zero tokens.
  3. The answer really crossed: `chat.history` on the MAIN session now carries
     the carrier, fenced as `<system-reminder>` and naming both the question and
     the side answer's text.
  4. The receipt names the question that crossed, so the user knows WHAT moved.

Usage:  drive_btw_promote.py WS_URL --db PATH_TO_sessions.db
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
ap.add_argument("--db", required=True, help="path to the server's sessions.db")
ap.add_argument("--budget", type=float, default=120.0)
args = ap.parse_args()

FAILURES = []

# The answer the mock's final turn produces. Asserting on the mock's own words
# rather than on "some text is present" is what makes (3) about the crossing
# rather than about the carrier being non-empty.
SIDE_QUESTION = "what is 2+2?"


def check(ok, label, detail=""):
    log(("PASS " if ok else "FAIL ") + label + (f"  {detail}" if detail else ""))
    if not ok:
        FAILURES.append(label)


def run_id_of(msg):
    return (msg.get("params") or {}).get("run_id")


async def collect_until_complete(ws, run_id, budget):
    """Every frame naming `run_id`, up to and including its terminal one."""
    frames = []
    end = time.monotonic() + budget
    while time.monotonic() < end:
        try:
            raw = await asyncio.wait_for(ws.recv(), timeout=5)
        except asyncio.TimeoutError:
            continue
        msg = json.loads(raw)
        if run_id_of(msg) != run_id:
            continue
        frames.append(msg)
        if msg.get("method") in ("stream.run_complete", "stream.run_error"):
            return frames
    return frames


async def main():
    async with websockets.connect(args.url, max_size=None) as ws:
        await rpc(ws, "connect", {"client_info": {"name": "qa-btw-promote"}}, 1)
        r = await reply(ws, 1)
        log("connect ->", r["result"]["role"])

        # --- a side question, answered to completion ------------------------
        await rpc(
            ws,
            "agent.run",
            {"input": f"/btw {SIDE_QUESTION}", "channel": "gui:qa-btw-promote"},
            2,
        )
        r = await reply(ws, 2)
        main_key = r["result"]["session_key"]
        run_side = r["result"]["run_id"]
        log(f"side run {run_side} on main session {main_key}")

        side_frames = await collect_until_complete(ws, run_side, args.budget)
        accepted = [
            (m.get("params") or {}).get("session_key")
            for m in side_frames
            if m.get("method") == "stream.run_accepted"
        ]
        side_key = accepted[0] if accepted else ""
        check(
            bool(side_key) and ":ephemeral:btw-" in side_key,
            "the side question ran on the derived side session",
            f"key={side_key!r}",
        )
        answer = ""
        for m in side_frames:
            if m.get("method") == "stream.run_complete":
                answer = ((m.get("params") or {}).get("summary") or {}).get(
                    "final_response"
                ) or ""
        check(
            bool(answer.strip()),
            "the side question was answered — nothing else here means anything "
            "without a completed exchange to carry",
            f"answer={answer[:60]!r}",
        )

        # The side log as it stands BEFORE the promote. (2) below is the claim
        # that this number does not move — so the count has to be non-zero
        # first, or "it did not move" is a predicate about nothing.
        #
        # `session_events.session_id` stores the JSON serialization of the
        # `SessionKey` (`session_id_to_string`), NOT the `agent:…:…` wire
        # spelling the frames carry, so the ephemeral id is the only part of
        # the key that appears in both. Handing the wire form to `SessionLog`
        # matches no row and reports 0 turns for a session that has one, which
        # is exactly how this assertion would have gone quietly vacuous.
        ephemeral_id = side_key.rsplit(":", 1)[-1]
        stored_side = next(
            (s for s in SessionLog(args.db).sessions() if ephemeral_id in s), None
        )
        check(
            stored_side is not None,
            "the side session is findable in the event log — without this the "
            "turn-count assertion below would be about nothing",
            f"looking for {ephemeral_id!r}",
        )
        side_log = SessionLog(args.db, stored_side)
        turns_before = len(side_log.rows("turn_started"))
        check(
            turns_before > 0,
            "the side session really recorded its own turn",
            f"{turns_before} turn(s)",
        )
        log(f"side session holds {turns_before} turn(s) before the promote")

        # --- the crossing ---------------------------------------------------
        await rpc(
            ws, "agent.run", {"input": "/btw promote", "session_key": main_key}, 3
        )
        r = await reply(ws, 3)
        run_promote = r["result"]["run_id"]
        check(
            r["result"]["session_key"] == main_key,
            "the agent.run reply for a promote names the MAIN session key",
            f"reply={r['result']['session_key']!r} main={main_key!r}",
        )

        promote_frames = await collect_until_complete(ws, run_promote, args.budget)
        promote_accepted = [
            (m.get("params") or {}).get("session_key")
            for m in promote_frames
            if m.get("method") == "stream.run_accepted"
        ]
        # (1) THE discriminator. An ordinary side turn is accepted on the
        # derived key; a served promote is accepted on the conversation the
        # user is looking at, because that is where its receipt has to arrive.
        check(
            len(promote_accepted) == 1,
            "exactly one run_accepted names the promote",
            f"got {len(promote_accepted)}: {promote_accepted}",
        )
        check(
            promote_accepted[:1] == [main_key],
            "the promote is ACCEPTED ON THE MAIN SESSION — if it named the "
            "derived key it was run as an ordinary side question about the "
            "literal word `promote`",
            f"{promote_accepted}",
        )

        receipt = ""
        tokens = None
        for m in promote_frames:
            if m.get("method") == "stream.run_complete":
                summary = (m.get("params") or {}).get("summary") or {}
                receipt = summary.get("final_response") or ""
                tokens = summary.get("total_tokens")
        # (4)
        check(
            SIDE_QUESTION in receipt,
            "the receipt names the question that crossed",
            f"receipt={receipt[:120]!r}",
        )
        # (2), first half: nothing was billed, because nothing was asked.
        check(
            tokens == 0,
            "the promote spent no tokens — no model was asked anything",
            f"total_tokens={tokens!r}",
        )

        # (2), second half: the side thread gained no turn of its own.
        turns_after = len(side_log.rows("turn_started"))
        check(
            turns_after == turns_before,
            "the side session gained no new turn — a promote is not a side "
            "question",
            f"{turns_before} -> {turns_after}",
        )

        # (3) The answer really is in the main conversation now.
        await rpc(ws, "chat.history", {"session_key": main_key, "limit": 50}, 4)
        r = await reply(ws, 4)
        messages = (r.get("result") or {}).get("messages") or []
        carriers = [
            m
            for m in messages
            if m.get("role") == "user" and "<system-reminder>" in (m.get("content") or "")
        ]
        check(
            len(carriers) == 1,
            "the main transcript gained exactly one carrier",
            f"{len(carriers)} of {len(messages)} messages",
        )
        if carriers:
            text = carriers[0]["content"]
            check(
                SIDE_QUESTION in text,
                "the carrier names the question, which is what gives the answer "
                "its referent",
                f"{text[:160]!r}",
            )
            check(
                answer.strip()[:24] in text,
                "the carrier really carries the SIDE ANSWER, not a summary of it",
                f"answer={answer[:60]!r} carrier={text[:200]!r}",
            )

    log(f"verdict: {len(FAILURES)} failure(s)" + (f": {FAILURES}" if FAILURES else ""))
    return 1 if FAILURES else 0


sys.exit(asyncio.run(main()))
