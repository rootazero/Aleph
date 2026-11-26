#!/usr/bin/env python3
"""Real-machine QA for per-channel `busy_input_mode` (§4.8).

`busy_input_mode` is reachable from exactly one direction: a channel arrival.
The RPC face never carries it (`chat.send` has no such parameter — deliberately,
see CLAUDE.md "三根会话旋钮"), so every earlier round could only unit-test it.
The generic webhook channel closes that gap: an HMAC-signed POST is a genuine
inbound message, and `subsystems.rs` overlays the channel's policy block onto
the `ChannelConfig` the executor reads.

What is proved here:

  interrupt   a message arriving while a run is live CANCELS that run — i.e.
              the mode travelled config -> ChannelConfig -> run metadata ->
              engine busy branch, end to end.
  queue       the same arrival cancels NOTHING.

What is deliberately NOT claimed here: that a *tight* burst of interrupts does
not eat itself (Round-8 ①). The coalescer sits in front of the busy lane with an
800 ms debounce / 200 ms early-flush window, so two messages from one
conversation cannot reach the engine closer together than that — far longer than
the admission the race needs to beat. From a channel surface each interrupt
therefore targets a run that genuinely was already going, and cancelling it is
correct. The tight case stays unit-covered
(`steering::tests::a_burst_of_interrupts_does_not_eat_itself`). This script
reports the cancellation count so the interpretation is visible rather than
assumed.

Pairing: generic channels are hardcoded to `DmPolicy::Pairing` (the flat-key
`ChannelPolicyConfig` cannot set `dm_policy`), so the scenario performs the real
operator handshake — `channel.pairing.list` then `channel.pairing.approve` —
rather than pretending the gate is not there.

Usage:
  drive_channel_busy.py WS_URL BASE_URL DB SECRET PATH MODE [--spacing S]
"""
import argparse
import asyncio
import json
import sys

import websockets

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from lib import SessionLog, channel_message, log, reply, rpc, webhook_post  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("ws_url")
ap.add_argument("base_url")
ap.add_argument("db")
ap.add_argument("secret")
ap.add_argument("path")
ap.add_argument("mode", choices=["interrupt", "queue", "steer"])
ap.add_argument("--hold", type=float, default=30.0)
ap.add_argument("--burst", type=int, default=3)
ap.add_argument(
    "--spacing",
    type=float,
    default=1.2,
    help="seconds between burst messages; must exceed the coalescer's 800ms "
    "debounce or the burst arrives as ONE merged message",
)
ap.add_argument("--conversation", default="qa-busy-conv")
ap.add_argument(
    "--request-log",
    default="",
    help="the mock provider's JSONL request log; steer mode reads it to prove "
    "the arrivals reached the live run's next provider request",
)
args = ap.parse_args()

CONV = args.conversation
_seq = [0]


BURST_WORDS = "bravo charlie delta echo".split()


def burst_text(i):
    return f"{BURST_WORDS[i % 4]}: burst message {i + 2}"


def requests_carrying(text):
    """Run-turn indices of logged provider requests whose `messages` contain
    `text`. Planning calls are skipped: they are not the run's own turns."""
    hits = []
    with open(args.request_log) as fh:
        for line in fh:
            entry = json.loads(line)
            if entry.get("is_planning"):
                continue
            messages = (entry.get("body") or {}).get("messages", [])
            if text in json.dumps(messages, ensure_ascii=False):
                hits.append(entry.get("run_turn"))
    return hits


async def wait_for_injection(texts, budget):
    """Poll the request log until every text has reached some run request.
    Returns `{text: [run_turn, ...]}` for whatever arrived within `budget`."""
    found = {}
    for _ in range(int(budget)):
        found = {t: requests_carrying(t) for t in texts}
        if all(found.values()):
            break
        await asyncio.sleep(1)
    return found


def send(text):
    _seq[0] += 1
    status, body = webhook_post(
        args.base_url,
        args.path,
        args.secret,
        channel_message(text, CONV, message_id=f"qa-{_seq[0]}"),
    )
    log(f"POST {text!r} -> HTTP {status} {body.strip()[:80]}")
    return status


async def pair(ws):
    """Complete the operator half of the pairing handshake for our sender."""
    for attempt in range(30):
        await rpc(ws, "channel.pairing.list", {}, 100 + attempt)
        r = await reply(ws, 100 + attempt)
        pending = (r.get("result") or {}).get("requests") or []
        if not isinstance(pending, list):
            pending = []
        mine = [p for p in pending if p.get("channel") == "webhook"]
        if mine:
            code = mine[0]["code"]
            log(f"pairing request found: {mine[0].get('sender_id')} code={code}")
            await rpc(ws, "channel.pairing.approve", {"channel": "webhook", "code": code}, 200)
            r = await reply(ws, 200)
            if r.get("error"):
                log(f"FAIL: pairing approve rejected: {r['error']}")
                return False
            log("pairing approved")
            return True
        await asyncio.sleep(1)
    log("FAIL: no pairing request ever appeared")
    return False


