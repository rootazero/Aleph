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


# --- checklist (F2) ----------------------------------------------------------

# The opening of `ScratchpadGoalVerifier`'s veto (src/verification/
# scratchpad_goal_verifier.rs `veto_reason`): what the model is told when it
# tries to finish with boxes still unchecked.
VETO_PHRASE = "incomplete step(s)"


async def send_collecting(client, message, session_key=None, budget=180):
    """`chat.send`, then keep reading the socket until the run finishes.

    Returns (session_key, run_id, frames): every frame that was not the reply,
    i.e. what a client attached to this conversation was shown live.
    """
    params = {"message": message}
    if session_key:
        params["session_key"] = session_key
    else:
        params["channel"] = f"gui:qa-thread-{STAGE}"
    client.rid += 1
    rid = client.rid
    await rpc(client.ws, "chat.send", params, rid)
    frames, run_id = [], None
    slog = SessionLog(DB)
    end = time.monotonic() + budget
    while time.monotonic() < end:
        try:
            m = json.loads(await asyncio.wait_for(client.ws.recv(), 0.5))
        except asyncio.TimeoutError:
            m = None
        if m is not None:
            if m.get("id") == rid:
                if "result" not in m:
                    check(False, f"chat.send accepted {message!r}", json.dumps(m)[:300])
                    return None, None, frames
                session_key = m["result"]["session_key"]
                run_id = m["result"]["run_id"]
            else:
                frames.append(m)
        if run_id and slog.runs().get(run_id) is not None:
            break
    check(run_id is not None and slog.runs().get(run_id) is not None, f"the run for {message!r} finished")
    # The outcome row can land before the client's `stream.run_complete`
    # frame; drain briefly so callers can read the run's closing summary.
    grace = time.monotonic() + 5
    while run_id and time.monotonic() < grace and run_complete(frames, run_id) is None:
        try:
            frames.append(json.loads(await asyncio.wait_for(client.ws.recv(), 0.5)))
        except asyncio.TimeoutError:
            pass
    return session_key, run_id, frames


def run_complete(frames, run_id):
    """The `stream.run_complete` params for `run_id`, or None."""
    for f in frames:
        params = f.get("params") or {}
        if f.get("method") == "stream.run_complete" and params.get("run_id") == run_id:
            return params
    return None


def flat(obj):
    """JSON text with nested JSON strings unescaped one level, so a tool
    result embedded as a string is searchable like the rest of the frame."""
    return json.dumps(obj).replace('\\"', '"')


def frames_mentioning(frames, *needles):
    return [f for f in frames if all(n in flat(f) for n in needles)]


def plan_items(plan):
    return [(i.get("text"), i.get("status")) for i in (plan or {}).get("items") or []]


async def checklist_before():
    client = await connect("qa-thread-checklist-before")
    session_key, run_id, frames = await send_collecting(client, "QA_CHECKLIST work the list")
    if not run_id:
        return

    vetoed = [r for r in run_requests_carrying(VETO_PHRASE) if "QA_CHECKLIST" in flat(r["body"])]
    check(
        bool(vetoed),
        "finishing with QA_ITEM_B unchecked is vetoed: a later request carries the veto",
        f"{len(vetoed)} request(s) carry {VETO_PHRASE!r}",
    )
    if vetoed:
        text = flat(vetoed[0]["body"]["messages"])
        check(
            "still has 1 incomplete step(s)" in text and "QA_ITEM_B" in text,
            "the veto names the one remaining item",
        )
    check(
        len(SessionLog(DB).runs()) == 1,
        "the veto continued the SAME run (exactly one run on the server)",
        f"runs={SessionLog(DB).runs()}",
    )

    kinds = sorted({str(f.get("method") or f.get("event") or f.get("type")) for f in frames_mentioning(frames, "QA_ITEM_A")})
    log(f"{len(frames)} live frames; frames naming QA_ITEM_A: {kinds}")
    # Every list-changing scratchpad call must hand an attached client the
    # snapshot in its own completion frame, not just leave it for a later
    # `chat.history` read.
    snapshots = {}
    for frame in frames:
        if frame.get("method") != "stream.tool_end":
            continue
        output = ((frame.get("params") or {}).get("result") or {}).get("output")
        try:
            out = json.loads(output) if isinstance(output, str) else None
        except ValueError:
            out = None
        if isinstance(out, dict) and "message" in out:
            snapshots[out["message"]] = out.get("snapshot")
    for message in (
        "Plan set with 2 item(s)",
        "Item 0 marked in progress (current step)",
        "Item 0 marked as complete",
        "Item 1 marked as complete",
    ):
        snap = snapshots.get(message)
        check(
            message in snapshots and [i for i, _ in plan_items(snap)] == ["QA_ITEM_A", "QA_ITEM_B"],
            f"the live tool_end frame for {message!r} carries the checklist snapshot",
            f"snapshot={json.dumps(snap)[:200]}",
        )
    final = snapshots.get("Item 1 marked as complete")
    check(
        plan_items(final) == [("QA_ITEM_A", "completed"), ("QA_ITEM_B", "completed")],
        "the final live snapshot shows both items completed",
        f"items={plan_items(final)}",
    )

    closing_plan = ((run_complete(frames, run_id) or {}).get("summary") or {}).get("plan")
    check(
        plan_items(closing_plan) == [("QA_ITEM_A", "completed"), ("QA_ITEM_B", "completed")],
        "the run's closing summary carries the finished checklist",
        f"summary.plan={json.dumps(closing_plan)[:300]}",
    )

    _, result = await history_session(client, session_key)
    plan = result.get("plan")
    log(f"chat.history plan: {json.dumps(plan)}")
    check(
        plan_items(plan) == [("QA_ITEM_A", "completed"), ("QA_ITEM_B", "completed")],
        "chat.history reports both items completed",
        f"items={plan_items(plan)}",
    )
    save_state({"session_key": session_key, "plan": plan})
    await client.ws.close()


