#!/usr/bin/env python3
"""Drive one turn that leaves a tool call dangling, and PROVE it dangled.

An instrument that cannot show it produced the state it claims to test is not
an instrument. Two of the three modes below exist for exactly that:

  * `send` does not return until the durable event log actually holds a NEW
    `tool_call_requested` row — it polls the log itself rather than guessing
    a sleep, so `run.sh` can `kill -9` the server the instant the call is
    truly in flight and not a moment before.
  * `assert-dangling` reads the same log after the kill and fails loudly
    (`FAIL instrument`, exit 1) when fewer than `--min-count` calls are
    missing their `tool_result`/`tool_error` — that means the kill landed
    too late, or the call never dispatched at all, and every assertion a
    later stage makes would be passing over an empty set.

The third mode, `config-resume`, flips `[resume] enabled` in the generated
config in place, scoped to that one section (a global "first `enabled =` line
in the file" replacement would just as happily corrupt `[cron]` or
`[heartbeat]`'s own `enabled` key, both of which sit in the same generated
file) — mirrors `qa/busy_input/patch_config.py::set_key`'s section-scoped
rewrite, the pattern every other config-mutating fixture here already uses.

The event log itself: a single sqlite file at `<ALEPH_HOME>/data/sessions.db`
(see run.sh's `EVENTS_DB` comment for the source trail), table
`session_events`. Reads here always filter `retired_at IS NULL` — compaction
soft-deletes retired rows, and an unfiltered read would count events the
loader never actually returns to a resume scan.

Transport is the real one: JSON-RPC 2.0 over the gateway's WebSocket
(`connect` then `chat.send`), via the same `rpc`/`reply`/`log` helpers every
other fixture under qa/busy_input and qa/run_halt already uses.

Usage:
  drive_dangle.py --mode config-resume --config PATH --enabled true|false
  drive_dangle.py --mode send --port PORT --channel NAME --session-file PATH
                   --events-db PATH [--budget SECS]
  drive_dangle.py --mode assert-dangling --events-db PATH [--min-count N]
"""
import argparse
import asyncio
import json
import os
import re
import sqlite3
import sys
import time

import websockets

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "busy_input"))
from lib import log, reply, rpc  # noqa: E402


# --- durable event log ------------------------------------------------------


def _query(db_path, sql, args=()):
    """Read-only query, tolerant of "the server hasn't created the file yet"
    and "the server holds the write lock" — both are "no answer yet", never
    "no such event". Mirrors `qa/busy_input/lib.py::SessionLog._query`.
    """
    try:
        con = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True, timeout=5)
        try:
            return con.execute(sql, args).fetchall()
        finally:
            con.close()
    except sqlite3.Error:
        return []


def _call_ids(db_path, event_types):
    placeholders = ",".join("?" for _ in event_types)
    rows = _query(
        db_path,
        f"select payload_json from session_events "
        f"where event_type in ({placeholders}) and retired_at is null",
        tuple(event_types),
    )
    out = []
    for (raw,) in rows:
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            continue
        cid = payload.get("call_id")
        if cid:
            out.append(cid)
    return out


def requested_call_ids(db_path):
    return _call_ids(db_path, ["tool_call_requested"])


def dangling_call_ids(db_path):
    """Requested minus answered — `ToolError` counts as an answer too, same
    rule as `resume_coordinator::repairs_for` (a prior failure closes the
    call; only a truly unanswered one dangles)."""
    requested = requested_call_ids(db_path)
    answered = set(_call_ids(db_path, ["tool_result", "tool_error"]))
    return [c for c in requested if c not in answered]


# --- mode: config-resume -----------------------------------------------------


