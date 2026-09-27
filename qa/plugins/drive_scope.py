#!/usr/bin/env python3
"""enable → disable → enable of `qa-scope`; the catalogue must change each time.

Two entries are watched in `tools.catalog`: the plugin's slash command
(`qa-scope:qa-scope-cmd`, registered by the `slash_command` effect) and the
mock server's tool (name ends with `__qa_echo`; the exact prefix is the
sanitised server id, which this driver does not re-derive). Presence is
polled, because the MCP half is asynchronous on purpose: the mount enqueues
the server start and the tool bridge registers the tool when the handshake
completes.

Every claim about a flip is settled on the catalogue a client reads, never on
`plugin_manage`'s own return value — the RPC saying "disabled" is the call,
the entry leaving the catalogue is the effect (判据 §4).
"""
import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, Rpc, ran, ws_connect  # noqa: E402

WS = sys.argv[1]
L = Ledger()
PLUGIN = "qa-scope"
SLASH = "qa-scope:qa-scope-cmd"
ECHO_SUFFIX = "__qa_echo"


def names(node, out):
    """Every `name` string anywhere in the `tools.catalog` result.

    Envelope-agnostic on purpose (the same reason `drive_plugins.py::rows`
    walks): `groups[].tools[].name` today, and a moved envelope must read as
    "entry absent" — a failure — rather than a KeyError that looks like a
    transport problem.
    """
    if isinstance(node, dict):
        if isinstance(node.get("name"), str):
            out.add(node["name"])
        for v in node.values():
            names(v, out)
    elif isinstance(node, list):
        for v in node:
            names(v, out)
    return out


async def catalog(rpc):
    msg = await rpc.call("tools.catalog", {})
    return names(msg.get("result", {}), set())


def watched(cat):
    return SLASH in cat, any(n.endswith(ECHO_SUFFIX) for n in cat)


async def poll(rpc, want_present, label, seconds=30):
    """Poll until both watched entries are (present|absent), or time out."""
    for _ in range(seconds * 2):
        slash, echo = watched(await catalog(rpc))
        if slash == want_present and echo == want_present:
            L.check(label, True, f"slash={slash} echo={echo}")
            return True
        await asyncio.sleep(0.5)
    slash, echo = watched(await catalog(rpc))
    L.check(label, False, f"timed out after {seconds}s; slash={slash} echo={echo}")
    return False


async def row(rpc):
    msg = await rpc.call("plugins.list", {})
    for r in msg.get("result", {}).get("plugins", []):
        if r.get("name") == PLUGIN:
            return r
    return None


async def status(rpc):
    r = await row(rpc)
    return r.get("status") if r else None


async def flip(rpc, action):
    ok, body = await rpc.invoke("plugin_manage", {"action": action, "name": PLUGIN})
    changed = (body or {}).get("data", {}).get("changed") if isinstance(body, dict) else None
    L.check(f"plugin_manage {action} answered", ran(ok, body) and changed is True,
            f"changed={changed} {str(body)[:160]}")


async def main():
    async with ws_connect(WS) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-scope")

        L.log("\n--- boot: the plugin is mounted by load_all ---")
        r = await row(rpc)
        L.log("plugins.list row ->", {k: r.get(k) for k in ("status", "kind", "commands_count", "mcp_servers_count")} if r else None)
        # One reader feeds the count and the spawn (P4.15 / D-1). Before it,
        # this row said 0 beside a running server: the count's reader dropped
        # the absolute `sys.executable` command the spawn's reader ran.
        n = r.get("mcp_servers_count") if r else None
        L.check("plugins.list counts the one server the mount spawns", n == 1, str(n))
        st = r.get("status") if r else None
        L.check("plugins.list shows qa-scope loaded", st == "loaded", str(st))
        await poll(rpc, True, "slash command AND mock tool are in tools.catalog after boot")

        L.log("\n--- disable ---")
        await flip(rpc, "disable")
        st = await status(rpc)
        L.check("plugins.list shows disabled", st == "disabled", str(st))
        await poll(rpc, False, "both entries left tools.catalog on disable")

        L.log("\n--- enable again (the direction that used to do nothing) ---")
        await flip(rpc, "enable")
        st = await status(rpc)
        L.check("plugins.list shows loaded again", st == "loaded", str(st))
        await poll(rpc, True, "both entries are back in tools.catalog on enable")

    return L.verdict()


sys.exit(asyncio.run(main()))
