#!/usr/bin/env python3
"""Driver for qa/thread_continuity. One stage, one phase per invocation.

Usage:  drive.py STAGE PHASE WS_URL DB REQUEST_LOG STATE

  STAGE        reopen | checklist | two_clients | fork
  PHASE        before | after (stages that restart the server) | only
  REQUEST_LOG  the mock provider's request log (`scripted` plan): one JSON
               object per request, `body` = what the model was sent
  STATE        JSON file carrying facts from `before` to `after`

Exit code = number of failed claims.
"""
import asyncio
import json
import sys
import time

import websockets

sys.path.insert(0, __file__.rsplit("/", 1)[0] + "/../busy_input")
from lib import SessionLog, log, reply, rpc  # noqa: E402

STAGE, PHASE, WS_URL, DB, REQUEST_LOG, STATE = sys.argv[1:7]

failures = []


def check(ok, claim, evidence=""):
    """Record one claim. Never raises: report every failure, not the first."""
    log(("  PASS  " if ok else "  FAIL  ") + claim)
    if evidence:
        log(f"          {evidence}")
    if not ok:
        failures.append(claim)
    return ok


def load_state():
    try:
        with open(STATE) as f:
            return json.load(f)
    except FileNotFoundError:
        return {}


def save_state(state):
    with open(STATE, "w") as f:
        json.dump(state, f, indent=2)


def requests():
    """Every request the mock provider received, oldest first."""
    out = []
    try:
        with open(REQUEST_LOG) as f:
            for line in f:
                if line.strip():
                    out.append(json.loads(line))
    except FileNotFoundError:
        pass
    return out


def message_text(message):
    content = message.get("content")
    if isinstance(content, str):
        return content
    parts = []
    for block in content or []:
        if not isinstance(block, dict):
            continue
        if block.get("type") == "text":
            parts.append(block.get("text", ""))
        elif block.get("type") == "tool_result":
            inner = block.get("content")
            if isinstance(inner, list):
                parts.extend(x.get("text", "") for x in inner if isinstance(x, dict))
            elif isinstance(inner, str):
                parts.append(inner)
        elif block.get("type") == "tool_use":
            parts.append(json.dumps(block.get("input")))
    return " ".join(parts)


def run_requests_carrying(marker):
    """Run requests (ones with a tool surface) whose messages contain `marker`.

    Attribution by CONTENT, never by position: side-channel calls and other
    sessions' turns interleave in the log.
    """
    return [
        r
        for r in requests()
        if r["body"].get("tools")
        and any(marker in message_text(m) for m in r["body"].get("messages", []))
    ]


class Client:
    """One WebSocket client with its own request ids."""

    def __init__(self, ws, name):
        self.ws = ws
        self.name = name
        self.rid = 1000

    async def call(self, method, params, budget=60):
        self.rid += 1
        await rpc(self.ws, method, params, self.rid)
        return await reply(self.ws, self.rid, budget)


async def connect(name):
    ws = await websockets.connect(WS_URL, max_size=None)
    client = Client(ws, name)
    r = await client.call("connect", {"client_info": {"name": name}})
    if "result" not in r:
        raise SystemExit(f"connect refused: {r}")
    return client


async def wait_run_finished(run_id, budget=180):
    """Block until `run_id` has a `run_finished` row. Returns its outcome."""
    slog = SessionLog(DB)
    end = time.monotonic() + budget
    while time.monotonic() < end:
        runs = slog.runs()
        if runs.get(run_id) is not None:
            return runs[run_id]
        await asyncio.sleep(0.3)
    return None


async def send(client, message, session_key=None, **extra):
    params = {"message": message, **extra}
    if session_key:
        params["session_key"] = session_key
    else:
        params["channel"] = f"gui:qa-thread-{STAGE}"
    r = await client.call("chat.send", params)
    if "result" not in r:
        check(False, f"chat.send accepted {message!r}", json.dumps(r)[:300])
        return None, None
    return r["result"]["session_key"], r["result"]["run_id"]


def run_envelope(run_id):
    """The `RunStarted` envelope for `run_id`: what the run actually ran under."""
    for p in SessionLog(DB).payloads("run_started"):
        if p.get("run_id") == run_id:
            return p.get("envelope") or {}
    return None


async def history_session(client, session_key):
    r = await client.call("chat.history", {"session_key": session_key})
    return (r.get("result") or {}).get("session") or {}, r.get("result") or {}


async def session_usage(client, session_key):
    r = await client.call("session.usage", {"session_key": session_key})
    return r.get("result") or {"error": r.get("error")}


# --- reopen (F1) -------------------------------------------------------------

KNOBS = {"session_mode": "code", "exec_tier": "auto", "think_level": "high"}
PIN = "qa-mock-model-b"
# The snapshot fields a reopened client paints: knobs, pin, and the counters.
SNAPSHOT_FIELDS = [
    "mode",
    "exec_tier",
    "think_level",
    "memory_mode",
    "model_pin",
    "model_pin_provider",
    "model",
    "input_tokens",
    "output_tokens",
    "total_tokens",
]
USAGE_FIELDS = ["tokens", "input_tokens", "output_tokens", "messages", "cost_usd"]