async def checklist_after():
    state = load_state()
    session_key = state["session_key"]
    client = await connect("qa-thread-checklist-after")

    _, result = await history_session(client, session_key)
    check(
        result.get("plan") == state["plan"] and state["plan"] is not None,
        "after restart, chat.history reports the same checklist",
        f"before={json.dumps(state['plan'])[:200]} after={json.dumps(result.get('plan'))[:200]}",
    )

    _, run_id, frames = await send_collecting(client, "QA_CHECKLIST_CLEAR start over", session_key)
    _, result = await history_session(client, session_key)
    plan = result.get("plan")
    check(
        plan is None or plan_items(plan) == [],
        "after `clear`, chat.history shows no checklist, not the old one",
        f"plan={json.dumps(plan)[:300]}",
    )
    # `clear` returns no snapshot, so a client that paints from snapshots
    # (the TUI) learns about it only from the run's closing summary: an EMPTY
    # `summary.plan` (shared/protocol/src/events.rs `RunSummary.plan`). The
    # Panel additionally hides on the `clear` action itself.
    closing = run_complete(frames, run_id) or {}
    summary_plan = (closing.get("summary") or {}).get("plan", "absent")
    check(
        isinstance(summary_plan, dict) and plan_items(summary_plan) == [],
        "the cleared run's closing summary carries an EMPTY checklist, so live clients drop the old one",
        f"run_complete seen={bool(closing)} summary.plan={json.dumps(summary_plan)[:300]}",
    )
    await client.ws.close()


# --- two_clients (F8) --------------------------------------------------------

SAME_TEXT = "same text"


async def send_and_watch(client, session_key, message, idle):
    """`chat.send` on `client`, then keep reading its socket until `idle` is
    set. Returns (reply, frames): the RPC answer and every other frame."""
    client.rid += 1
    rid = client.rid
    await rpc(client.ws, "chat.send", {"message": message, "session_key": session_key}, rid)
    answer, frames = None, []
    while not idle.is_set():
        try:
            m = json.loads(await asyncio.wait_for(client.ws.recv(), 0.5))
        except asyncio.TimeoutError:
            continue
        if m.get("id") == rid:
            answer = m
        else:
            frames.append(m)
    return answer, frames


async def wait_idle(idle, budget=180, quiet=4.0):
    """Set `idle` once every run has finished and neither the run table nor
    the provider request log has changed for `quiet` seconds: a message the
    gateway queued behind a busy run would start a new run inside that window."""
    slog = SessionLog(DB)
    end = time.monotonic() + budget
    last, stable_since = None, time.monotonic()
    await asyncio.sleep(1.0)
    while time.monotonic() < end:
        runs = slog.runs()
        now = (tuple(sorted(runs.items(), key=lambda kv: kv[0])), len(requests()))
        if now != last:
            last, stable_since = now, time.monotonic()
        elif all(v is not None for v in runs.values()) and time.monotonic() - stable_since >= quiet:
            break
        await asyncio.sleep(0.3)
    idle.set()


def error_frames(frames):
    return [
        f for f in frames
        if "error" in f or str(f.get("method", "")).endswith("error")
    ]