def mode_config_resume(args):
    with open(args.config) as fh:
        lines = fh.read().splitlines()

    out, cur, inserted = [], None, False
    header_re = re.compile(r"^\[+([^\]]+)\]+\s*$")
    for line in lines:
        m = header_re.match(line)
        if m:
            cur = m.group(1)
            out.append(line)
            if cur == "resume":
                out.append(f"enabled = {args.enabled}")
                inserted = True
            continue
        if cur == "resume" and re.match(r"^\s*enabled\s*=", line):
            continue  # superseded by the line inserted at the header above
        out.append(line)

    text = "\n".join(out) + "\n"
    if not inserted:
        text += f"\n[resume]\nenabled = {args.enabled}\n"

    with open(args.config, "w") as fh:
        fh.write(text)
    print(f"config-resume: [resume] enabled = {args.enabled}")
    return 0


# --- mode: send ---------------------------------------------------------------


def mode_send(args):
    baseline = set(requested_call_ids(args.events_db))

    session_key = None
    if args.session_file and os.path.exists(args.session_file):
        with open(args.session_file) as fh:
            session_key = fh.read().strip() or None

    async def go():
        async with websockets.connect(f"ws://127.0.0.1:{args.port}/ws", max_size=None) as ws:
            await rpc(ws, "connect", {"client_info": {"name": "qa-resume-boundary"}}, 1)
            r = await reply(ws, 1, budget=30)
            log("connect ->", (r.get("result") or {}).get("role"))

            params = {"message": "run the probe"}
            if session_key:
                params["session_key"] = session_key
            else:
                params["channel"] = args.channel
            await rpc(ws, "chat.send", params, 2)
            r = await reply(ws, 2, budget=60)
            if "error" in r:
                log(f"FAIL: chat.send error: {r['error']}")
                return 1

            result = r.get("result") or {}
            new_session_key = result.get("session_key")
            log(f"chat.send accepted: run={result.get('run_id')} session={new_session_key}")
            if new_session_key and args.session_file and not session_key:
                with open(args.session_file, "w") as fh:
                    fh.write(new_session_key)

            # Poll the durable log rather than sleeping a guess: the call is
            # confirmed dangling the instant its `tool_call_requested` row
            # lands, and not a moment before.
            end = time.monotonic() + args.budget
            while time.monotonic() < end:
                new_calls = set(requested_call_ids(args.events_db)) - baseline
                if new_calls:
                    log(f"ok: tool_call_requested landed: {sorted(new_calls)}")
                    return 0
                await asyncio.sleep(0.3)
            log(
                f"FAIL: no new tool_call_requested within {args.budget}s "
                f"(baseline={sorted(baseline)}) — the dangle was never created"
            )
            return 1

    return asyncio.run(go())


# --- mode: assert-dangling -----------------------------------------------------


def mode_assert_dangling(args):
    dangling = dangling_call_ids(args.events_db)
    if len(dangling) < args.min_count:
        print(
            f"FAIL instrument: expected at least {args.min_count} dangling call(s), "
            f"found {len(dangling)}: {dangling}. The kill landed too late, the tool "
            "call never dispatched, or a stale event log is being read.",
            file=sys.stderr,
        )
        return 1
    print(f"ok: {len(dangling)} dangling call(s): {dangling}")
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", required=True, choices=["config-resume", "send", "assert-dangling"])
    ap.add_argument("--config")
    ap.add_argument("--enabled", choices=["true", "false"])
    ap.add_argument("--port", type=int)
    ap.add_argument("--channel", default="gui:qa-resume-boundary")
    ap.add_argument("--session-file")
    ap.add_argument("--events-db")
    ap.add_argument("--budget", type=float, default=30.0)
    ap.add_argument("--min-count", type=int, default=1)
    args = ap.parse_args()

    if args.mode == "config-resume":
        if not args.config or args.enabled is None:
            print("config-resume needs --config and --enabled", file=sys.stderr)
            return 2
        return mode_config_resume(args)
    if args.mode == "send":
        if not args.port or not args.events_db:
            print("send needs --port and --events-db", file=sys.stderr)
            return 2
        return mode_send(args)
    if args.mode == "assert-dangling":
        if not args.events_db:
            print("assert-dangling needs --events-db", file=sys.stderr)
            return 2
        return mode_assert_dangling(args)
    return 2


if __name__ == "__main__":
    sys.exit(main())
