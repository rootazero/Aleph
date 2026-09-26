#!/usr/bin/env python3
"""Drive one context-slim phase through the real gateway, then judge it from
the two oracles that can settle it:

  * the mock's request log — what the server actually SENT (body + headers).
    Turn N+1's request carries turn N's tool_result, so it is also the record
    of what a tool handed the model.
  * the gateway's own replies and event frames (`chat.send`,
    `context.breakdown`, `events.subscribe` + `stream.*`).

Every negative check is anchored by a positive one first: "no reasoning was
copied" is also what a request that carried no reasoning at all would show,
so the phase first proves the reasoning is there.

Usage:  drive.py WS_URL DB PHASE REQUEST_LOG [ARM] [SERVER_LOG_DIR]
"""
import asyncio
import json
import os
import sys
import time

import websockets

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "busy_input"))
from lib import SessionLog  # noqa: E402

URL, DB, PHASE, LOG = sys.argv[1:5]
ARM = sys.argv[5] if len(sys.argv) > 5 else ""
LOG_DIR = sys.argv[6] if len(sys.argv) > 6 else ""


def server_log_line(needle):
    """The first line of the server's file log containing `needle`, or ""."""
    if not LOG_DIR or not os.path.isdir(LOG_DIR):
        return ""
    for name in sorted(os.listdir(LOG_DIR)):
        with open(os.path.join(LOG_DIR, name), errors="replace") as fh:
            for line in fh:
                if needle in line:
                    return line
    return ""
NEEDLE = "QA_NEEDLE_LINE"
WARNING = "server_context_editing is enabled but"
CHANNEL = "gui:qa-context-slim"
rc = 0


def check(ok, label, detail=""):
    global rc
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}" + (f" — {detail}" if detail else ""))
    if not ok:
        rc = 1
    return ok


def requests():
    try:
        with open(LOG) as fh:
            return [json.loads(line) for line in fh if line.strip()]
    except FileNotFoundError:
        return []


async def call(ws, rid, method, params, budget=60):
    await ws.send(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}))
    frames = []
    end = time.monotonic() + budget
    while time.monotonic() < end:
        m = json.loads(await asyncio.wait_for(ws.recv(), timeout=max(0.1, end - time.monotonic())))
        if m.get("id") == rid:
            return m, frames
        frames.append(m)
    raise TimeoutError(f"no reply to {method}")


async def connect(ws, name):
    reply, _ = await call(ws, 1, "connect", {"client": name, "version": "1"})
    return reply


async def send(ws, rid, message, session_key=None):
    params = {"message": message, "channel": CHANNEL}
    if session_key:
        params["session_key"] = session_key
    reply, frames = await call(ws, rid, "chat.send", params)
    if "error" in reply:
        print(f"  chat.send rejected: {json.dumps(reply['error'])[:300]}")
        return None, frames
    return reply.get("result", {}).get("session_key"), frames


async def wait_runs(count, budget=120):
    """`count` runs finished, and every one of them COMPLETED — a run that
    errored also writes `run_finished`, so its presence alone proves nothing."""
    slog = SessionLog(DB)
    row = await slog.wait_for("run_finished", count, budget)
    runs = slog.payloads("run_finished")
    return row is not None and all(r.get("outcome") == "completed" for r in runs), runs


async def collect_until_complete(ws, frames, budget=120):
    """Event method names on this socket until the run completes."""
    seen = [f.get("method") for f in frames if f.get("method")]
    end = time.monotonic() + budget
    while time.monotonic() < end and "stream.run_complete" not in seen:
        try:
            m = json.loads(await asyncio.wait_for(ws.recv(), timeout=max(0.1, end - time.monotonic())))
        except asyncio.TimeoutError:
            break
        if m.get("method"):
            seen.append(m["method"])
    return seen


def tool_names(body):
    return {t.get("name") for t in body.get("tools", []) if isinstance(t, dict)}


def anthropic_tool_results(body):
    """(tool name, text) of every tool_result in an Anthropic request body."""
    names = {}
    out = []
    for m in body.get("messages", []):
        c = m.get("content")
        if not isinstance(c, list):
            continue
        for b in c:
            if not isinstance(b, dict):
                continue
            if b.get("type") == "tool_use":
                names[b.get("id")] = b.get("name")
            elif b.get("type") == "tool_result":
                inner = b.get("content")
                if isinstance(inner, list):
                    text = "\n".join(x.get("text", "") for x in inner if isinstance(x, dict))
                else:
                    text = inner if isinstance(inner, str) else json.dumps(inner)
                out.append((names.get(b.get("tool_use_id")), text))
    return out


# ── phases ────────────────────────────────────────────────────────────────────


