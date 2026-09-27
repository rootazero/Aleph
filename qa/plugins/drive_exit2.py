#!/usr/bin/env python3
"""A `>&2; exit 2` PreToolUse hook blocks the tool and its reason reaches the model.

  python3 drive_exit2.py <ws-url> <request_log> prime
      -> one run while the hook is NOT approved yet. Consent skips an
         un-approved shell hook (fail-safe) and records it pending, so the
         tool runs: the probe content reaching the model is EXPECTED here.
         This is the control that makes run 2's negative mean something.
  python3 drive_exit2.py <ws-url> <request_log> block
      -> one run after the operator approved it: the hook's stderr is the
         tool result, and the probe content never reaches the model.

The oracle is the mock's request log (a fresh file per phase): turn N+1's
`messages` carry turn N's `tool_result` verbatim. If the hook never fires in
`block`, the probe's CONTENT reaching the model is the tell, and the failure
line says so.
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
REASON = "QA_BLOCK_REASON_policy"
PROBE_CONTENT = "QA_PROBE_CONTENT_MUST_NOT_REACH_THE_MODEL"
BUDGET = 150.0


def tool_results():
    """Every distinct tool_result text in the request log."""
    if not REQ_LOG.exists():
        return []
    out, seen = [], set()
    for line in REQ_LOG.read_text().splitlines():
        if not line.strip():
            continue
        for m in json.loads(line)["body"].get("messages", []):
            c = m.get("content")
            if not isinstance(c, list):
                continue
            for b in c:
                if isinstance(b, dict) and b.get("type") == "tool_result":
                    inner = b.get("content")
                    text = inner if isinstance(inner, str) else json.dumps(inner)
                    if text not in seen:
                        seen.add(text)
                        out.append(text)
    return out


def wait_for(pred):
    end = time.monotonic() + BUDGET
    while time.monotonic() < end:
        for t in tool_results():
            if pred(t):
                return t
        time.sleep(0.5)
    return None


async def send(session_key):
    """One run, awaited to its terminal frame: the `prime` run must be over
    before the operator approves, or one of its late calls would be judged
    under the approval meant for `block`."""
    async with ws_connect(WS) as ws:
        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "connect",
                                  "params": {"client_info": {"name": "qa-exit2"}}}))
        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 2, "method": "chat.send",
                                  "params": {"message": "read the probe file", "session_key": session_key}}))
        # A terminal frame may race its own `chat.send` reply: keep every one.
        run_id, ended, end = None, {}, time.monotonic() + BUDGET
        while time.monotonic() < end:
            if run_id in ended:
                L.log(f"  (run {run_id} ended: {ended[run_id]})")
                return
            try:
                m = json.loads(await asyncio.wait_for(ws.recv(), timeout=1.0))
            except asyncio.TimeoutError:
                continue
            if m.get("id") == 2:
                L.check("chat.send accepted", "result" in m, json.dumps(m.get("error", ""))[:200])
                run_id = (m.get("result") or {}).get("run_id")
                if run_id is None:
                    return
            elif m.get("method") in ("stream.run_complete", "stream.run_error"):
                ended[(m.get("params") or {}).get("run_id")] = m["method"]
        L.check("the run reached a terminal frame", False, f"run_id={run_id}")


async def prime():
    await send("agent:main:qa-exit2-prime")
    leaked = wait_for(lambda t: PROBE_CONTENT in t)
    L.check("control: while un-approved the hook is skipped and file_read runs (probe content reached the model)",
            leaked is not None, f"{len(tool_results())} tool_result(s) seen")
    L.check("control: no block reason while un-approved",
            all(REASON not in t for t in tool_results()))


async def block():
    await send("agent:main:qa-exit2-block")
    blocked = wait_for(lambda t: REASON in t)
    results = tool_results()
    if blocked is None:
        leaked = any(PROBE_CONTENT in t for t in results)
        L.check("the exit-2 hook's stderr reached the model as the tool result", False,
                "the hook did not block — the probe content reached the model (approval not honoured, "
                "or `Read` no longer matches file_read)" if leaked
                else f"no result carried the reason; {len(results)} tool_result(s) seen: {[r[:80] for r in results]}")
        return
    L.check("the exit-2 hook's stderr reached the model as the tool result", True, blocked[:200])
    L.check("the tool did NOT run (probe content never reached the model)",
            all(PROBE_CONTENT not in t for t in results), f"{len(results)} tool_result(s)")


def main():
    if PHASE == "prime":
        asyncio.run(prime())
    elif PHASE == "block":
        asyncio.run(block())
    else:
        L.check(f"known phase (prime | block), got {PHASE!r}", False)
    return L.verdict()


sys.exit(main())
