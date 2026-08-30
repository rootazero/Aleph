#!/usr/bin/env python3
"""What the model received, and what the backend was asked.

Two oracles, and the phases need both:

  * the mock provider's request log — turn N+1 carries turn N's `tool_result`
    verbatim, so it is the only record of what the search tool actually handed
    back. The tool's RPC reply is a different object on a different path.
  * each mock SearXNG's request log — the query string that went over the
    wire, which is the only place a claim like "`recency` reaches the backend"
    can be settled.

Every phase anchors before it negates. "the notes do not say X" and "no
request arrived" are both satisfied by a turn that never ran, so each phase
first proves the search returned something before asserting anything about
what it says.

Usage:  drive_search.py WS_URL PHASE LLM_LOG SEARX_LOG [SEARX_LOG_2]
"""
import asyncio
import json
import os
import sys
import time

import websockets

URL, PHASE, LLM_LOG = sys.argv[1], sys.argv[2], sys.argv[3]
SEARX_LOGS = sys.argv[4:]
BUDGET = float(60 if PHASE in ("reach", "degrade", "empty") else 120)

# Markers carried in the query text, because `SearchOutput` echoes `query`.
# Attribution by content, never by turn number: a run opens with a
# strategy-planner call that advances the mock's counter without emitting a
# tool call, so "turn 2 holds turn 1's result" is false.
ARM_DOMAINS = "QA_ARM_DOMAINS"
ARM_PLAIN = "QA_ARM_PLAIN"

rc = 0


def check(ok, label, detail=""):
    global rc
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}" + (f" — {detail}" if detail else ""))
    if not ok:
        rc = 1


def tool_results():
    """Every `tool_result` payload the LLM log has seen, oldest first, deduped."""
    out, seen = [], set()
    try:
        fh = open(LLM_LOG)
    except FileNotFoundError:
        return out
    with fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            for m in json.loads(line)["body"].get("messages", []):
                content = m.get("content")
                if not isinstance(content, list):
                    continue
                for block in content:
                    if not isinstance(block, dict) or block.get("type") != "tool_result":
                        continue
                    c = block.get("content")
                    if isinstance(c, str):
                        text = c
                    elif isinstance(c, list):
                        text = " ".join(b.get("text", "") for b in c if isinstance(b, dict))
                    else:
                        text = json.dumps(c)
                    if text not in seen:
                        seen.add(text)
                        out.append(text)
    return out


def wait_for(predicate):
    end = time.monotonic() + BUDGET
    while time.monotonic() < end:
        for text in tool_results():
            if predicate(text):
                return text
        time.sleep(0.5)
    return None


def requests_to(path):
    try:
        with open(path) as fh:
            return [line.strip() for line in fh if line.strip()]
    except FileNotFoundError:
        return []


async def ask(message):
    """One real message, through the surface a Panel uses."""
    async with websockets.connect(URL, max_size=None) as ws:
        await ws.send(
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "connect",
                    "params": {"client": "qa-web-search", "version": "1"},
                }
            )
        )
        await ws.send(
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "chat.send",
                    "params": {"message": message, "channel": "gui:qa-web-search"},
                }
            )
        )
        end = time.monotonic() + 30
        while time.monotonic() < end:
            m = json.loads(await asyncio.wait_for(ws.recv(), timeout=30))
            if m.get("id") == 2:
                if "error" in m:
                    print(f"  chat.send rejected: {json.dumps(m['error'])[:300]}")
                    return False
                return True
    return False


def main():
    if not asyncio.run(ask(f"search please ({PHASE})")):
        check(False, "chat.send was accepted")
        return

    if PHASE == "reach":
        hit = wait_for(lambda t: "QA result 1" in t)
        if hit is None:
            check(False, "a search result reached the model", f"{len(tool_results())} tool_result(s)")
            return
        check(True, "a search result reached the model")
        asked = requests_to(SEARX_LOGS[0])
        check(bool(asked), "the backend was asked at all", f"{len(asked)} request(s)")
        check(
            any("time_range=week" in q for q in asked),
            "the freshness the model named reached the backend's query string",
            "; ".join(asked)[:300],
        )
        check(
            "provider_used" in hit and "searxng" in hit,
            "the answer names which backend answered",
            hit[:300],
        )
        return

    if PHASE == "degrade":
        hit = wait_for(lambda t: "QA result 1" in t)
        if hit is None:
            check(False, "the search still ran", f"{len(tool_results())} tool_result(s)")
            return
        check(True, "the search still ran despite an unsupported dimension")
        check(
            "domains" in hit and "searxng" in hit,
            "the answer names the dropped dimension and the backend that dropped it",
            hit[:400],
        )
        asked = requests_to(SEARX_LOGS[0])
        check(
            not any("domain" in q for q in asked),
            "and the backend was not sent a parameter it has no name for",
            "; ".join(asked)[:300],
        )
        return

    if PHASE == "order":
        # The arm that asks for a dimension only Exa can carry.
        domains = wait_for(lambda t: ARM_DOMAINS in t)
        if domains is None:
            check(False, "the domain-filtered arm ran", f"{len(tool_results())} tool_result(s)")
            return
        check("QA result 1" in domains, "the domain-filtered arm still got results", domains[:200])
        check(
            "answered after" in domains,
            "a backend that can carry `domains` was asked first, and its failure is reported",
            domains[:400],
        )
        # The control: identical config, same backends, no dimension asked for.
        # Without it a green above only says "exa always fails".
        plain = wait_for(lambda t: ARM_PLAIN in t)
        if plain is None:
            check(False, "the control arm ran", f"{len(tool_results())} tool_result(s)")
            return
        check("QA result 1" in plain, "the control arm got results", plain[:200])
        check(
            "answered after" not in plain,
            "with no dimension asked for, the default backend was asked first",
            plain[:400],
        )
        return

    if PHASE == "empty":
        hit = wait_for(lambda t: "QA result 1" in t)
        if hit is None:
            check(False, "the chain produced results", f"{len(tool_results())} tool_result(s)")
            return
        first, second = requests_to(SEARX_LOGS[0]), requests_to(SEARX_LOGS[1])
        check(bool(first), "the first backend was asked", f"{len(first)} request(s)")
        check(
            bool(second),
            "a zero-result answer did not end the chain: the next backend was asked too",
            f"{len(second)} request(s)",
        )
        # Which instance answered, taken from the caller rather than guessed
        # from a log filename: the results themselves are tagged with the port
        # that produced them, and that tag is the only thing that tells the two
        # identical-looking backends apart.
        tag = os.environ.get("QA_EXPECT_TAG", "")
        check(
            bool(tag) and tag in hit,
            f"and the results came from the backend that had any ({tag or 'no tag given'})",
            hit[:200],
        )
        return

    check(False, f"unknown phase {PHASE}")


main()
print(f"\n{'PASS' if rc == 0 else 'FAIL'} ({PHASE})")
sys.exit(rc)
