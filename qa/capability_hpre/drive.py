#!/usr/bin/env python3
"""Real-binary H-pre runtime-mount probe.

This driver deliberately uses only public gateway/client surfaces.  It treats
catalogue and provider request bodies as evidence of delivered effects, and
reports missing ownership/slow-consumer controls as UNVERIFIED rather than
inventing a diagnostic route.
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time
from pathlib import Path

QA = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(QA / "browser_managed"))
from qa_rpc import Ledger, Rpc, ran, ws_connect  # noqa: E402


class FloorLedger(Ledger):
    def __init__(self):
        super().__init__()
        self.assertions = 0
        self.receipts = 0
        self.unverified = []

    def check(self, claim, ok, detail=""):
        self.assertions += 1
        return super().check(claim, ok, detail)

    def receipt(self, claim, ok, detail=""):
        self.receipts += 1
        return self.check(claim, ok, detail)

    def gap(self, claim, detail):
        self.unverified.append(f"{claim}: {detail}")
        print(f"  [UNVERIFIED] {claim} — {detail}", flush=True)


def walk_names(node, out):
    if isinstance(node, dict):
        value = node.get("name")
        if isinstance(value, str):
            out.add(value)
        for child in node.values():
            walk_names(child, out)
    elif isinstance(node, list):
        for child in node:
            walk_names(child, out)
    return out


def request_bodies(path: Path):
    result = []
    if not path.exists():
        return result
    for line in path.read_text(errors="replace").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            # The mock log wraps the actual provider request under `body`;
            # inspect that request so `tools` proves model-visible delivery,
            # while retaining a fallback for an unwrapped fixture log.
            request = value.get("body")
            result.append(request if isinstance(request, dict) else value)
    return result


def body_has_marker(body, marker):
    return marker in json.dumps(body, sort_keys=True)


def body_tools(body):
    tools = body.get("tools", [])
    result = set()
    if isinstance(tools, list):
        for tool in tools:
            if isinstance(tool, dict) and isinstance(tool.get("name"), str):
                result.add(tool["name"])
    return result


async def catalog(rpc):
    msg = await rpc.call("tools.catalog", {})
    return walk_names(msg.get("result", {}), set())


async def wait_catalog(rpc, suffix, present=True, timeout=30):
    end = time.monotonic() + timeout
    latest = set()
    while time.monotonic() < end:
        latest = await catalog(rpc)
        found = sorted(name for name in latest if name.endswith(suffix))
        if bool(found) == present:
            return found, latest
        await asyncio.sleep(0.5)
    return sorted(name for name in latest if name.endswith(suffix)), latest


async def invoke(rpc, ledger, method, arguments):
    """Call a public gateway method, not a `tools.invoke` tool name.

    `tools.catalog`, `mcp_config.*`, and `chat.send` are registered gateway
    methods.  Routing them through `tools.invoke` asks the tool registry for
    names that do not exist and would turn a healthy server into a false
    negative.
    """
    msg = await rpc.call(method, arguments)
    if "error" in msg:
        ok, body = False, {"rpc_error": msg["error"]}
    else:
        ok, body = True, msg.get("result", {})
    ledger.check(f"gateway {method} success", ran(ok, body), json.dumps(body)[:240])
    return ok, body


async def wait_marker_bodies(path: Path, marker: str, timeout=30):
    """Wait for the provider's recorded effect, bounded by a monotonic deadline."""
    end = time.monotonic() + timeout
    latest = []
    while time.monotonic() < end:
        latest = [b for b in request_bodies(path) if body_has_marker(b, marker)]
        # The real loop may first issue a planning request without tools, then
        # the executor request that carries the mounted consumer surface.  Do
        # not stop at the first marker match or the effect oracle races itself.
        if any(body_tools(b) for b in latest):
            return latest
        await asyncio.sleep(0.25)
    return latest


async def send_marker(rpc, marker, ledger, provider_log):
    # chat.send is the canonical real AgentLoop/run-loop path.  The mock
    # provider is intentionally the only model endpoint; the request log is
    # the effect-level oracle for the model-visible tool surface.
    ok, body = await invoke(rpc, ledger, "chat.send", {"message": marker})
    ledger.receipt("chat.send reached the real run loop", ok and bool(body), json.dumps(body)[:240])
    ledger.receipt("chat.send returned a nonempty consumer receipt", bool(body), json.dumps(body)[:240])
    return ok, body, await wait_marker_bodies(provider_log, marker)


