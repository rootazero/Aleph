#!/usr/bin/env python3
"""Real-machine QA for the background-`bash` completion announce.

The claim under test is a *runtime* one and a unit test cannot reach it: a
backgrounded command that finishes AFTER its run has ended used to finish into
an empty room. `ProcessRegistry::complete` wrote the entry, woke whoever
happened to be parked in `wait`, and stopped — so the model's "I'll report back
when the build is done" was silently broken.

What makes this observable at all is that the cure spends a **provider turn
nobody's client asked for**. So the oracle is the mock's `observations.jsonl`:
a request whose newest user text carries `[system] Background process N
finished`, arriving after the run that spawned the job is already `run_finished`.

Three scenarios, and the second and third exist because the first one's claim
is satisfied too easily:

  outlive    the flagship — job outlives the run, a fresh run is driven
  collected  the model collected the job itself, so no turn is spent. Paired
             with a PRESENCE check (the job really did finish and its output
             really did reach the model through `wait`), because "no announce"
             is otherwise satisfied by "the job never completed".
  midrun     the run is still alive when the job lands, so the notice is
             absorbed into it at a turn boundary — ONE run, not two.

Usage:  drive_announce.py WS_URL DB OBS SCENARIO SLEEP_SECS
"""
import asyncio
import json
import os
import sys
import time

import websockets

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "busy_input"))
from lib import SessionLog, log, reply, rpc  # noqa: E402

WS_URL, DB, OBS, SCENARIO = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
SLEEP_SECS = int(sys.argv[5]) if len(sys.argv) > 5 else 12
MARKER = "QA_ANNOUNCE_MARKER_7f3a"

PASS, FAIL = [], []


def check(ok, claim, detail=""):
    """Record one claim. `detail` is built by the caller FROM WHAT IT SAW —
    a string formatted before the comparison lies on a pass (a trap this
    fixture family has paid for once already)."""
    (PASS if ok else FAIL).append(claim)
    log(f"[{'PASS' if ok else 'FAIL'}] {claim}" + (f" — {detail}" if detail else ""))
    return ok


def observations():
    """Every record the mock has written so far, oldest first."""
    out = []
    try:
        with open(OBS) as fh:
            for line in fh:
                line = line.strip()
                if line:
                    try:
                        out.append(json.loads(line))
                    except json.JSONDecodeError:
                        pass
    except FileNotFoundError:
        pass
    return out


def turns():
    return [o for o in observations() if o.get("kind") == "turn"]


async def wait_for_announce(budget):
    """The first turn whose newest user text carries the notice, or None."""
    end = time.monotonic() + budget
    while time.monotonic() < end:
        for o in turns():
            if o.get("carries_announce"):
                return o
        await asyncio.sleep(0.5)
    return None