async def replay():
    async with websockets.connect(URL, max_size=None) as ws:
        await connect(ws, "qa-context-slim")
        key, _ = await send(ws, 2, "QA replay turn one")
        if not check(key is not None, "turn 1 accepted", f"session {key}"):
            return
        ok, runs = await wait_runs(1)
        check(ok, "turn 1 completed", json.dumps([r.get("outcome") for r in runs]))
        key2, _ = await send(ws, 3, "QA replay turn two", key)
        check(key2 is not None, "turn 2 accepted")
        ok, runs = await wait_runs(2)
        check(ok, "turn 2 completed", json.dumps([r.get("outcome") for r in runs])[:200])

    reqs = [r for r in requests() if r["path"].endswith("/chat/completions")]
    rejected = [r["n"] for r in reqs if r["rejected"]]
    check(bool(reqs), "the DeepSeek-classified host was reached", f"{len(reqs)} request(s)")
    check(not rejected, "no request was rejected for a missing reasoning_content",
          f"rejected #{rejected}")
    tooled = [r for r in reqs if r["body"].get("tools")]
    last = tooled[-1]["body"] if tooled else {}
    assistants = [m for m in last.get("messages", []) if m.get("role") == "assistant"]
    replayed = [m.get("reasoning_content", "") for m in assistants]
    check(len(assistants) >= 3, "turn 2's last request carries the earlier turns",
          f"{len(assistants)} assistant message(s)")
    check(all("reasoning_content" in m for m in assistants),
          "every prior assistant message carries reasoning_content",
          json.dumps([bool(x) for x in replayed]))
    check(sum(1 for x in replayed if "QA_REASONING_" in x) >= 2,
          "turn 1's reasoning is replayed verbatim (anchor for the next check)",
          json.dumps(replayed)[:240])
    stripped = json.dumps(
        [{k: v for k, v in m.items() if k != "reasoning_content"} for m in last.get("messages", [])]
    )
    check("QA_REASONING_" not in stripped,
          "the reasoning appears nowhere but reasoning_content (not in content, tool calls, "
          "tool results)")


async def carve():
    async def run(name, subscribe, message):
        async with websockets.connect(URL, max_size=None) as ws:
            await connect(ws, name)
            reply, _ = await call(ws, 2, "events.subscribe", subscribe)
            if "error" in reply:
                return None
            key, frames = await send(ws, 3, message)
            if key is None:
                return None
            return await collect_until_complete(ws, frames)

    control = await run("qa-carve-control", {"topics": ["stream.*"]}, "QA carve control")
    if not check(control is not None, "control run accepted"):
        return
    check("stream.reasoning" in control,
          "control: a plain stream.* subscriber receives stream.reasoning (anchor)",
          f"{len(control)} frame(s): {sorted(set(control))}")
    await wait_runs(1)
    carved = await run("qa-carve-except",
                       {"topics": ["stream.*"], "except": ["stream.reasoning"]},
                       "QA carve except")
    if not check(carved is not None, "carved run accepted"):
        return
    check(any(m.startswith("stream.") for m in carved),
          "carved: other stream.* frames still arrive", f"{sorted(set(carved))}")
    check("stream.reasoning" not in carved, "carved: no stream.reasoning frame arrives",
          f"{carved.count('stream.reasoning')} reasoning frame(s)")


async def ingress(also_breakdown):
    async with websockets.connect(URL, max_size=None) as ws:
        await connect(ws, "qa-context-slim")
        key, _ = await send(ws, 2, "QA ingress: run the big command")
        if not check(key is not None, "turn accepted", f"session {key}"):
            return
        ok, runs = await wait_runs(1, 180)
        check(ok, "turn completed", json.dumps([r.get("outcome") for r in runs]))

        reqs = [r for r in requests() if r["path"].endswith("/v1/messages")]
        bash = next(
            ((r, t) for r in reqs for (name, t) in anthropic_tool_results(r["body"])
             if name == "bash"),
            None,
        )
        if not check(bash is not None, "a bash tool_result reached the model"):
            return
        first, text = bash
        check("[Full output persisted: " in text, "the model-facing result is the persist marker",
              text[:160].replace("\n", " | "))
        check(NEEDLE not in text and "59999" not in text,
              "the body stayed on disk (no needle, no tail of the seq output)")
        named = [t for t in ("ctx_search", "file_read") if t in text]
        callable_ = tool_names(first["body"])
        check(bool(named), "the footer names a retrieval tool", f"{named}")
        check(all(t in callable_ for t in named),
              "every tool the footer names is in this request's tool list",
              f"named {named}; tools include ctx_search={('ctx_search' in callable_)}, "
              f"file_read={('file_read' in callable_)}")
        later = [r for r in reqs if r["n"] > first["n"]]
        # Far from the needle, so a ctx_search section around it cannot carry it.
        body_probe = "12345\\n12346"
        check(bool(later) and all(body_probe not in json.dumps(r["body"]) for r in later),
              "no later request carries the offloaded body", f"{len(later)} later request(s)")
        hit = next(
            (t for r in reqs for (name, t) in anthropic_tool_results(r["body"])
             if name == "ctx_search"),
            None,
        )
        check(hit is not None, "ctx_search ran")
        check(hit is not None and NEEDLE in hit,
              "ctx_search returned a section BODY from the persisted original",
              (hit or "")[:200].replace("\n", " | "))

        if also_breakdown:
            reply, _ = await call(ws, 9, "context.breakdown", {"session_key": key})
            res = reply.get("result") or {}
            if not check("error" not in reply, "context.breakdown answered",
                         json.dumps(reply.get("error"))[:200]):
                return
            print(f"  context.breakdown: {json.dumps(res)[:400]}")
            messages = res.get("messages") or {}
            tool_output = res.get("tool_output") or {}
            check((messages.get("tool_results") or 0) > 0,
                  "messages.tool_results > 0", json.dumps(messages))
            check(isinstance(tool_output.get("since_unix_ms"), int),
                  "tool_output carries since_unix_ms", json.dumps(tool_output))
            check((tool_output.get("offloaded") or 0) >= 1,
                  "tool_output counts the Layer-2 offload", json.dumps(tool_output))


