#!/usr/bin/env python3
"""~/.claude/plugins discovery on a real daemon.

  python3 drive_cc_cache.py <ws-url> first  <request_log> <server_pid> <mcp_mock_path>
      -> listed with origin claude_cache, disabled: its command, its MCP tool
         and its MCP server PROCESS absent; the MODEL's enable
         (`plugin_manage`) is refused on both faces — a real agent turn (the
         mock calls it; the request log is the oracle) and `tools.invoke` —
         and changes nothing; the OPERATOR's (`plugins.enable`, what
         `aleph plugin enable` and the Panel call) enables it, and the mount
         brings the command, the `.mcp.json` server process and its tool up
         with no restart.
  python3 drive_cc_cache.py <ws-url> second <request_log> <server_pid> <mcp_mock_path>
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
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, Rpc, ran, ws_connect  # noqa: E402

WS, PHASE, REQ_LOG = sys.argv[1], sys.argv[2], Path(sys.argv[3])
SERVER_PID, MCP_MOCK = int(sys.argv[4]), str(Path(sys.argv[5]).resolve())
L = Ledger()
PLUGIN = "qa-cc"
SLASH = "qa-cc:hello"
ECHO_SUFFIX = "__qa_echo"
COMMAND_BODY = "QA_CC_COMMAND_BODY"
# `plugin_manage::may_enable` for a claude_cache row: `AlephError::PermissionDenied`,
# whose rendering leads with this label (P4.13a O-1; before it,
# "Configuration/Database error:").
POLICY_LABEL = f"Permission denied: '{PLUGIN}' was installed by Claude Code"
OLD_LABEL = "Configuration/Database error"
# `path_utils` denial of a pre-grant root (R2-I2), pinned to its current cause.
PROTECTED = "is in a protected location"


def mcp_server_running():
    """Whether the plugin's MCP server process runs under THIS daemon: a
    descendant of the server pid whose command line names the mock (review
    M-4 — the spawn itself, not a catalog read that could precede it)."""
    out = subprocess.run(["ps", "-axo", "pid=,ppid=,command="], capture_output=True, text=True).stdout
    procs = []
    for line in out.splitlines():
        parts = line.split(None, 2)
        if len(parts) == 3 and parts[0].isdigit() and parts[1].isdigit():
            procs.append((int(parts[0]), int(parts[1]), parts[2]))
    tree, grew = {SERVER_PID}, True
    while grew:
        grew = False
        for pid, ppid, _ in procs:
            if ppid in tree and pid not in tree:
                tree.add(pid)
                grew = True
    return any(pid in tree and MCP_MOCK in cmd for pid, _, cmd in procs)


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


async def run_turn(message, session_key, seconds=150):
    """One agent turn on its own connection, awaited to its terminal frame."""
    async with ws_connect(WS) as ws:
        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "connect",
                                  "params": {"client_info": {"name": "qa-cc-cache-turn"}}}))
        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 2, "method": "chat.send",
                                  "params": {"message": message, "session_key": session_key}}))
        run_id, ended, end = None, {}, time.monotonic() + seconds
        while time.monotonic() < end:
            if run_id in ended:
                return ended[run_id]
            try:
                m = json.loads(await asyncio.wait_for(ws.recv(), timeout=1.0))
            except asyncio.TimeoutError:
                continue
            if m.get("id") == 2:
                L.check(f"chat.send accepted {message!r}", "result" in m, json.dumps(m.get("error", ""))[:200])
                run_id = (m.get("result") or {}).get("run_id")
                if run_id is None:
                    return "rejected"
            elif m.get("method") in ("stream.run_complete", "stream.run_error"):
                ended[(m.get("params") or {}).get("run_id")] = m["method"]
        return "no_terminal_frame"


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
    # Review I-1: the command half of "disabled" — `commands_count: 1` is on
    # the disabled row already, so only the listing can say it is not live.
    L.check("a disabled plugin's command is NOT in commands.list",
            SLASH not in json.dumps(await rpc.call("commands.list", {})))
    L.check("a disabled plugin's MCP server process is NOT running (under this daemon)",
            not mcp_server_running())

    L.log("\n--- the model may not enable it (P4.10 D-A): a real agent turn ---")
    outcome = await run_turn(f"turn on the {PLUGIN} plugin", "agent:main:qa-cc-enable")
    L.check("the model's turn reached a terminal frame", outcome == "stream.run_complete", outcome)
    results = tool_results()
    refusal = next((t for t in results if "installed by Claude Code" in t), None)
    L.check("the model's plugin_manage enable came back refused", refusal is not None,
            f"{len(results)} tool_result(s): {[t[:80] for t in results][:3]}")
    if refusal is not None:
        L.check("the model reads the policy label, not a config/DB error (P4.13a O-1)",
                POLICY_LABEL in refusal and OLD_LABEL not in refusal, refusal[:240])
        L.check("the refusal names no way around it (no switch= / ladder)",
                "switch=" not in refusal and "ladder_must" not in refusal, refusal[:240])

    L.log("\n--- the same refusal on the RPC face (tools.invoke) ---")
    ok, body = await rpc.invoke("plugin_manage", {"action": "enable", "name": PLUGIN})
    text = json.dumps(body)
    L.check("plugin_manage enable is REFUSED for a claude_cache row",
            not ran(ok, body) and "installed by Claude Code" in text, text[:240])
    L.check("…with the policy label, not a config/DB error (P4.13a O-1)",
            POLICY_LABEL in text and OLD_LABEL not in text, text[:240])
    r, _ = await row(rpc)
    L.check("the row stays disabled after the model's attempts",
            r is not None and r.get("status") == "disabled" and not r.get("enabled"), str(fields(r)))
    L.check("…its command stays out of commands.list",
            SLASH not in json.dumps(await rpc.call("commands.list", {})))
    L.check("…its MCP tool stays out of tools.catalog", not has_echo(await catalog(rpc)))
    L.check("…and its MCP server process never started", not mcp_server_running())

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
    L.check("…and its process runs under this daemon", mcp_server_running())


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
    # Pinned to the CURRENT cause (review M-5): the pre-grant root rule, on
    # this fixture's path. The same read denied by some other rule is a change.
    L.check("…by the protected-location rule, on the fixture's own path",
            denied is not None and PROTECTED in denied and "/.claude/plugins/cache/qa-market/qa-cc/" in denied,
            (denied or "")[:240])
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