async def main():
    slog = SessionLog(DB)
    async with websockets.connect(WS_URL, max_size=None) as ws:
        await rpc(ws, "connect", {"client_info": {"name": "qa-announce"}}, 1)
        r = await reply(ws, 1)
        log("connect ->", r["result"]["role"])

        t0 = time.monotonic()
        await rpc(
            ws,
            "chat.send",
            {
                "message": "kick off the long job and report back when it lands",
                "channel": "gui:qa-announce",
            },
            2,
        )
        r = await reply(ws, 2)
        session_key, run_a = r["result"]["session_key"], r["result"]["run_id"]
        log(f"run A accepted: {run_a} in session {session_key}")
        # NOT `slog.session_id = session_key`. The `session_events.session_id`
        # column holds a serialized SessionId JSON blob
        # (`{"type":"main","agent_id":"main",...}`), not the `session_key`
        # string `chat.send` hands back, so scoping by it silently matches
        # nothing and every `wait_for` below times out reporting "the run never
        # finished" about a run that finished in 80 ms. The fixture runs one
        # session, so unscoped is both correct and honest.

        # --- the spawn actually happened -----------------------------------
        spawned = False
        for _ in range(120):
            if turns():
                spawned = True
                break
            await asyncio.sleep(0.5)
        if not check(spawned, "the model's first turn reached the provider",
                     f"{len(turns())} turn(s) observed"):
            return 2

        # --- CONTROL, in the same process ----------------------------------
        # Every plan opens with a FOREGROUND `bash`. Without this, "the
        # background job failed" and "bash cannot run in this fixture at all"
        # are the same observation — which is exactly how the first run of this
        # scenario read.
        fg = None
        for _ in range(120):
            fg = next((o for o in turns() if o.get("fg_control_ok")), None)
            if fg:
                break
            await asyncio.sleep(0.5)
        if not check(
            fg is not None,
            "CONTROL: a foreground `bash` ran and its output reached the model",
            f"{len(turns())} turns seen; last tool_results="
            f"{turns()[-1].get('tool_results') if turns() else 'none'}",
        ):
            blocked = any(
                "cwd outside workspace root" in json.dumps(o.get("tool_results", []))
                for o in turns()
            )
            if blocked:
                log("")
                log("  DIAGNOSIS: `bash` was refused before it ran, so nothing below")
                log("  this line could have been tested. Two subsystems answer")
                log("  \"where does this session work\" and they never agree:")
                log("    * tools/adapters/registry_adapter.rs injects")
                log("      `default_working_dir` into every bash/code_exec call that")
                log("      omits `working_dir`. Its value is `effective_workspace`")
                log("      (run_loop/inner.rs) = the AGENT workspace,")
                log("      `~/.aleph/workspaces/<agent_id>`.")
                log("    * sandbox/workspace/mod.rs::for_session puts the session's")
                log("      cwd at `~/.aleph/workspaces/<sha256(session_id)[..16]>`,")
                log("      and refuses any cwd outside it.")
                log("  A 32-hex directory name is never the agent id, so the")
                log("  injected path is always outside — and the refusal reads as")
                log("  a sandbox policy decision rather than as a wiring bug.")
                log("  Passing NO working_dir would land in the session workspace")
                log("  (`cwd: None` skips the containment check entirely); it is")
                log("  the injection that creates the violation.")
            return 2

        if SCENARIO == "outlive":
            fin = await slog.wait_for("run_finished", 1, 120)
            if not check(fin is not None, "run A finished on its own"):
                return 2
            elapsed = time.monotonic() - t0
            check(
                elapsed < SLEEP_SECS,
                "run A ended while the job was still running",
                f"run finished {elapsed:.1f}s in, job sleeps {SLEEP_SECS}s",
            )

            ann = await wait_for_announce(SLEEP_SECS + 90)
            if not check(ann is not None,
                         "a provider turn arrived carrying the completion notice",
                         f"{len(turns())} turns seen"):
                return 2
            check(
                ann.get("carries_marker", False),
                "the notice carried the job's own output, not just a ping",
                f"marker {MARKER!r} " + ("present" if ann.get("carries_marker") else "ABSENT"),
            )

            runs = slog.runs()
            check(
                len(runs) >= 2,
                "the announce opened a SECOND run in the same session",
                f"{len(runs)} run(s) in {session_key}: {runs}",
            )

        elif SCENARIO == "collected":
            # Presence first: the job must really have finished and its output
            # must really have reached the model — otherwise "no announce" is
            # a vacuous pass.
            collected = None
            end = time.monotonic() + SLEEP_SECS + 90
            while time.monotonic() < end and collected is None:
                for o in turns():
                    if MARKER in (o.get("last_user_text") or ""):
                        collected = o
                        break
                await asyncio.sleep(0.5)
            if not check(
                collected is not None,
                "the model collected the job itself (its output came back through `wait`)",
                f"{len(turns())} turns seen",
            ):
                return 2

            # Now the absence, over a window that comfortably covers the
            # ladder's first two rungs (immediate, then +30s).
            log("holding 45s to see whether a turn is spent re-stating it...")
            ann = await wait_for_announce(45)
            check(
                ann is None,
                "no turn was spent re-announcing a result the model already had",
                "no announce turn observed" if ann is None else f"announce at turn {ann.get('turn')}",
            )

        elif SCENARIO == "midrun":
            ann = await wait_for_announce(SLEEP_SECS + 120)
            if not check(ann is not None,
                         "the completion notice reached the model",
                         f"{len(turns())} turns seen"):
                return 2

            prev = [o for o in turns() if o.get("turn", 0) < ann.get("turn", 0)]
            check(
                bool(prev) and ann.get("messages", 0) > prev[-1].get("messages", 0),
                "the notice was APPENDED to the live conversation, not given a fresh one",
                f"turn {ann.get('turn')} carried {ann.get('messages')} messages, "
                f"previous turn carried {prev[-1].get('messages') if prev else 'n/a'}",
            )

            runs = slog.runs()
            check(
                len(runs) == 1,
                "no second run was opened — the notice was absorbed as steering",
                f"{len(runs)} run(s): {runs}",
            )

        else:
            log(f"unknown scenario {SCENARIO}")
            return 64

    log("")
    log(f"=== {len(PASS)}/{len(PASS) + len(FAIL)} claims passed ===")
    for f in FAIL:
        log(f"  FAILED: {f}")
    return 0 if not FAIL else 1


sys.exit(asyncio.run(main()))