async def main():
    slog = SessionLog(args.db)

    async with websockets.connect(args.ws_url, max_size=None) as ws:
        await rpc(ws, "connect", {"client_info": {"name": "qa-channel-busy"}}, 1)
        r = await reply(ws, 1)
        log("connect ->", (r.get("result") or {}).get("role"))

        # --- 1. first contact opens a pairing request, not a run ------------
        if send("hello, pairing please") not in (200, 202):
            log("FAIL: the channel refused the first message")
            return 2
        if not await pair(ws):
            return 2

        # --- 2. now a real message starts a real run -----------------------
        if send("alpha: long task, keep working") not in (200, 202):
            log("FAIL: the channel refused the post-pairing message")
            return 2

        started = await slog.wait_for("run_started", 1, 120)
        if not started:
            log("FAIL: no run_started — the message never reached the engine")
            return 2
        log(f"run_started #1 (seq {started[0]}); sessions: {slog.sessions()}")

        first = await slog.wait_for("assistant_message", 1, 120)
        if not first:
            log("FAIL: no assistant turn (provider unreachable?)")
            return 2
        log(f"first assistant_message (seq {first[0]}) — run is live mid-loop")

        runs_before = slog.runs()
        first_run = next(iter(runs_before), None)
        log(f"run under test: {first_run} (outcomes so far: {runs_before})")

        # --- 3. arrivals while that run is live ----------------------------
        for i in range(args.burst):
            send(burst_text(i))
            if i + 1 < args.burst:
                await asyncio.sleep(args.spacing)

        log(f"{args.burst} arrival(s) sent {args.spacing}s apart; "
            f"holding {args.hold}s for the engine to settle")
        await asyncio.sleep(args.hold)

        # --- 4. verdict, read from the engine's own record ------------------
        runs = slog.runs()
        cancelled = [r for r, o in runs.items() if o == "cancelled"]
        log(f"runs: {runs}")
        log(f"cancelled: {len(cancelled)} of {len(runs)} -> {cancelled}")

        if args.mode == "interrupt":
            if not cancelled:
                log("FAIL: interrupt mode cancelled nothing — either the mode never "
                    "reached the executor, or Interrupt degraded to Queue")
                return 1
            if first_run and first_run not in cancelled:
                log(f"FAIL: the cancelled run(s) {cancelled} do not include the one "
                    f"that predated the arrivals ({first_run})")
                return 1
            log(f"PASS: interrupt reached the engine — the live run {first_run} was "
                f"cancelled by a channel arrival ({len(cancelled)} cancellation(s) "
                f"for {args.burst} arrival(s), spaced {args.spacing}s)")
            return 0

        if args.mode in ("queue", "steer"):
            if cancelled:
                log(f"FAIL: {args.mode} mode must never cancel, but {cancelled} was")
                return 1
            log(f"PASS: {len(runs)} run(s), zero cancellations under {args.mode}")

        if args.mode == "queue":
            return 0

        # steer: the arrivals must join the LIVE run, not start their own.
        # Zero cancellations alone is also what a queue would show, so the two
        # claims below are what make this a steer test.
        if len(runs) != 1:
            log(f"FAIL: steer must keep one run, but the engine recorded {runs}")
            return 1
        if not args.request_log:
            log("FAIL: steer needs --request-log to prove the injection")
            return 1
        texts = [burst_text(i) for i in range(args.burst)]
        found = await wait_for_injection(texts, 120)
        missing = [t for t, turns in found.items() if not turns]
        if missing:
            log(f"FAIL: steered text never reached a provider request: {missing}")
            return 1
        late = [t for t, turns in found.items() if min(turns) <= 1]
        if late:
            log(f"FAIL: steered text {late} was already in the first run turn, "
                "so it was not injected mid-run")
            return 1
        if len(slog.runs()) != 1:
            log(f"FAIL: a second run appeared while waiting: {slog.runs()}")
            return 1
        log(f"PASS: every steered message reached run {first_run}'s later "
            f"provider requests (first run turn carrying each: "
            f"{ {t: min(n) for t, n in found.items()} })")
        return 0

    return 1


sys.exit(asyncio.run(main()))