async def gate():
    """A result between the 4k per-result default and the old 8k one is
    offloaded now — and would have passed verbatim before."""
    async with websockets.connect(URL, max_size=None) as ws:
        await connect(ws, "qa-context-slim")
        key, _ = await send(ws, 2, "QA gate: a mid-sized output")
        if not check(key is not None, "turn accepted", f"session {key}"):
            return
        ok, runs = await wait_runs(1, 180)
        check(ok, "turn completed", json.dumps([r.get("outcome") for r in runs]))
        reply, _ = await call(ws, 9, "context.breakdown", {"session_key": key})
    tool_output = (reply.get("result") or {}).get("tool_output") or {}
    produced = tool_output.get("produced_tokens") or 0
    # `produced_tokens` is `estimate_tokens_smart` of the tool's untouched
    # output — the same estimate, of the same string, that ingress compares
    # to the per-result budget (`ingress.rs`: `before > limit`). One call, so
    # it is this call's measure alone.
    in_range = tool_output.get("calls") == 1 and 4000 < produced <= 8000
    check(in_range, "precondition: the output measures 4k < tokens <= 8k, as Layer 2 measures it",
          json.dumps(tool_output) + ("" if in_range else " — retune QA_GATE_LINES"))
    reqs = [r for r in requests() if r["path"].endswith("/v1/messages")]
    text = next((t for r in reqs for (name, t) in anthropic_tool_results(r["body"])
                 if name == "bash"), None)
    if not check(text is not None, "a bash tool_result reached the model"):
        return
    check("[Full output persisted: " in text,
          "under the 4k default it is offloaded (marker + recovery footer), not sent verbatim",
          text[:160].replace("\n", " | "))



async def firstparty():
    async with websockets.connect(URL, max_size=None) as ws:
        await connect(ws, "qa-context-slim")
        key, _ = await send(ws, 2, f"QA first-party ({ARM})")
        if not check(key is not None, "turn accepted"):
            return
        ok, runs = await wait_runs(1)
        check(ok, "turn completed", json.dumps([r.get("outcome") for r in runs]))
    reqs = [r for r in requests() if r["path"].endswith("/v1/messages")]
    after = next((r for r in reqs if anthropic_tool_results(r["body"])), None)
    if not check(after is not None, "a request after the tool call was sent"):
        return
    body, headers = after["body"], after["headers"]
    blocks = [b for m in body.get("messages", []) if isinstance(m.get("content"), list)
              for b in m["content"] if isinstance(b, dict)]
    thinking = [b for b in blocks if b.get("type") == "thinking"]
    uses = [b for b in blocks if b.get("type") == "tool_use"]
    check(bool(thinking) and all(b.get("signature") for b in thinking),
          "the signed thinking block is replayed (anchor)", f"{len(thinking)} block(s)")
    copied = [b for b in uses if "reasoning_content" in (b.get("input") or {})]
    beta = headers.get("anthropic-beta", "")
    cm = body.get("context_management")
    if ARM == "1p":
        check(not copied, "first-party: no reasoning_content copied into tool_use.input",
              f"{len(copied)}/{len(uses)} copied")
        check(cm == {"edits": [{"type": "clear_tool_uses_20250919"}]},
              "first-party + enabled: context_management.edits is on the wire", json.dumps(cm))
        check("context-management-2025-06-27" in beta, "…with its beta header", beta)
        check(os.path.isdir(LOG_DIR) and not server_log_line(WARNING),
              "first-party: no 'has no effect' warning in the server log (the other half)")
    else:
        check(bool(copied) and len(copied) == len(uses),
              "custom host: the reasoning_content copy is kept (status quo, U1)",
              f"{len(copied)}/{len(uses)} copied")
        check(cm is None, "custom host: no context_management even though enabled",
              json.dumps(cm))
        check("context-management-2025-06-27" not in beta, "…and no beta", beta)
        said = server_log_line(WARNING)
        check(bool(said), "the server log says the setting has no effect here",
              said.strip()[:200])


def main():
    if PHASE == "replay":
        asyncio.run(replay())
    elif PHASE == "carve":
        asyncio.run(carve())
    elif PHASE == "ingress":
        asyncio.run(ingress(False))
    elif PHASE == "breakdown":
        asyncio.run(ingress(True))
    elif PHASE == "gate":
        asyncio.run(gate())
    elif PHASE == "firstparty":
        asyncio.run(firstparty())
    print(f"\n{PHASE}{'/' + ARM if ARM else ''}: {'PASS' if rc == 0 else 'FAIL'}")
    sys.exit(rc)


main()
