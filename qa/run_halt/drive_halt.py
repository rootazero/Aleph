#!/usr/bin/env python3
"""Real-machine QA for what a run says about how it ended (§3.17c/d).

The claim is a *wire* one and every layer under it already had passing tests
while the chain was broken: the terminal settle sat below the error `?`, so a
run that did N turns and burned tokens before failing emitted a synthesized
`FlowOutcome::default()` — and `TerminateReason`'s `Default` is `Completed`.
The receipt read `completed`, 0 loops, 0 tokens, no cost, no tool timeline, on
every one of the four surfaces that render it.

What makes it observable is exactly one frame: `run_complete`'s `summary`. So
this driver collects frames off the socket and asserts against that object,
not against anything's rendering of it.

Two scenarios, on the two arms of the flow:

  crash  the provider refuses mid-run. The failure arm. Claims: a
         `run_complete` arrives at all; it says `failed`; and it still carries
         the loops / tool calls / tokens the run really spent.
  cap    `max_iterations` trips. The success arm, and the one that exercises
         `terminate_detail`: the escalation folds the cap into the
         `budget_exhausted_partial_result` umbrella, and the cap itself
         survives only in `terminate_detail`. Three of the five rendering
         surfaces used to read `terminate_reason` alone.

Usage:  drive_halt.py WS_URL SCENARIO [BUDGET_SECS]
"""
import asyncio
import json
import os
import sys
import time

import websockets

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "busy_input"))
from lib import log, reply, rpc  # noqa: E402

WS_URL = sys.argv[1]
SCENARIO = sys.argv[2] if len(sys.argv) > 2 else "crash"
BUDGET = int(sys.argv[3]) if len(sys.argv) > 3 else 120
# Seconds to keep listening after the first terminal frame. A retried
# dispatch takes a provider round trip to come back, and against a local
# mock that is milliseconds — but the retry ladder also backs off, so this
# is generous rather than tight.
QUIET_AFTER = int(os.environ.get("QUIET_AFTER", "12"))

PASS, FAIL = [], []


def check(ok, claim, detail=""):
    """Record one claim.

    `detail` is built by the caller FROM WHAT IT SAW — a string formatted
    before the comparison reads as true on a pass, a trap this fixture family
    has paid for once already.
    """
    (PASS if ok else FAIL).append(claim)
    log(f"[{'PASS' if ok else 'FAIL'}] {claim}" + (f" — {detail}" if detail else ""))
    return ok


def effective_token(summary):
    """`aleph_protocol::terminate::effective_token`, in Python.

    Deliberately a second implementation and deliberately three lines: this is
    a fixture asserting the *wire*, so reaching for the Rust one would mean
    asserting that a function agrees with itself. What is being pinned here is
    that core really puts the granular cap on the wire — whether the clients
    then prefer it is the unit tests' question, and they answer it.
    """
    reason = (summary.get("terminate_reason") or "").strip()
    detail = (summary.get("terminate_detail") or "").strip()
    if not reason or reason == "completed":
        return None
    return detail or reason


async def collect(ws, run_id, budget, quiet_after_terminal):
    """Frames for `run_id`, and then `quiet_after_terminal` more seconds.

    Deliberately NOT "stop at the first terminal frame", which is what the
    first version of this driver did and what makes the most interesting claim
    unaskable. §3.17c moved the terminal settle ABOVE `run_result.map_err(..)?`
    so a failed run settles at all — and `run_loop/inner.rs` retries a dispatch
    it classifies `Transient`, which a 401 used to be, while the failover layer
    called the same error `Permanent` two log lines earlier. A settle that runs
    on the failure arm, inside something the caller retries, is one terminal
    frame per attempt unless something stops it.

    Two things now stop it (2026-08-29): the two classifiers agree, so a 401 is
    not retried at all; and a transient attempt that WILL be retried has its
    terminal frame withheld rather than broadcast. Neither is asserted by
    reading the code — the claim below counts frames on the wire, which is why
    this function still listens past the first one. Stop listening early and
    `len(completes) == 1` becomes a tautology.
    """
    frames = []
    end = time.monotonic() + budget
    settled_at = None
    while time.monotonic() < end:
        if settled_at and time.monotonic() - settled_at > quiet_after_terminal:
            break
        remaining = max(0.1, end - time.monotonic())
        try:
            raw = await asyncio.wait_for(ws.recv(), timeout=min(remaining, 1.0))
        except asyncio.TimeoutError:
            continue
        msg = json.loads(raw)
        params = msg.get("params") or {}
        method = msg.get("method") or ""
        if not method.startswith("stream."):
            continue
        if params.get("run_id") not in (run_id, None):
            continue
        frames.append((method, params))
        if method == "stream.run_complete" and settled_at is None:
            settled_at = time.monotonic()
    return frames


