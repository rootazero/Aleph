#!/usr/bin/env python3
"""Plugin visibility on a real daemon: what the MODEL is shown, per run.

  python3 drive_visibility.py <ws-url> <project_root> <request_log> register
      -> projects.add the folder (loopback = operator); checks the row.
  python3 drive_visibility.py <ws-url> <project_root> <request_log> probe
      -> two chat.send runs, one with project_root and one without, then
         reads what the MOCK PROVIDER was sent in each run.

The oracle is the mock's request log, never an RPC reply: the claim is about
what the model was shown, and the only witness to that is the request body.
`plugins.list` and `tools.catalog` are management faces and list the plugin
for everyone; they are read here only as preconditions ("was it discovered",
"has its MCP server finished the handshake"), so that a missing entry in a
request reads as a visibility answer and not as a plugin that never loaded.

Three faces, one owner (`qa-vis`, planted by `plant_visibility.py`), each
asserted where it lands in the project-bound run:
  * skill  — the `<available_skills>` index in `system`;
  * agent  — the `<available_agents>` catalog in `system`;
  * MCP    — the joined tool in `tools[]` (`defer_mcp_tools` is off by default,
             so it is not in a deferred catalog).
The project-less run's negatives read the WHOLE request instead: the skill
also reaches a project-bound run through a second face, the project-skills
`<system-reminder>` in `messages` (`collect_project_skill_block`), and a leak
through any face is the same leak.

Requests are attributed to a run by the run's own message text, not by log
position: a run also makes side-channel provider calls (planning, titling)
that may land after its terminal frame, and a positional window would hand
run 1's late call to run 2.
"""
import asyncio
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, ws_connect  # noqa: E402

WS, PROJECT, REQ_LOG, PHASE = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4]
L = Ledger()
PLUGIN, SKILL, AGENT = "qa-vis", "qa-vis-skill", "qa-vis-agent"
# `McpHandler::qualified_name` of `plugin:qa-vis/vis` + `qa_echo`, as observed
# in the request log on the first green run (2026-09-23). Asserted exactly on
# the positive arm; the negative arm refuses ANY `__qa_echo` spelling, so a
# changed sanitiser cannot turn a leak into a pass.
MCP_TOOL = "plugin_qa-vis_vis__qa_echo"
MCP_SUFFIX = "__qa_echo"
IN_MARK, OUT_MARK = "hello from inside", "hello from nowhere"


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


async def register(conn):
    res = await conn.call("projects.add", {"path": str(PROJECT), "name": "qa-vis"})
    L.check("projects.add registers the folder", "result" in res, json.dumps(res)[:200])
    wp = ((res.get("result") or {}).get("project") or {}).get("workspace_path")
    L.check("the row carries the workspace_path discovery reads",
            wp is not None and Path(wp).resolve() == PROJECT.resolve(), f"workspace_path={wp!r}")


async def preconditions(conn):
    """Discovered, loaded, and its server is up — before any claim about runs."""
    rows = ((await conn.call("plugins.list", {})).get("result") or {}).get("plugins", [])
    row = next((r for r in rows if r.get("name") == PLUGIN), None)
    L.log("plugins.list row ->", {k: row.get(k) for k in ("status", "kind")} if row else None)
    L.check("precondition: discovery walked the registered project (plugins.list has qa-vis loaded)",
            row is not None and row.get("status") == "loaded", str(row and row.get("status")))
    # The MCP half mounts asynchronously: the tool bridge registers the tool
    # when the handshake completes. A run sent before that would miss the tool
    # for a reason unrelated to visibility.
    for _ in range(60):
        cat = names((await conn.call("tools.catalog", {})).get("result", {}), set())
        hits = sorted(n for n in cat if n.endswith(MCP_SUFFIX))
        if hits:
            break
        await asyncio.sleep(0.5)
    L.check("precondition: the plugin's MCP server finished its handshake (tools.catalog lists it)",
            bool(hits), f"catalog name(s)={hits}")


async def probe(conn):
    await preconditions(conn)

    L.log("\n--- run 1: bound to the project ---")
    outcome = await run(conn, IN_MARK, "agent:main:qa-vis-in", PROJECT)
    L.check("project-bound run reached a terminal frame", outcome == "stream.run_complete", outcome)
    inside = requests_of(IN_MARK)
    main_in = [b for b in inside if tool_names(b)]
    L.check("project-bound run: a main-turn request (with a tool surface) was recorded",
            bool(main_in), f"{len(inside)} request(s), {len(main_in)} with tools")
    L.check("project-bound run: the model saw the plugin skill",
            any(SKILL in system_text(b) for b in inside), f"{len(inside)} request(s)")
    L.check("project-bound run: the model saw the plugin agent",
            any(AGENT in system_text(b) for b in inside), f"{len(inside)} request(s)")
    joined = sorted({n for b in inside for n in tool_names(b) if n.endswith(MCP_SUFFIX)})
    L.check("project-bound run: the model saw the plugin MCP tool",
            MCP_TOOL in joined, f"expected {MCP_TOOL!r}; tools[] carried {joined}")

    L.log("\n--- run 2: no project (daemon CWD has no .aleph/plugins) ---")
    outcome = await run(conn, OUT_MARK, "agent:main:qa-vis-out", None)
    L.check("project-less run reached a terminal frame", outcome == "stream.run_complete", outcome)
    outside = requests_of(OUT_MARK)
    main_out = [b for b in outside if tool_names(b)]
    # Controls: the negatives below are only claims about a prompt that WAS
    # built — a tool surface to be absent from, and a skill index and an agent
    # catalog that rendered (other skills and the builtin sub-agents fill
    # them) for the plugin's entries to be missing from.
    L.check("project-less run: a main-turn request (with a tool surface) was recorded",
            bool(main_out), f"{len(outside)} request(s), {len(main_out)} with tools")
    L.check("project-less run: the skill index rendered (`<available_skills>`)",
            any("<available_skills>" in system_text(b) for b in main_out))
    L.check("project-less run: the agent catalog rendered (`<available_agents>`)",
            any("<available_agents>" in system_text(b) for b in main_out))
    # The whole request body — system, tools[] and messages: the run's own
    # user text names none of these, so any hit is a plugin face leaking.
    seen = [json.dumps(b) for b in outside]
    L.check("project-less run: the plugin skill is NOT in the prompt",
            not any(SKILL in s for s in seen), f"{len(outside)} request(s)")
    L.check("project-less run: the plugin agent is NOT in the prompt",
            not any(AGENT in s for s in seen), f"{len(outside)} request(s)")
    leaked = sorted({n for b in outside for n in tool_names(b) if n.endswith(MCP_SUFFIX)})
    L.check("project-less run: the plugin MCP tool is NOT in the prompt",
            not leaked and not any(MCP_SUFFIX in s for s in seen), f"tools[] carried {leaked}")


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
