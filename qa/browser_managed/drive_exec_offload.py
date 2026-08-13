#!/usr/bin/env python3
"""Real-machine QA for `browser_exec`'s offload branch — the half `tools.invoke`
structurally cannot reach.

When a `snapshot` step overflows its own `max_chars`, `snapshot_output` tries to
spill the full tree through `offload_full_content` and hand the model a
`[Full output persisted: …]` pointer. That spill is keyed by
`(tool_call_id, tool_name)`, and the tool call id is a **task-local minted by
the harness Act phase** — `tools.invoke` runs a tool without a turn, so there is
no id, and the tool takes its other branch: an honest "the dropped tail is not
recoverable" note. `qa/browser_managed/run.sh tools` asserts that branch. Only an
agent turn can reach this one, which is why this scenario has a mock provider
while every other browser scenario deliberately has none.

The oracle is the mock's request log, not the tool's RPC reply: turn 2 carries
turn 1's `tool_result` verbatim, so the file is literally what the model was
handed. Asserting against the `tools.invoke` reply instead would be asserting
about a different path than the one under test.

Usage: drive_exec_offload.py WS_URL --page-url … --request-log … --marker …
"""
import argparse
import asyncio
import json
import os
import re
import sys
import time

import websockets

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qa_rpc import Ledger, Rpc  # noqa: E402

PERSIST_RE = re.compile(r"\[Full output persisted: (?P<path>.+?) \((?P<meta>[^)]*)\)\]")


def tool_results(request_log):
    """Every `tool_result` block the mock ever received, oldest first."""
    out = []
    if not os.path.exists(request_log):
        return out
    with open(request_log) as fh:
        for line in fh:
            try:
                entry = json.loads(line)
            except ValueError:
                continue
            for msg in entry.get("body", {}).get("messages", []):
                content = msg.get("content")
                if not isinstance(content, list):
                    continue
                for block in content:
                    if not isinstance(block, dict) or block.get("type") != "tool_result":
                        continue
                    inner = block.get("content")
                    if isinstance(inner, str):
                        out.append(inner)
                    elif isinstance(inner, list):
                        out.append(
                            "\n".join(
                                b.get("text", "")
                                for b in inner
                                if isinstance(b, dict)
                            )
                        )
    return out


async def main(args):
    led = Ledger()
    async with websockets.connect(args.url, max_size=None) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-exec-offload")

        led.log("\n--- open a page (no agent yet: this is just staging) ---")
        ok, res = await rpc.invoke("browser_open", {"url": args.page_url, "profile": "default"})
        if not led.check("browser_open succeeds", ok and res.get("success"), json.dumps(res)[:220]):
            return led.verdict()

        led.log("\n--- one agent turn, whose only tool call is browser_exec ---")
        msg = await rpc.call("chat.send", {"message": "snapshot the page", "channel": "gui:qa-exec"})
        result = msg.get("result") or {}
        led.check("chat.send accepted the run", bool(result.get("run_id")), json.dumps(msg)[:220])

        # Two provider turns must land: the one that calls the tool, and the one
        # that carries its result back. Poll the mock's log for the second.
        deadline = time.monotonic() + args.wait_secs
        results = []
        while time.monotonic() < deadline:
            results = tool_results(args.request_log)
            if results:
                break
            await asyncio.sleep(1)
        if not led.check("the model was handed a tool_result at all",
                         bool(results),
                         f"request log: {args.request_log}"):
            return led.verdict()

        body = "\n".join(results)
        led.log(f"  (tool_result tail: {body[-400:]})")

        # The claim. Over `tools.invoke` this same call says "not recoverable";
        # inside a turn it must name a file instead.
        m = PERSIST_RE.search(body)
        led.check(
            "a budget-cut exec snapshot offloads inside an agent turn",
            bool(m),
            body[-500:] if not m else m.group(0),
        )
        led.check(
            "…and does NOT fall back to the no-call-id wording",
            "not recoverable" not in body,
            body[-400:],
        )
        if not m:
            return led.verdict()

        path = m.group("path")
        led.check("…and the file it names exists", os.path.exists(path), path)
        if not os.path.exists(path):
            return led.verdict()
        with open(path, encoding="utf-8", errors="replace") as fh:
            spilled = fh.read()

        # "A file exists" and "the dropped tail is in it" are different claims,
        # and only the second one is what the marker promises. `filler row 59` is
        # the last node of the fixture's accessibility tree and sits far beyond
        # the 1000-char cut, so it is present in the spill and absent from what
        # the model got inline.
        led.check(
            "…the spill carries the tail that was cut, not just the head again",
            "filler row 59" in spilled,
            f"{len(spilled)} bytes, tail={spilled[-160:]!r}",
        )
        led.check(
            "…and that tail really was absent from what the model saw inline",
            "filler row 59" not in body,
            body[-300:],
        )
        led.check(
            "…and the page marker is in the spill (it is this page's tree)",
            args.marker in spilled,
            spilled[:200],
        )
    return led.verdict()


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("url")
    ap.add_argument("--page-url", required=True)
    ap.add_argument("--request-log", required=True)
    ap.add_argument("--marker", required=True)
    ap.add_argument("--wait-secs", type=float, default=180.0)
    sys.exit(asyncio.run(main(ap.parse_args())))
