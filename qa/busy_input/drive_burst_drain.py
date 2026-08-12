#!/usr/bin/env python3
"""Real-machine QA for the busy-lane burst-drain wake edge (§4.8 Round-9).

A steer refused for backpressure parks in the busy lane. It is not waiting for
the run *slot* — the sibling it wants to steer must keep running for the steer
to mean anything — it is waiting for that sibling to *answer* its burst. The
wake edge for that is `wake_lane_if_burst_drained`; before Round-9 there was
none, and the documented "redelivers once the burst drains" was really the
30 s fallback tick.

Shape of the proof:

  1. wait until the log holds its first assistant_message — burst counting is
     live only from there (`count_pending_steering` returns 0 before any
     assistant turn, so steers sent earlier never reach the branch at all;
     this is the trap that made the first version of this script pass while
     testing nothing)
  2. steer B  -> injected, burst pending = 1 = the configured cap
  3. steer C  -> refused for backpressure, parks in the lane
  4. the next assistant_message is the drain edge under test

C can only be redelivered by: the run slot freeing (the run is still executing —
`run_finished` lands later, and every mock turn but the last ends in a tool_use
precisely to keep it that way), the 600 s fallback tick, or the drain edge. So
an injection seconds after that assistant_message, with the run still alive, is
the edge and nothing else.

Usage:  drive_burst_drain.py WS_URL DB [--hold SECONDS]
"""
import argparse
import asyncio
import sys
import time

import websockets

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from lib import SessionLog, log, reply, rpc  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("url")
ap.add_argument("db")
ap.add_argument("--hold", type=float, default=5.0)
args = ap.parse_args()


async def main():
    slog = SessionLog(args.db)
    async with websockets.connect(args.url, max_size=None) as ws:
        await rpc(ws, "connect", {"client_info": {"name": "qa-burst-drain"}}, 1)
        r = await reply(ws, 1)
        log("connect ->", r["result"]["role"])

        await rpc(
            ws,
            "chat.send",
            {"message": "long task, keep working", "channel": "gui:qa-burst"},
            2,
        )
        r = await reply(ws, 2)
        session_key, run_a = r["result"]["session_key"], r["result"]["run_id"]
        log(f"A accepted: run {run_a} session {session_key}")

        first = await slog.wait_for("assistant_message", 1, 180)
        if not first:
            log("FAIL: no assistant turn ever reached the session log")
            return 2
        log(f"first assistant_message committed (seq {first[0]}) — burst counting is live")

        await rpc(ws, "chat.send", {"message": "steer one", "session_key": session_key}, 3)
        b = await reply(ws, 3)
        log(f"steer #1 sent: run {b['result']['run_id']}")
        await asyncio.sleep(3)

        t_send_c = time.monotonic()
        await rpc(ws, "chat.send", {"message": "steer two", "session_key": session_key}, 4)
        c = await reply(ws, 4)
        run_c = c["result"]["run_id"]
        log(f"steer #2 sent: run {run_c}  <-- this is the one that must park")

        drain = await slog.wait_for("assistant_message", 2, 240)
        if not drain:
            log("FAIL: no second assistant turn (nothing could drain the burst)")
            return 2
        log(f"second assistant_message committed (seq {drain[0]}) — the drain edge fires NOW")

        # Hold past the drain: a few seconds is enough to see the wake, and the
        # fallback tick is 600 s, so anything this fast is the edge.
        await asyncio.sleep(args.hold)
        elapsed = time.monotonic() - t_send_c
        log(f"elapsed since steer #2 was sent: {elapsed:.1f}s (fallback tick is 600s)")
        log(f"RUN_C={run_c}")
        log(f"DRAIN_MS={drain[1]}")
        log("Inspect the server log for the injection of 'steer two': it must land "
            "within seconds of the drain above, while the run is still alive.")
        return 0


sys.exit(asyncio.run(main()))
