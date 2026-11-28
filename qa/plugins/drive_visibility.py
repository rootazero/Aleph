#!/usr/bin/env python3
"""Verify plugin visibility against the real Gateway and the mock provider's requests.

The two fixtures share an ID and a server name. Requests inside the project
must expose the project copy. Sessionless requests must expose the global
copy. Once the project copy is disabled, the project must see neither.
"""
import asyncio
import hashlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, ws_connect  # noqa: E402

WS, PROJECT, REQ_LOG, PHASE = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4]
L = Ledger()
PLUGIN = "qa-vis"
GLOBAL_TOOL = "plugin_qa-vis_vis__qa_echo"


def project_tool_name(selector):
    scope_digest = selector.removeprefix("p:")
    server_id = f"plugin:p_{scope_digest}_{PLUGIN}/vis"
    server = hashlib.sha256(b"mcp-server\0" + server_id.encode()).hexdigest()[:30]
    tool = hashlib.sha256(b"mcp-tool\0qa_echo").hexdigest()[:32]
    return f"p_{server}{tool}"


class Conn:
    """One websocket. Notifications that arrive while a reply is awaited are
    kept, so a terminal frame racing its own `chat.send` reply is not lost."""

    def __init__(self, ws):
        self.ws, self.frames, self._id = ws, [], 0

    async def call(self, method, params):
        self._id += 1
        rid = self._id
        await self.ws.send(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}))
        while True:
            msg = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=60))
            if msg.get("id") == rid:
                return msg
            if "method" in msg:
                self.frames.append(msg)

    async def terminal(self, run_id, seconds=90):
        """`stream.run_complete` / `stream.run_error` for `run_id`, or a miss."""
        def match(msg):
            return msg.get("method") in ("stream.run_complete", "stream.run_error") \
                and (msg.get("params") or {}).get("run_id") == run_id
        for msg in self.frames:
            if match(msg):
                return msg["method"]
        deadline = asyncio.get_event_loop().time() + seconds
        while asyncio.get_event_loop().time() < deadline:
            try:
                msg = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=1.0))
            except asyncio.TimeoutError:
                continue
            if match(msg):
                return msg["method"]
        return "no_terminal_frame"


async def run(conn, message, session_key, project_root):
    payload = {"message": message, "session_key": session_key, "stream": True}
    if project_root is not None:
        payload["project_root"] = str(project_root)
    sent = await conn.call("chat.send", payload)
    if "error" in sent:
        return f"chat.send rejected: {json.dumps(sent['error'])[:200]}"
    return await conn.terminal(sent["result"]["run_id"])


def requests_of(marker):
    """Every request body the mock recorded whose messages carry `marker`."""
    if not REQ_LOG.exists():
        return []
    out = []
    for line in REQ_LOG.read_text().splitlines():
        body = json.loads(line)["body"]
        if marker in json.dumps(body.get("messages", [])):
            out.append(body)
    return out


def system_text(body):
    # Always an array of blocks on the wire (anthropic adapter); dumping it
    # keeps the substring test independent of the block layout.
    return json.dumps(body.get("system", ""))


def tool_names(body):
    return [t.get("name", "") for t in body.get("tools") or [] if isinstance(t, dict)]


async def register(conn):
    res = await conn.call("projects.add", {"path": str(PROJECT), "name": "qa-vis"})
    L.check("projects.add registers the folder", "result" in res, json.dumps(res)[:200])
    wp = ((res.get("result") or {}).get("project") or {}).get("workspace_path")
    L.check("the row carries the workspace_path discovery reads",
            wp is not None and Path(wp).resolve() == PROJECT.resolve(), f"workspace_path={wp!r}")


async def preconditions(conn):
    latest = {}
    for _ in range(60):
        rows = ((await conn.call("plugins.list", {})).get("result") or {}).get("plugins", [])
        latest = {row.get("scope_kind"): row for row in rows if row.get("name") == PLUGIN}
        if all(latest.get(scope, {}).get("status") == "loaded" for scope in ("project", "global")):
            break
        await asyncio.sleep(0.5)
    L.check("both exact instances loaded and completed MCP startup",
            all(latest.get(scope, {}).get("status") == "loaded" for scope in ("project", "global")),
            json.dumps(latest)[:600])
    selector = latest.get("project", {}).get("selector")
    L.check("project instance supplies an exact selector", bool(selector and selector.startswith("p:")))
    return selector


async def check_run(conn, marker, project_root, expected_copy, project_tool):
    outcome = await run(conn, marker, f"agent:main:{marker}", project_root)
    L.check(f"{marker}: run reached a terminal frame", outcome == "stream.run_complete", outcome)
    bodies = requests_of(marker)
    main = [body for body in bodies if tool_names(body)]
    L.check(f"{marker}: a main request reached the model", bool(main), str(len(bodies)))
    L.check(f"{marker}: skill index and agent catalog rendered",
            any("<available_skills>" in system_text(body) for body in main)
            and any("<available_agents>" in system_text(body) for body in main))

    for copy, tool in [("project", project_tool), ("global", GLOBAL_TOOL)]:
        wanted = expected_copy == copy
        for contribution in ("skill", "agent"):
            name = f"qa-vis-{copy}-{contribution}"
            seen = any(name in json.dumps(body) for body in bodies)
            L.check(f"{marker}: {name} visible={wanted}", seen == wanted)
        offered = any(tool in tool_names(body) for body in main)
        L.check(f"{marker}: {copy} MCP tool visible={wanted}", offered == wanted, tool)


async def check_command(conn, marker, project_root, expected_copy):
    outcome = await run(conn, f"/{PLUGIN}:echo {marker}", f"agent:main:{marker}", project_root)
    L.check(f"{marker}: command turn reached a terminal frame",
            outcome == "stream.run_complete", outcome)
    rendered = json.dumps(requests_of(marker))
    for copy in ("project", "global"):
        wanted = expected_copy == copy
        seen = f"QA_COMMAND_{copy}: {marker}" in rendered
        L.check(f"{marker}: {copy} command rendered={wanted}", seen == wanted)


async def probe(conn):
    selector = await preconditions(conn)
    if not selector:
        return
    project_tool = project_tool_name(selector)
    await check_run(conn, "qa-vis-inside", PROJECT, "project", project_tool)
    await check_run(conn, "qa-vis-outside", None, "global", project_tool)
    await check_command(conn, "qa-vis-project-command", PROJECT, "project")
    await check_command(conn, "qa-vis-global-command", None, "global")

    response = await conn.call("plugins.disable", {"selector": selector})
    L.check("operator disabled the exact project copy", "result" in response, json.dumps(response)[:400])
    await check_run(conn, "qa-vis-disabled", PROJECT, None, project_tool)
    await check_command(conn, "qa-vis-shadowed-command", PROJECT, None)
    await check_run(conn, "qa-vis-global-still-live", None, "global", project_tool)


async def main():
    async with ws_connect(WS) as ws:
        conn = Conn(ws)
        await conn.call("connect", {"client_type": "cli"})
        if PHASE == "register":
            await register(conn)
        elif PHASE == "probe":
            await probe(conn)
        else:
            L.check(f"known phase (register | probe), got {PHASE!r}", False)
    return L.verdict()


sys.exit(asyncio.run(main()))