async def setup_server(rpc, ledger, mcp_script, name, python):
    args = {"name": name, "config": {
        "command": python,
        "args": [str(mcp_script)],
        "env": {},
        "enabled": True,
    }}
    return await invoke(rpc, ledger, "mcp_config.create", args)


async def delete_server(rpc, ledger, server_id):
    return await invoke(rpc, ledger, "mcp_config.delete", {"id": server_id})


async def initial(rpc, ledger, args):
    name = args.prefix + "_initial"
    suffix = "__qa_echo"
    await setup_server(rpc, ledger, args.mcp_script, name, args.python)
    hits, _ = await wait_catalog(rpc, suffix)
    hits = [hit for hit in hits if hit.startswith(name + "__")]
    ledger.receipt("initial MCP tool is delivered to tools.catalog", bool(hits), str(hits))
    if not hits:
        return
    marker = "QA_HPRE_INITIAL_" + args.run_id
    _, _, bodies = await send_marker(rpc, marker, ledger, args.provider_log)
    tools = set().union(*(body_tools(b) for b in bodies)) if bodies else set()
    ledger.receipt("initial provider request was recorded", bool(bodies), f"{len(bodies)} request(s)")
    ledger.receipt("initial marker attribution has a nonempty provider body", any(bool(body) for body in bodies), f"{len(bodies)} request(s)")
    ledger.receipt("initial provider request carries the mounted MCP tool", any(h in tools for h in hits), sorted(tools))


async def replacement(rpc, ledger, args):
    old = args.prefix + "_replace_old"
    new = args.prefix + "_replace_new"
    suffix = "__qa_echo"
    await setup_server(rpc, ledger, args.mcp_script, old, args.python)
    old_hits, _ = await wait_catalog(rpc, suffix)
    old_hits = [hit for hit in old_hits if hit.startswith(old + "__")]
    ledger.receipt("replacement precondition has an old MCP tool", bool(old_hits), str(old_hits))
    if not old_hits:
        return
    marker_old = "QA_HPRE_REPLACE_OLD_" + args.run_id
    _, _, _ = await send_marker(rpc, marker_old, ledger, args.provider_log)
    # derive_server_id preserves ASCII alnum, _, -, and .; it does not add a prefix.
    old_id = "".join(c if (c.isalnum() and c.isascii()) or c in "_-." else "_" for c in old)
    await delete_server(rpc, ledger, old_id)
    end = time.monotonic() + 30
    removed = []
    while time.monotonic() < end:
        current = await catalog(rpc)
        removed = sorted(name for name in current if name.startswith(old + "__"))
        if not removed:
            break
        await asyncio.sleep(0.5)
    ledger.receipt("old MCP tool is removed before replacement", not removed, str(removed))
    await setup_server(rpc, ledger, args.mcp_script, new, args.python)
    new_hits, _ = await wait_catalog(rpc, suffix)
    new_hits = [hit for hit in new_hits if hit.startswith(new + "__")]
    ledger.receipt("replacement MCP tool is delivered", bool(new_hits), str(new_hits))
    ledger.receipt(
        "replacement consumer identity changed",
        bool(new_hits) and set(new_hits).isdisjoint(old_hits),
        f"old={old_hits} new={new_hits}",
    )
    if new_hits:
        marker_new = "QA_HPRE_REPLACE_NEW_" + args.run_id
        _, _, bodies = await send_marker(rpc, marker_new, ledger, args.provider_log)
        tools = set().union(*(body_tools(b) for b in bodies)) if bodies else set()
        ledger.receipt("replacement provider request was recorded", bool(bodies), f"{len(bodies)} request(s)")
        ledger.receipt("replacement marker attribution has a nonempty provider body", any(bool(body) for body in bodies), f"{len(bodies)} request(s)")
        ledger.receipt("replacement provider request carries the new MCP tool", any(h in tools for h in new_hits), sorted(tools))
    ledger.gap("replacement cursor receipt", "the public consumer surfaces expose changed tool identity, not registry cursor/generation receipts")