def event_seq_holes():
    """Per stream in `session_events`, the seq values missing between its
    first and last row. Empty dict = every stream is contiguous."""
    import sqlite3

    holes = {}
    with sqlite3.connect(DB) as db:
        streams = db.execute("SELECT session_id, MIN(seq), MAX(seq), COUNT(*) FROM session_events GROUP BY session_id")
        for session_id, low, high, count in streams.fetchall():
            if high - low + 1 != count:
                present = {s for (s,) in db.execute("SELECT seq FROM session_events WHERE session_id = ?", (session_id,))}
                holes[session_id] = sorted(set(range(low, high + 1)) - present)[:20]
    return holes


async def two_clients_only():
    a = await connect("qa-thread-client-a")
    session_key, run_id = await send(a, "QA_TWO_CLIENTS_OPEN")
    if not run_id:
        return
    check(await wait_run_finished(run_id) is not None, "the opening run finished")
    b = await connect("qa-thread-client-b")

    idle = asyncio.Event()
    (answer_a, frames_a), (answer_b, frames_b), _ = await asyncio.gather(
        send_and_watch(a, session_key, SAME_TEXT, idle),
        send_and_watch(b, session_key, SAME_TEXT, idle),
        wait_idle(idle),
    )
    log(f"client A chat.send -> {json.dumps(answer_a)[:300]}")
    log(f"client B chat.send -> {json.dumps(answer_b)[:300]}")
    log(f"runs: {SessionLog(DB).runs()}")
    for name, answer in (("A", answer_a), ("B", answer_b)):
        check(answer is not None and "result" in answer, f"client {name}'s chat.send returned success", json.dumps(answer)[:300])
    for name, frames in (("A", frames_a), ("B", frames_b)):
        bad = error_frames(frames)
        check(not bad, f"client {name} received no error frame", json.dumps(bad[:2])[:400])
    for name, answer, frames in (("A", answer_a, frames_a), ("B", answer_b, frames_b)):
        own = ((answer or {}).get("result") or {}).get("run_id")
        terminal = sorted({
            (f.get("method"), (f.get("params") or {}).get("run_id"))
            for f in frames
            if f.get("method") in ("stream.run_complete", "stream.run_error")
        })
        log(f"client {name}: own run {own}; terminal frames seen {terminal}")
        mentions = sorted({str(f.get("method")) for f in frames if own and own in json.dumps(f)})
        log(f"client {name}: frame kinds naming its own run: {mentions}")
        # A client that was handed a run id waits for that id to settle: the
        # Panel binds it from the `chat.send` reply and releases the composer
        # only on `run_complete` / `run_error` for the same id
        # (interfaces/webchat/src/state/sessions.rs `settle_run`).
        check(
            any(run == own for _, run in terminal),
            f"client {name} receives a terminal frame for the run id its chat.send returned",
            f"own={own} terminal={terminal}",
        )

    r = await a.call("chat.history", {"session_key": session_key, "limit": 200})
    rows = (r.get("result") or {}).get("messages") or []
    transcript = [(m.get("role"), message_text(m).strip()) for m in rows]
    log("transcript: " + json.dumps(transcript)[:1200])
    users = [i for i, (role, text) in enumerate(transcript) if role == "user"]
    same = [i for i in users if transcript[i][1] == SAME_TEXT]
    check(len(same) == 2, f"the transcript holds exactly two {SAME_TEXT!r} user messages", f"found {len(same)}")
    # Busy-input mode `Steer` (the default) folds a message that arrives while
    # this session's run is going into that run (src/gateway/execution_engine/
    # gate.rs `BusyInputMode::Steer`), so the two messages may share ONE reply.
    # What must hold either way: nothing is lost, and the model is shown both.
    check(
        bool(same) and any(role == "assistant" and text for role, text in transcript[same[-1] + 1 :]),
        f"an assistant reply follows the last {SAME_TEXT!r}",
    )
    shown = max(
        (
            # Raw, or wrapped by the steering reminder ("The user sent the
            # following message:\nsame text\n\n...").
            sum(
                1
                for m in r["body"].get("messages", [])
                if m.get("role") == "user"
                and (message_text(m).strip() == SAME_TEXT or f"message:\n{SAME_TEXT}\n" in message_text(m))
            )
            for r in requests()
            if r["body"].get("tools")
        ),
        default=0,
    )
    check(shown == 2, f"one provider request shows the model both {SAME_TEXT!r} messages", f"max shown in one request: {shown}")

    holes = event_seq_holes()
    check(not holes, "session event sequence numbers have no holes", json.dumps(holes)[:400])
    for c in (a, b):
        await c.ws.close()


STAGES = {
    ("reopen", "before"): reopen_before,
    ("reopen", "after"): reopen_after,
    ("checklist", "before"): checklist_before,
    ("checklist", "after"): checklist_after,
    ("two_clients", "only"): two_clients_only,
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
