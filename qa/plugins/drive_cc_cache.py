#!/usr/bin/env python3
"""~/.claude/plugins discovery on a real daemon.

  python3 drive_cc_cache.py <ws-url> first  <request_log>
      -> listed with origin claude_cache, disabled, its MCP tool absent; the
         MODEL's enable (`plugin_manage`) is refused and changes nothing; the
         OPERATOR's (`plugins.enable`, what `aleph plugin enable` and the
         Panel call) enables it, and the mount brings the command and the
         `.mcp.json` server up with no restart.
  python3 drive_cc_cache.py <ws-url> second <request_log>
      -> after a restart: still enabled, command and server back at boot; and
         the R2-I2 observation — the model's `file_read` of a file under the
         CC plugin root gets an access-denied tool result.

The plugin's registry id is its manifest name (`qa-cc`): `qa-cc@qa-market` is
only the key `installed_plugins.json` files it under (P4.10, D-B bare id).

Every claim about the mount is settled on what a client reads
(`commands.list`, `tools.catalog`), never on the enable call's own reply.
"""
import asyncio
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, Rpc, ran, ws_connect  # noqa: E402

WS, PHASE, REQ_LOG = sys.argv[1], sys.argv[2], Path(sys.argv[3])
L = Ledger()
PLUGIN = "qa-cc"
SLASH = "qa-cc:hello"
ECHO_SUFFIX = "__qa_echo"
COMMAND_BODY = "QA_CC_COMMAND_BODY"


def names(node, out):
    """Every `name` string anywhere in an RPC result (envelope-agnostic, as in
    `drive_scope.py`: a moved envelope reads as "absent", not a KeyError)."""
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
    return names((await rpc.call("tools.catalog", {})).get("result", {}), set())


def has_echo(cat):
    return any(n.endswith(ECHO_SUFFIX) for n in cat)


async def poll_echo(rpc, label, seconds=30):
    """The MCP half is asynchronous: the tool bridge registers the tool when
    the handshake completes."""
    for _ in range(seconds * 2):
        if has_echo(await catalog(rpc)):
            L.check(label, True)
            return
        await asyncio.sleep(0.5)
    L.check(label, False, f"timed out after {seconds}s; catalog: {sorted(await catalog(rpc))[:20]}")


async def row(rpc):
    rows = ((await rpc.call("plugins.list", {})).get("result") or {}).get("plugins", [])
    return next((r for r in rows if r.get("name") == PLUGIN), None), [r.get("name") for r in rows]


def fields(r):
    return {k: r.get(k) for k in ("origin", "status", "enabled", "mcp_servers_count", "commands_count")} if r else None


async def first(rpc):
    r, all_names = await row(rpc)
    L.log("plugins.list row ->", fields(r))
    L.check("the Claude Code-installed plugin is discovered", r is not None, f"names: {sorted(all_names)[:10]}")
    if r is None:
        return
    L.check("listed with origin claude_cache", r.get("origin") == "claude_cache", str(r.get("origin")))
    L.check("disabled until Aleph is told otherwise",
            r.get("status") == "disabled" and not r.get("enabled"), f"{r.get('status')} enabled={r.get('enabled')}")
    L.check("a disabled plugin's MCP tool is NOT in tools.catalog", not has_echo(await catalog(rpc)))

    L.log("\n--- the model may not enable it (P4.10 D-A) ---")
    ok, body = await rpc.invoke("plugin_manage", {"action": "enable", "name": PLUGIN})
    text = json.dumps(body)
    L.check("plugin_manage enable is REFUSED for a claude_cache row",
            not ran(ok, body) and "installed by Claude Code" in text, text[:240])
    r, _ = await row(rpc)
    L.check("the row stays disabled after the model's attempt",
            r is not None and r.get("status") == "disabled" and not r.get("enabled"), str(fields(r)))
    L.check("…and its MCP tool stays out of tools.catalog", not has_echo(await catalog(rpc)))

    # Observation only: the `installed_plugins.json` key is not the id.
    keyed = await rpc.call("plugins.enable", {"name": "qa-cc@qa-market"})
    L.log("  (observation) plugins.enable {name: 'qa-cc@qa-market'} ->",
          json.dumps(keyed.get("error") or keyed.get("result"))[:160])

    L.log("\n--- the operator enables it (plugins.enable = `aleph plugin enable` / Panel) ---")
    res = await rpc.call("plugins.enable", {"name": PLUGIN})
    L.check("plugins.enable accepted", (res.get("result") or {}).get("ok") is True,
            json.dumps(res.get("error") or res.get("result"))[:200])
    r, _ = await row(rpc)
    L.log("plugins.list row ->", fields(r))
    L.check("enabled after the operator's verb", r is not None and r.get("enabled") is True
            and r.get("status") == "loaded", str(fields(r)))
    # One reader feeds the count and the spawn (P4.15 / D-1).
    n = r.get("mcp_servers_count") if r else None
    L.check("the row counts the one .mcp.json server (mcp_servers_count == 1)", n == 1, str(n))
    cmds = json.dumps(await rpc.call("commands.list", {}))
    L.check("the command is registered by the mount (no restart)", SLASH in cmds, f"payload {len(cmds)}B")
    await poll_echo(rpc, "the .mcp.json server (no aleph.runtime) mounted: *__qa_echo is in tools.catalog")


def tool_results():
    if not REQ_LOG.exists():
        return []
    out = []
    for line in REQ_LOG.read_text().splitlines():
        if not line.strip():
            continue
        for m in json.loads(line)["body"].get("messages", []):
            c = m.get("content")
            if isinstance(c, list):
                for b in c:
                    if isinstance(b, dict) and b.get("type") == "tool_result":
                        inner = b.get("content")
                        out.append(inner if isinstance(inner, str) else json.dumps(inner))
    return out


async def second(rpc):
    r, _ = await row(rpc)
    L.log("plugins.list row ->", fields(r))
    L.check("the enable survived a restart (plugins.toml, not ~/.claude)",
            r is not None and r.get("enabled") is True and r.get("status") == "loaded", str(fields(r)))
    n = r.get("mcp_servers_count") if r else None
    L.check("mcp_servers_count is still 1 after boot", n == 1, str(n))
    cmds = json.dumps(await rpc.call("commands.list", {}))
    L.check("the cached plugin's command is registered after boot", SLASH in cmds, f"payload {len(cmds)}B")
    await poll_echo(rpc, "the MCP server mounts again at boot")

    L.log("\n--- R2-I2 (accepted compat cost, P4.14 DEVIATION row; candidate plugin-resource face) ---")
    sent = await rpc.call("chat.send", {"message": "read the plugin's command file",
                                        "session_key": "agent:main:qa-cc-read"})
    L.check("chat.send accepted", "result" in sent, json.dumps(sent.get("error", ""))[:200])
    end, denied = time.monotonic() + 120, None
    while time.monotonic() < end and denied is None:
        denied = next((t for t in tool_results() if "Access denied" in t), None)
        if denied is None:
            await asyncio.sleep(0.5)
    results = tool_results()
    L.check("file_read under ~/.claude/plugins/cache/… is denied to the model",
            denied is not None, (denied or f"{len(results)} tool_result(s): {[t[:80] for t in results]}")[:240])
    L.check("the command file's content never reached the model", all(COMMAND_BODY not in t for t in results))


async def main():
    async with ws_connect(WS) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-cc-cache")
        if PHASE == "first":
            await first(rpc)
        elif PHASE == "second":
            await second(rpc)
        else:
            L.check(f"known phase (first | second), got {PHASE!r}", False)
    return L.verdict()


sys.exit(asyncio.run(main()))
