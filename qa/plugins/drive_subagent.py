#!/usr/bin/env python3
"""A restricted plugin command's `subagent` child sees the restricted view.

  python3 drive_subagent.py <ws-url> <request_log> restricted
      -> the claim: `/qa-sub-plugin:delegate` (allowed-tools: Read, Task);
         the child's tools[] holds nothing outside that list.
  python3 drive_subagent.py <ws-url> <request_log> control
      -> the same delegation from a plain turn: the child's tools[] carries
         `bash`, so the negative above is one that can go red.

Sends `/qa-sub-plugin:delegate`; the mock answers every tool turn with a
`subagent` call whose task carries `QA_SUB_CHILD_TASK`. Requests are then
attributed from the request log by content, never by position (a run makes
side-channel calls that also advance the mock's counter):
  * parent turn — a request with a tool surface whose messages carry the
    command's rendered body (`QA_SUB_PARENT_MARKER`);
  * child turn  — a request with a tool surface whose FIRST user message is
    the delegated task (`QA_SUB_CHILD_TASK`).
"""
import asyncio
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, ws_connect  # noqa: E402

WS, REQ_LOG, PHASE = sys.argv[1], Path(sys.argv[2]), sys.argv[3]
L = Ledger()
PARENT_MARK, CHILD_MARK = "QA_SUB_PARENT_MARKER", "QA_SUB_CHILD_TASK"
# What the command's `allowed-tools: Read, Task` folds to, plus the tools a
# restriction never removes today (`subagent` is attached to every run,
# `tool_search` is added under deferral, `get_tool_schema` rides along — the
# P4.11 carry lists all three as outside a command's list, for the final
# review). Anything else in a tools[] is outside the list.
EXEMPT = {"subagent", "tool_search", "get_tool_schema"}
ALLOWED = {"file_read"} | EXEMPT
# A tool every unrestricted main turn carries: its presence is the tell of a
# full catalogue.
UNRESTRICTED_TELL = "bash"


def requests():
    if not REQ_LOG.exists():
        return []
    return [json.loads(line)["body"] for line in REQ_LOG.read_text().splitlines() if line.strip()]


def tool_names(body):
    return {t.get("name", "") for t in body.get("tools") or [] if isinstance(t, dict)}


def first_user_text(body):
    for m in body.get("messages", []):
        if m.get("role") == "user":
            c = m.get("content")
            if isinstance(c, str):
                return c
            if isinstance(c, list):
                return " ".join(b.get("text", "") for b in c if isinstance(b, dict))
    return ""


def split(bodies, parent_mark):
    """(parent turns, child turns) among the requests that carry a tool
    surface. `parent_mark=None` takes every non-child turn as the parent's.
    A child turn counts only if it was logged AFTER the first parent turn: a
    child from an earlier run that is still looping lands in this log too
    (first control run, 2026-09-27: two such stragglers were read as the
    control's child)."""
    with_tools = [b for b in bodies if tool_names(b)]
    is_child = [CHILD_MARK in first_user_text(b) for b in with_tools]
    parent_at = [i for i, b in enumerate(with_tools) if not is_child[i]
                 and (parent_mark is None or parent_mark in json.dumps(b.get("messages", [])))]
    if not parent_at:
        return [], []
    parent = [with_tools[i] for i in parent_at]
    child = [b for i, b in enumerate(with_tools) if is_child[i] and i > parent_at[0]]
    return parent, child


async def send(message, session_key):
    """One run, awaited to its terminal frame, so none of its children is
    still calling the mock when the next phase starts."""
    async with ws_connect(WS) as ws:
        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "connect",
                                  "params": {"client_info": {"name": "qa-subagent"}}}))
        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 2, "method": "chat.send",
                                  "params": {"message": message, "session_key": session_key}}))
        # A terminal frame may race its own `chat.send` reply: keep every one.
        run_id, ended, end = None, {}, time.monotonic() + 150
        while time.monotonic() < end:
            if run_id in ended:
                L.log(f"  (run {run_id} ended: {ended[run_id]})")
                return
            try:
                m = json.loads(await asyncio.wait_for(ws.recv(), timeout=1.0))
            except asyncio.TimeoutError:
                continue
            if m.get("id") == 2:
                L.check(f"chat.send accepted {message!r}", "result" in m, json.dumps(m.get("error", ""))[:200])
                run_id = (m.get("result") or {}).get("run_id")
                if run_id is None:
                    return
            elif m.get("method") in ("stream.run_complete", "stream.run_error"):
                ended[(m.get("params") or {}).get("run_id")] = m["method"]
        L.check("the run reached a terminal frame", False, f"run_id={run_id}")


async def observe(parent_mark):
    end, parent, child = time.monotonic() + 150, [], []
    while time.monotonic() < end and not (parent and child):
        parent, child = split(requests(), parent_mark)
        if not (parent and child):
            await asyncio.sleep(0.5)
    L.log(f"  {len(requests())} request(s): {len(parent)} parent turn(s), {len(child)} child turn(s)")
    L.check("a parent turn was recorded", bool(parent))
    L.check("a child turn (first user message = the delegated task) was recorded", bool(child))
    return (parent, child) if parent and child else (None, None)


async def restricted():
    await send("/qa-sub-plugin:delegate", "agent:main:qa-subagent")
    parent, child = await observe(PARENT_MARK)
    if parent is None:
        return
    p_tools = tool_names(parent[0])
    L.log("  parent tools[]:", sorted(p_tools))
    L.check("control: the parent turn's view is the restricted list",
            p_tools <= ALLOWED and "file_read" in p_tools, f"outside the list: {sorted(p_tools - ALLOWED)}")
    for i, c in enumerate(child):
        c_tools = tool_names(c)
        L.log(f"  child turn {i + 1} tools[] ({len(c_tools)}):", sorted(c_tools)[:30])
        L.check(f"child turn {i + 1} sees no tool outside the command's list",
                c_tools <= ALLOWED, f"outside the list ({len(c_tools - ALLOWED)}): "
                f"{sorted(c_tools - ALLOWED)[:15]}; `{UNRESTRICTED_TELL}` present={UNRESTRICTED_TELL in c_tools}")


async def control():
    """The same delegation from a plain, unrestricted turn: its child MUST see
    `bash`, or the `restricted` phase's negative could never have gone red."""
    await send("delegate a read to a sub-agent", "agent:main:qa-subagent-control")
    parent, child = await observe(None)
    if parent is None:
        return
    L.log("  parent tools[] (count):", len(tool_names(parent[0])))
    c_tools = tool_names(child[0])
    L.log(f"  child turn 1 tools[] ({len(c_tools)}):", sorted(c_tools)[:30])
    L.check(f"control: an unrestricted delegation's child carries `{UNRESTRICTED_TELL}` (the tell can fire)",
            UNRESTRICTED_TELL in c_tools, f"{len(c_tools)} tool(s)")


def main():
    if PHASE == "restricted":
        asyncio.run(restricted())
    elif PHASE == "control":
        asyncio.run(control())
    else:
        L.check(f"known phase (restricted | control), got {PHASE!r}", False)
    return L.verdict()


sys.exit(main())