async def unsupported_controls(ledger, scenario):
    if scenario == "ownership":
        ledger.gap(
            "OwnershipTree invalidation control",
            "no legitimate external OwnershipTree bump/revoke/dispose control exists",
        )
        ledger.gap(
            "Invalidated plus replacement snapshot",
            "the public consumer surface cannot force or observe owner invalidation and replacement delivery",
        )
        ledger.gap(
            "ownership registry cursor invariant",
            "the public catalog/provider surfaces do not expose the registry cursor needed to assert it is unchanged",
        )
    elif scenario == "close":
        ledger.gap(
            "ProjectionHost close control",
            "no legitimate external ProjectionHost hold/inspect/close control exists",
        )
        ledger.gap(
            "close waits for delivery completion",
            "the public surface cannot observe close/join completion before registry teardown",
        )
        ledger.gap(
            "post-close mutation is absent",
            "the public surface cannot mutate after close and observe that no event arrives",
        )
    elif scenario == "overflow":
        ledger.gap(
            "slow-consumer hold control",
            "no legitimate external consumer hold/overflow control exists; the host's 64-event policy cannot be driven externally",
        )
        ledger.gap(
            "64-event per-consumer overflow replacement",
            "the public surface cannot fill one consumer queue and observe its independent replacement snapshot",
        )
        ledger.gap(
            "256-event registry ring independence",
            "the public surface cannot compare per-consumer overflow against the registry's separate 256-slot notification ring",
        )


async def main(args):
    ledger = FloorLedger()
    try:
        async with ws_connect(args.ws) as ws:
            rpc = Rpc(ws)
            await rpc.connect("qa-capability-hpre")
            await invoke(rpc, ledger, "tools.catalog", {})
            if args.scenario in ("all", "initial"):
                await initial(rpc, ledger, args)
            if args.scenario in ("all", "replacement"):
                await replacement(rpc, ledger, args)
            if args.scenario in ("all", "ownership"):
                await unsupported_controls(ledger, "ownership")
            if args.scenario in ("all", "close"):
                await unsupported_controls(ledger, "close")
            if args.scenario in ("all", "overflow"):
                await unsupported_controls(ledger, "overflow")
    except Exception as exc:
        ledger.check("driver completed without transport/runtime error", False, repr(exc))

    if args.scenario in ("all", "initial", "replacement"):
        ledger.check("positive assertion floor (>= 8 assertions)", ledger.assertions >= 8, str(ledger.assertions))
        ledger.check("positive receipt floor (>= 3 effect-level receipts)", ledger.receipts >= 3, str(ledger.receipts))
    else:
        # An unsupported control must not inherit a green-looking evidence
        # floor.  The explicit gap below keeps this run UNVERIFIED, while a
        # supported initial/replacement run with no receipt remains a FAIL.
        ledger.gap("positive evidence floor", f"{args.scenario} has no externally drivable positive control")

    print(f"ASSERTION_COUNT: {ledger.assertions}", flush=True)
    print(f"RECEIPT_COUNT: {ledger.receipts}", flush=True)
    if ledger.unverified:
        print("UNVERIFIED_GAPS:", flush=True)
        for gap in ledger.unverified:
            print(f"  - {gap}", flush=True)
    if ledger.failures:
        print(f"VERDICT: FAIL ({len(ledger.failures)} claim(s))", flush=True)
        return 1
    if ledger.unverified:
        print("VERDICT: UNVERIFIED (external controls unavailable; no PASS claimed)", flush=True)
        return 3
    print("VERDICT: PASS", flush=True)
    return 0


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--ws", required=True)
    parser.add_argument("--provider-log", type=Path, required=True)
    parser.add_argument("--mcp-script", type=Path, required=True)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--scenario", choices=("all", "initial", "replacement", "ownership", "close", "overflow"), default="all")
    parser.add_argument("--prefix", default="qa_hpre")
    parser.add_argument("--run-id", default=str(os.getpid()))
    return parser.parse_args()


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main(parse_args())))
