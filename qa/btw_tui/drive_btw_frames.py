#!/usr/bin/env python3
"""Real-machine QA for the frames the TUI's `/btw` overlay is built on.

The overlay in `interfaces/tui/src/tui/btw_overlay.rs` rests on four claims
about the wire, every one of which a fake backend would confirm by
construction because the fake returns what the code expects. Only a real
gateway can refute them:

  1. the `agent.run` reply for a `/btw` names the MAIN session key, not the
     derived side key — so `adopt_canonical_session_key` cannot repoint the
     screen at a session the user never opened;
  2. exactly ONE `stream.run_accepted` ever names the side run;
  3. that frame carries the DERIVED key (`…:ephemeral:btw-…`), which is why the
     overlay keys on the run id: this client cannot compute that key, and the
     screen's own cross-session guard will read every later side frame as
     foreign;
  4. later frames naming the side run do arrive on this connection, so the
     overlay has something to render rather than a spinner over silence.

(2) and (3) together are the live re-verification of the dual-`RunAccepted`
hazard: `AgentRunManager::start_run` emits a pre-dispatch `RunAccepted` with
the *routed* key, and `mark_run_session` is first-write-wins, so if both ever
reached one client the screen would believe the side run was its own.

Usage:  drive_btw_frames.py WS_URL
"""
import argparse
import asyncio
import sys
import time

import websockets

sys.path.insert(0, __file__.rsplit("/", 1)[0] + "/../busy_input")
from lib import log, reply, rpc  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("url")
ap.add_argument("--budget", type=float, default=180.0)
ap.add_argument(
    "--observe",
    type=float,
    default=45.0,
    help="how long to keep watching after the /btw is sent; must outlast one "
    "mock provider turn or the last assertion measures nothing",
)
args = ap.parse_args()

FAILURES = []


def check(ok, label, detail=""):
    log(("PASS " if ok else "FAIL ") + label + (f"  {detail}" if detail else ""))
    if not ok:
        FAILURES.append(label)


def frame_run_id(msg):
    p = msg.get("params") or {}
    return p.get("run_id")


async def main():
    async with websockets.connect(args.url, max_size=None) as ws:
        await rpc(ws, "connect", {"client_info": {"name": "qa-btw-frames"}}, 1)
        r = await reply(ws, 1)
        log("connect ->", r["result"]["role"])

        # A main run first: a side question exists to be asked WHILE one runs.
        await rpc(
            ws,
            "agent.run",
            {"input": "long task, keep working", "channel": "gui:qa-btw"},
            2,
        )
        r = await reply(ws, 2)
        main_key = r["result"]["session_key"]
        run_main = r["result"]["run_id"]
        log(f"main run {run_main} on {main_key}")

        # Let the main run actually get going.
        await asyncio.sleep(4)

        # The side question, sent exactly as `commands::send_to_agent` sends it.
        await rpc(
            ws,
            "agent.run",
            {"input": "/btw what is 2+2?", "session_key": main_key},
            3,
        )
        r = await reply(ws, 3)
        side_reply_key = r["result"]["session_key"]
        run_side = r["result"]["run_id"]
        log(f"side run {run_side}; reply named session {side_reply_key}")

        # (1) The reply names the MAIN key. If it named the side key, the TUI
        # would adopt it and every later keyed RPC would address a session the
        # user never opened.
        check(
            side_reply_key == main_key,
            "the agent.run reply for a /btw names the MAIN session key",
            f"reply={side_reply_key!r} main={main_key!r}",
        )
        check(
            run_side != run_main,
            "the side question is its own run",
            f"{run_side} != {run_main}",
        )

        accepted_for_side = []
        other_side_frames = []
        main_frames_after = 0
        # Observe for a FIXED window rather than breaking as soon as the side
        # frames arrive. The first version broke out 40 ms after sending the
        # `/btw` and then asserted the main run "kept streaming" — measuring
        # the probe's own impatience, not the server. The mock's turns take
        # tens of seconds, so the window has to outlast one of them for that
        # assertion to be about anything.
        observe_until = time.monotonic() + args.observe
        deadline = min(observe_until, time.monotonic() + args.budget)

        while time.monotonic() < deadline:
            try:
                raw = await asyncio.wait_for(ws.recv(), timeout=5)
            except asyncio.TimeoutError:
                continue
            import json

            msg = json.loads(raw)
            method = msg.get("method", "")
            rid = frame_run_id(msg)
            if rid == run_side:
                if method == "stream.run_accepted":
                    key = (msg.get("params") or {}).get("session_key")
                    accepted_for_side.append(key)
                    log(f"  run_accepted(side) session_key={key!r}")
                else:
                    other_side_frames.append(method)
                    if len(other_side_frames) <= 8:
                        log(f"  side frame: {method}")
            elif rid == run_main:
                main_frames_after += 1
                if main_frames_after <= 4:
                    log(f"  main frame: {method}")

        # (2) exactly one, and (3) it is the derived key.
        check(
            len(accepted_for_side) == 1,
            "exactly one run_accepted names the side run",
            f"got {len(accepted_for_side)}: {accepted_for_side}",
        )
        if accepted_for_side:
            key = accepted_for_side[0] or ""
            check(
                key != main_key and ":ephemeral:btw-" in key,
                "that frame carries the DERIVED side key, never the main one",
                f"key={key!r}",
            )
            check(
                main_key not in accepted_for_side,
                "no run_accepted for the side run ever carried the main key "
                "(the dual-RunAccepted hazard)",
                f"{accepted_for_side}",
            )

        # (4) the overlay has something to render.
        check(
            len(other_side_frames) >= 1,
            "further frames naming the side run reach this connection",
            f"{len(other_side_frames)} frames: {sorted(set(other_side_frames))}",
        )

        # And the main run was still producing while all that happened — the
        # side question answered BESIDE it, not instead of it.
        check(
            main_frames_after > 0,
            "the main run kept streaming while the side question ran",
            f"{main_frames_after} main frames in the {args.observe:.0f}s after the /btw",
        )

    log(f"verdict: {len(FAILURES)} failure(s)" + (f": {FAILURES}" if FAILURES else ""))
    return 1 if FAILURES else 0


sys.exit(asyncio.run(main()))
