#!/usr/bin/env python3
"""Driver for qa/human_gate (F4: tools that pause for a human).

Usage:  drive.py STAGE WS_URL DB REQUEST_LOG BASE_URL WEBHOOK_PATH SECRET

  STAGE  ask | text | unattended | stale

The mock's first run turn calls `ask_user` with two questions (`QA_Q1`,
`QA_Q2`); its next turn ends the run. What the model got back is read from
the mock's request log: the `tool_result` the following request carries.

Exit code = number of failed claims.
"""
import asyncio
import json
import sys
import time

import websockets

sys.path.insert(0, __file__.rsplit("/", 1)[0] + "/../busy_input")
from lib import SessionLog, channel_message, log, reply, rpc, webhook_post  # noqa: E402

STAGE, WS_URL, DB, REQUEST_LOG, BASE_URL, WEBHOOK_PATH, SECRET = sys.argv[1:8]

# The model-facing refusal for a turn nobody can answer
# (src/clarification/ask.rs `HEADLESS_DENIAL`).
HEADLESS_PHRASE = "no human is reachable on this run"

failures = []


def check(ok, claim, evidence=""):
    """Record one claim. Never raises: report every failure, not the first."""
    log(("  PASS  " if ok else "  FAIL  ") + claim)
    if evidence:
        log(f"          {evidence}")
    if not ok:
        failures.append(claim)
    return ok


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


def tool_results():
    """The text of every `tool_result` block any run request carried, in order
    of first appearance (later requests repeat earlier history)."""
    seen, out = set(), []
    for r in requests():
        if not r["body"].get("tools"):
            continue
        for m in r["body"].get("messages", []):
            if not isinstance(m.get("content"), list):
                continue
            for block in m["content"]:
                if not isinstance(block, dict) or block.get("type") != "tool_result":
                    continue
                inner = block.get("content")
                if isinstance(inner, list):
                    text = " ".join(x.get("text", "") for x in inner if isinstance(x, dict))
                else:
                    text = str(inner)
                key = (block.get("tool_use_id"), text)
                if key not in seen:
                    seen.add(key)
                    out.append(text)
    return out


def answers_in_order(text, first, second):
    a, b = text.find(first), text.find(second)
    return a != -1 and b != -1 and a < b


class Client:
    def __init__(self, ws):
        self.ws = ws
        self.rid = 1000

    async def call(self, method, params, budget=60):
        self.rid += 1
        await rpc(self.ws, method, params, self.rid)
        return await reply(self.ws, self.rid, budget)


async def connect(name):
    ws = await websockets.connect(WS_URL, max_size=None)
    client = Client(ws)
    r = await client.call("connect", {"client_info": {"name": name}})
    if "result" not in r:
        raise SystemExit(f"connect refused: {r}")
    return client


async def pending(client):
    r = await client.call("clarification.pending", {})
    return (r.get("result") or {}).get("pending") or []


async def await_question(client, marker, budget=60):
    """The pending, not yet answered clarification whose question text
    carries `marker`."""
    end = time.monotonic() + budget
    while time.monotonic() < end:
        for p in await pending(client):
            if marker in json.dumps(p) and p.get("answered", 0) == 0:
                return p
        await asyncio.sleep(0.3)
    return None


async def wait_all_runs_finished(budget=60):
    """True once at least one run exists and every run has an outcome."""
    slog = SessionLog(DB)
    end = time.monotonic() + budget
    while time.monotonic() < end:
        runs = slog.runs()
        if runs and all(v is not None for v in runs.values()):
            return runs
        await asyncio.sleep(0.3)
    return None


def check_model_got_both_answers(label):
    results = [t for t in tool_results() if "A1-" in t or "A2-" in t]
    log(f"tool results carrying an answer: {[t[:300] for t in results]}")
    check(
        any(answers_in_order(t, "A1-RED", "A2-LARGE") for t in results),
        f"{label}: the ask_user tool result carries both answers, A1 before A2",
    )


async def ask_and_resolve(client):
    """Shared by `ask` and `stale`. Returns (session_key, request_id)."""
    r = await client.call("chat.send", {"message": "QA_ASK go", "channel": f"gui:qa-human-{STAGE}"})
    if not check("result" in r, "chat.send accepted", json.dumps(r)[:300]):
        return None, None
    session_key = r["result"]["session_key"]

    question = await await_question(client, "QA_Q1")
    if not check(question is not None, "the ask_user request shows up in clarification.pending"):
        return session_key, None
    log(f"pending: {json.dumps(question)[:500]}")
    check(
        [q.get("id") for q in question.get("questions") or []] == ["q1", "q2"],
        "the pending request carries both questions in order",
    )
    request_id = question.get("request_id")
    r = await client.call(
        "clarification.resolve",
        {"session_key": session_key, "answers": ["A1-RED", "A2-LARGE"], "expected_request_id": request_id},
    )
    result = r.get("result") or {}
    check(
        result.get("resolved") is True and result.get("stale") is False,
        "clarification.resolve with both answers is accepted",
        json.dumps(r)[:300],
    )
    runs = await wait_all_runs_finished()
    check(runs is not None and set(runs.values()) == {"completed"}, "the run finished normally", f"runs={runs}")
    check_model_got_both_answers("ask")
    check(not await pending(client), "nothing is left pending")
    return session_key, request_id