def last_request_for(marker):
    matches = run_requests_carrying(marker)
    return matches[-1]["body"] if matches else None


def check_run_used_knobs(run_id, marker, label):
    env = run_envelope(run_id)
    check(
        env is not None and env.get("exec_tier") == "auto",
        f"{label}: the run's RunStarted envelope has exec_tier auto",
        f"envelope={json.dumps(env)[:400]}",
    )
    check(
        env is not None and env.get("think_level") == "high",
        f"{label}: the run's RunStarted envelope has think_level high",
    )
    body = last_request_for(marker)
    check(
        body is not None and body.get("model") == PIN,
        f"{label}: the provider request uses the pinned model {PIN}",
        f"model={(body or {}).get('model')!r}",
    )
    return body


async def reopen_before():
    client = await connect("qa-thread-reopen-before")
    session_key, run_id = await send(client, "QA_REOPEN_ONE pin the model")
    if not run_id:
        return
    outcome = await wait_run_finished(run_id)
    check(outcome is not None, "the pinning run finished", f"outcome={outcome!r}")

    r = await client.call("sessions.patch", {"session_key": session_key, "metadata": KNOBS})
    check("result" in r, "sessions.patch accepts mode/exec_tier/think_level", json.dumps(r)[:300])

    _, run_id = await send(client, "QA_REOPEN_TWO after the patch", session_key)
    outcome = await wait_run_finished(run_id)
    check(outcome is not None, "the post-patch run finished", f"outcome={outcome!r}")
    body = check_run_used_knobs(run_id, "QA_REOPEN_TWO", "before restart")
    # The control arm. The first run went out before the patch, so its request
    # must differ from the patched one in thinking — otherwise "the same
    # thinking setting after the restart" below would hold for a knob that
    # never reached the provider at all. The FIRST matching request, because
    # later turns of this session also carry the marker in their history.
    unpatched = (run_requests_carrying("QA_REOPEN_ONE") or [{"body": {}}])[0]["body"]
    check(
        body is not None and unpatched.get("thinking") != body.get("thinking"),
        "CONTROL: think_level high changes the provider request's thinking field",
        f"unpatched={unpatched.get('thinking')!r} patched={(body or {}).get('thinking')!r}",
    )

    session, _ = await history_session(client, session_key)
    usage = await session_usage(client, session_key)
    log(f"session snapshot: {json.dumps({k: session.get(k) for k in SNAPSHOT_FIELDS})}")
    log(f"session.usage: {json.dumps(usage)}")
    check(session.get("mode") == "code", "the snapshot reports mode code", f"mode={session.get('mode')!r}")
    check(session.get("model_pin") == PIN, f"the snapshot reports model_pin {PIN}", f"model_pin={session.get('model_pin')!r}")
    check(
        (session.get("input_tokens") or 0) > 0,
        "the snapshot carries non-zero cumulative input tokens",
        f"input_tokens={session.get('input_tokens')!r}",
    )
    save_state(
        {
            "session_key": session_key,
            "session": session,
            "usage": usage,
            "thinking": (body or {}).get("thinking"),
        }
    )
    await client.ws.close()


async def reopen_after():
    state = load_state()
    session_key = state["session_key"]
    client = await connect("qa-thread-reopen-after")

    session, _ = await history_session(client, session_key)
    before = state["session"]
    for field in SNAPSHOT_FIELDS:
        check(
            session.get(field) == before.get(field),
            f"after restart, session.{field} is unchanged",
            f"before={before.get(field)!r} after={session.get(field)!r}",
        )
    check(
        session.get("memory_mode") is None,
        "memory_mode was never set and still reads as unset (follow global)",
        f"memory_mode={session.get('memory_mode')!r}",
    )
    usage = await session_usage(client, session_key)
    for field in USAGE_FIELDS:
        check(
            usage.get(field) == state["usage"].get(field),
            f"after restart, session.usage {field} is unchanged",
            f"before={state['usage'].get(field)!r} after={usage.get(field)!r}",
        )

    _, run_id = await send(client, "QA_REOPEN_THREE after the restart", session_key)
    outcome = await wait_run_finished(run_id)
    check(outcome is not None, "the post-restart run finished", f"outcome={outcome!r}")
    body = check_run_used_knobs(run_id, "QA_REOPEN_THREE", "after restart")
    check(
        (body or {}).get("thinking") == state.get("thinking"),
        "the post-restart request carries the same thinking setting as before",
        f"before={state.get('thinking')!r} after={(body or {}).get('thinking')!r}",
    )
    await client.ws.close()


STAGES = {
    ("reopen", "before"): reopen_before,
    ("reopen", "after"): reopen_after,
}


async def main():
    step = STAGES.get((STAGE, PHASE))
    if step is None:
        raise SystemExit(f"no such stage/phase: {STAGE} {PHASE}")
    await step()
    log("")
    if failures:
        log(f"VERDICT ({STAGE} {PHASE}): FAIL ({len(failures)} claim(s))")
        for f in failures:
            log(f"  - {f}")
    else:
        log(f"VERDICT ({STAGE} {PHASE}): PASS")
    return len(failures)


sys.exit(asyncio.run(main()))