async def main():
    async with websockets.connect(WS_URL, max_size=None) as ws:
        await rpc(ws, "connect", {"client_info": {"name": "qa-run-halt"}}, 1)
        r = await reply(ws, 1)
        log("connect ->", r["result"]["role"])

        await rpc(
            ws,
            "chat.send",
            {"message": "do some work", "channel": "gui:qa-run-halt"},
            2,
        )
        r = await reply(ws, 2)
        run_id = r["result"]["run_id"]
        log(f"run accepted: {run_id}")

        frames = await collect(ws, run_id, BUDGET, QUIET_AFTER)
        kinds = [m for m, _ in frames]
        log(f"frames: {kinds}")

        completes = [p for m, p in frames if m == "stream.run_complete"]
        if not check(
            bool(completes),
            "a run that ends non-cleanly still broadcasts a terminal summary",
            f"frames seen: {kinds}",
        ):
            return report()

        # Ordered first because it explains the three below: every client keeps
        # the LAST terminal frame it saw, so if there are three, the run's
        # receipt is whatever the last one says.
        check(
            len(completes) == 1,
            "a run broadcasts exactly one terminal summary",
            f"{len(completes)} run_complete frames: "
            + json.dumps(
                [
                    {
                        "terminate_reason": (c.get("summary") or {}).get("terminate_reason"),
                        "loops": (c.get("summary") or {}).get("loops"),
                        "total_tokens": (c.get("summary") or {}).get("total_tokens"),
                    }
                    for c in completes
                ]
            ),
        )

        # The LAST frame, deliberately: that is the one every client keeps, so
        # it is the one the user's receipt is built from. Reading `completes[0]`
        # here would assert something true about the wire and false about the
        # screen — and on the failure arm today those are different objects.
        summary = completes[-1].get("summary") or {}
        log("summary (last frame): " + json.dumps(summary, ensure_ascii=False)[:600])
        if len(completes) > 1:
            log("summary (first frame): "
                + json.dumps(completes[0].get("summary") or {}, ensure_ascii=False)[:400])
        token = effective_token(summary)

        if SCENARIO == "crash":
            check(
                summary.get("terminate_reason") == "failed",
                "a failed run reports `failed`, not the enum's Default",
                f"terminate_reason={summary.get('terminate_reason')!r}",
            )
            # The whole point of moving the settle above the error return. Each
            # is asserted separately: a single "the summary is non-empty" check
            # passes on a summary that carries one of the three.
            check(
                (summary.get("loops") or 0) > 0,
                "the receipt carries the loops the run really spent",
                f"loops={summary.get('loops')}",
            )
            check(
                (summary.get("tool_calls") or 0) > 0,
                "the receipt carries the tool calls the run really made",
                f"tool_calls={summary.get('tool_calls')}",
            )
            check(
                (summary.get("total_tokens") or 0) > 0,
                "the receipt carries the tokens the run really burned",
                f"total_tokens={summary.get('total_tokens')}",
            )
        else:
            check(
                token is not None,
                "a capped run is not reported as a clean finish",
                f"terminate_reason={summary.get('terminate_reason')!r}",
            )
            # The `cap` scenario exists for this line. If the escalation fired,
            # `terminate_reason` is the umbrella and only `terminate_detail`
            # says which budget was hit; if it did not, the reason is already
            # granular. Either way the token a surface should RENDER is the
            # cap, and that is what the shared table is keyed on.
            check(
                token == "hit_max_iterations",
                "the token a surface renders names the cap, not the umbrella",
                f"reason={summary.get('terminate_reason')!r} "
                f"detail={summary.get('terminate_detail')!r} -> {token!r}",
            )
            check(
                (summary.get("total_tokens") or 0) > 0,
                "the receipt carries the tokens the run burned before the cap",
                f"total_tokens={summary.get('total_tokens')}",
            )
        return report()


def report():
    log(f"\n{len(PASS)} passed, {len(FAIL)} failed")
    for f in FAIL:
        log("  FAILED:", f)
    return len(FAIL)


if __name__ == "__main__":
    sys.exit(asyncio.run(main()) or 0)