async def stage_ask():
    client = await connect("qa-human-ask")
    await ask_and_resolve(client)
    await client.ws.close()


async def stage_stale():
    client = await connect("qa-human-stale")
    session_key, request_id = await ask_and_resolve(client)
    if not request_id:
        return
    r = await client.call(
        "clarification.resolve",
        {"session_key": session_key, "reply": "A-LATE", "expected_request_id": request_id},
    )
    result = r.get("result") or {}
    log(f"late resolve -> {json.dumps(r)[:300]}")
    check(
        result.get("stale") is True and result.get("resolved") is False,
        "a second reply to the already-answered request is reported stale, not resolved",
    )
    check(
        not any("A-LATE" in t for t in tool_results()),
        "the late reply never reaches the model",
    )
    await client.ws.close()


def post(text):
    status, body = webhook_post(
        BASE_URL, WEBHOOK_PATH, SECRET, channel_message(text, "qa-human-text")
    )
    check(200 <= status < 300, f"webhook accepted {text!r}", f"status={status} body={body[:200]}")


async def pair(client):
    """The operator half of the pairing handshake: a new channel sender's
    first message only opens a pairing request (same as qa/busy_input)."""
    for _ in range(30):
        r = await client.call("channel.pairing.list", {})
        mine = [p for p in (r.get("result") or {}).get("requests") or [] if p.get("channel") == "webhook"]
        if mine:
            r = await client.call("channel.pairing.approve", {"channel": "webhook", "code": mine[0]["code"]})
            return check("result" in r, "the operator approves the channel sender", json.dumps(r)[:300])
        await asyncio.sleep(1)
    return check(False, "a pairing request appeared for the channel sender")


def provider_request_count():
    try:
        return sum(1 for _ in open(REQUEST_LOG))
    except FileNotFoundError:
        return 0


async def wait_for_tool_request(budget=60):
    end = time.monotonic() + budget
    while time.monotonic() < end:
        try:
            for line in open(REQUEST_LOG):
                if json.loads(line)["body"].get("tools"):
                    return True
        except FileNotFoundError:
            pass
        await asyncio.sleep(0.3)
    return False


async def stage_text():
    client = await connect("qa-human-text")
    post("hello, pairing please")
    if not await pair(client):
        return
    post("QA_ASK go")
    # The ask is parked once the provider has seen the tool-bearing request
    # that the mock answers with `ask_user`.
    if not check(await wait_for_tool_request(), "the channel run reaches the model with tools"):
        return
    await asyncio.sleep(2)
    # Defect D1: the channel run parks its question under the structured
    # `agent:main:dm:qa-sender` key, the handler re-parses the string into the
    # channel-less key and misses the session row, so a scoped operator never
    # sees it. Kept as a claim; the text replies below do not depend on it.
    first = await await_question(client, "QA_Q1", budget=5)
    check(first is not None, "the channel run's ask_user request shows up in clarification.pending",
          f"pending={json.dumps(await pending(client))[:300]}")
    asked = provider_request_count()
    post("A1-RED")
    await asyncio.sleep(4)
    check(provider_request_count() == asked,
          "after the first text reply the run still waits for the second answer",
          f"requests before={asked} after={provider_request_count()}")
    post("A2-LARGE")
    runs = await wait_all_runs_finished()
    check(runs is not None and set(runs.values()) == {"completed"}, "the run finished normally", f"runs={runs}")
    check_model_got_both_answers("text")
    check(not await pending(client), "nothing is left pending")
    await client.ws.close()


async def stage_unattended():
    client = await connect("qa-human-unattended")
    r = await client.call(
        "cron.create",
        {
            "name": "qa-human-unattended",
            "prompt": "QA_CRON go",
            # Far in the future: the job only runs when `cron.run` fires it.
            "schedule_kind": {"kind": "at", "at": int(time.time() * 1000) + 86_400_000},
        },
    )
    if not check("result" in r, "cron.create accepted", json.dumps(r)[:300]):
        return
    job_id = r["result"]["job"]["id"]
    started = time.monotonic()
    r = await client.call("cron.run", {"job_id": job_id})
    check("result" in r, "cron.run accepted", json.dumps(r)[:300])

    ever_pending, refused = [], None
    while time.monotonic() - started < 60:
        ever_pending += await pending(client)
        refused = next((t for t in tool_results() if HEADLESS_PHRASE in t), None)
        runs = SessionLog(DB).runs()
        if refused and runs and all(v is not None for v in runs.values()):
            break
        await asyncio.sleep(0.3)
    elapsed = time.monotonic() - started
    check(refused is not None, "the unattended ask_user is refused with the headless denial", (refused or "")[:300])
    check(not ever_pending, "clarification.pending stayed empty the whole time", json.dumps(ever_pending)[:300])
    runs = SessionLog(DB).runs()
    check(
        bool(runs) and all(v is not None for v in runs.values()) and elapsed < 60,
        "the cron run finished within 60 seconds",
        f"elapsed={elapsed:.1f}s runs={runs}",
    )
    await client.ws.close()


STAGES = {"ask": stage_ask, "text": stage_text, "unattended": stage_unattended, "stale": stage_stale}


async def main():
    await STAGES[STAGE]()
    log(f"VERDICT ({STAGE}): " + ("PASS" if not failures else f"FAIL ({len(failures)} claim(s))"))
    for f in failures:
        log(f"  - {f}")
    return len(failures)


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
