#!/usr/bin/env python3
"""What the MODEL received — read out of the mock provider's request log.

The tool's own RPC reply is a different object on a different path. A later
request carries an earlier turn's `tool_result` verbatim, so that file is the
only oracle for what the loop actually handed back, and both claims here are
about exactly that:

  reach  — a `grep` call inside a real agent turn comes back with real match
           lines from the planted tree. Every in-process test calls
           `GrepTool::run` directly, which is blind to a tool that is
           registered on three faces and dispatched on none (`plugin_manage`
           shipped that way). A turn is not.

  steer  — a shell `grep -r` comes back with the advisory that names the
           builtin, and two commands that must not be steered do not. The
           negative arms matter more than the positive one: a steer appended
           to every shell call would satisfy the positive arm and be useless.

           Every arm is anchored before it is negated. `STEER not in output`
           is satisfied by an output that never reached the classifier — a
           command that could not run, a tool call that never dispatched —
           so each arm first proves it actually searched, by carrying the
           needle. Without that, the control half of this phase is green on
           any machine where the command is missing, which is exactly the
           machine it was first run on.

## Why nothing here is indexed by turn number

The first version was, and it was wrong on its first run: a run opens with a
strategy-planner call that carries no tool surface, so "turn 2 holds turn 1's
result" is false and the whole driver failed before reaching an assertion. The
mock's counter advances for every HTTP request, including the side-channel
ones, and how many of those a run makes is not this fixture's business. So the
oracle is content: each arm plants a marker only it could produce, and the
driver waits for the tool_result carrying that marker.
"""
import asyncio
import json
import sys
import time

import websockets

URL, PHASE, LOG, NEEDLE = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
BUDGET = float(sys.argv[5]) if len(sys.argv) > 5 else 180.0

# Markers the three `steer` commands echo, so a tool_result can be attributed
# to the arm that produced it without counting turns.
ARM_STEERED = "QA_ARM_STEERED"
ARM_BOUNDED = "QA_ARM_BOUNDED"
ARM_RG = "QA_ARM_RG"
STEER = "duplicates the `grep` tool"

# What a shell says when a program is not installed, for each of the three
# wordings in circulation. Spelled out rather than reduced to "not found"
# because this list decides when an assertion is downgraded to a SKIP, and a
# loose match there recreates the very false green the SKIP exists to prevent.
# Anything a shell says that is NOT on this list falls through to the strict
# assertion and fails loudly, which is the right direction to be wrong in.
MISSING = ("{p}: command not found", "{p}: not found", "command not found: {p}")

rc = 0


def check(ok, label, detail=""):
    global rc
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}" + (f" — {detail}" if detail else ""))
    if not ok:
        rc = 1


def skip(label, detail=""):
    """A claim this machine cannot make. Visible, and not counted as a pass.

    The alternative is a green that means "the command was missing", which
    reads identically to "the classifier let it through".
    """
    print(f"  [SKIP] {label}" + (f" — {detail}" if detail else ""))


def not_installed(program, text):
    """Whether `text` is a shell reporting that `program` does not exist."""
    return any(m.format(p=program) in text for m in MISSING)


def tool_results():
    """Every `tool_result` payload the log has seen, oldest first, deduped.

    A result is carried by every subsequent request, so without the dedup a
    six-turn run reports the same text five times.
    """
    out, seen = [], set()
    try:
        fh = open(LOG)
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
    """Poll the log until some tool_result satisfies `predicate`."""
    end = time.monotonic() + BUDGET
    while time.monotonic() < end:
        for text in tool_results():
            if predicate(text):
                return text
        time.sleep(0.5)
    return None


async def ask():
    """One real message, through the surface a Panel uses."""
    async with websockets.connect(URL, max_size=None) as ws:
        await ws.send(
            json.dumps({"jsonrpc": "2.0", "id": 1, "method": "connect",
                        "params": {"client": "qa-file-search", "version": "1"}})
        )
        await ws.send(
            json.dumps({"jsonrpc": "2.0", "id": 2, "method": "chat.send",
                        "params": {"message": f"find {NEEDLE}",
                                   "channel": "gui:qa-file-search"}})
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
    if not asyncio.run(ask()):
        check(False, "chat.send was accepted")
        return

    if PHASE == "reach":
        hit = wait_for(lambda t: NEEDLE in t and "alpha.rs" in t)
        if hit is None:
            check(
                False,
                "a grep result carrying the planted needle reached the model",
                f"{len(tool_results())} tool_result(s) seen",
            )
            return
        check(True, "a grep result carrying the planted needle reached the model")
        check("src/alpha.rs" in hit, "the match names the file it came from", hit[:200])
        check(
            "node_modules/" not in hit and ".pem" not in hit,
            "the ignored tree and the protected location did not reach the model",
            hit[:300],
        )
        check(
            "no_ignore=true" in hit,
            "the model was told which lever widens the search",
            hit[:300],
        )
        return

    if PHASE == "steer":
        steered = wait_for(lambda t: ARM_STEERED in t)
        if steered is None:
            check(False, "the shell `grep -r` arm ran", f"{len(tool_results())} tool_result(s)")
            return
        check(NEEDLE in steered, "the recursive arm really searched", steered[:200])
        check(STEER in steered, "a shell `grep -r` came back steered to the builtin", steered[:300])

        # The load-bearing control: same program, one property different.
        bounded = wait_for(lambda t: ARM_BOUNDED in t)
        if bounded is None:
            check(False, "the bounded `grep` arm ran", f"{len(tool_results())} tool_result(s)")
            return
        check(NEEDLE in bounded, "the bounded arm really searched", bounded[:200])
        check(
            STEER not in bounded,
            "a single-file `grep` was NOT steered (recursion is the expensive part)",
            bounded[:300],
        )

        # The documented carve-out. Meaningful only where `rg` exists.
        rg = wait_for(lambda t: ARM_RG in t)
        if rg is None:
            check(False, "the shell `rg` arm ran", f"{len(tool_results())} tool_result(s)")
            return
        if not_installed("rg", rg):
            skip("`rg` is not on PATH here, so its carve-out went unexercised", rg[:160])
        else:
            check(NEEDLE in rg, "the `rg` arm really searched", rg[:200])
            check(
                STEER not in rg,
                "a shell `rg` was NOT steered (bash's own description recommends it)",
                rg[:300],
            )
        return

    check(False, f"unknown phase {PHASE}")


main()
sys.exit(rc)
