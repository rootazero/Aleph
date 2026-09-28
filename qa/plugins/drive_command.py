#!/usr/bin/env python3
"""`/cmd args` → the rendered body reaches the model; the raw text stays persisted.

  python3 drive_command.py <ws-url> <request_log>

Oracle for "reached the model": the mock provider's request log — each line is
one request the server sent, `body.messages` verbatim. The rendered `<command>`
block rides the transient channel (`slash_command_body` → `transient_blocks`),
which is delivered every Think and never written to the session log, so it
shows up in the request log and NOT in `chat.history` — both halves asserted.

The caller is an OPERATOR: a loopback WebSocket connection with no credential
(`role_is_operator`). That decides which placeholder an un-approved
`` !`cmd` `` gets (`slash_command_body::ConsentedShell::run` vs the
`inline_shell_refusal` a channel guest gets), so the placeholder asserted
below is the operator one, re-derived from HEAD, and a turn that ran as a
guest reads as a FAIL naming the other text.

The inline command's SOURCE differs from its OUTPUT (`printf 'QA_%s_RAN'
INLINE` prints `QA_INLINE_RAN`): the placeholder quotes the source, so a
source-string check could not tell "withheld" from "ran".
"""
import asyncio
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, ws_connect  # noqa: E402

WS, REQ_LOG = sys.argv[1], Path(sys.argv[2])
L = Ledger()
PLUGIN = "qa-cmd-plugin"
SLASH = f"{PLUGIN}:greet"
SENT = f"/{SLASH} World"
MARKER = "QA_CMD_MARKER_World"
INLINE_OUTPUT = "QA_INLINE_RAN"
# `ConsentedShell::run`, the arm for an operator turn whose command has no
# approval yet. The guest arm's text is named so a wrong role is legible.
OPERATOR_PLACEHOLDER = "not run: pending operator approval"
GUEST_PLACEHOLDER = "not run: inline commands run only for an operator"


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

    async def terminal(self, run_id, seconds=120):
        """`stream.run_complete` / `stream.run_error` for `run_id`, or a miss."""
        def match(msg):
            return msg.get("method") in ("stream.run_complete", "stream.run_error") \
                and (msg.get("params") or {}).get("run_id") == run_id
        for msg in self.frames:
            if match(msg):
                return msg["method"], msg.get("params")
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                msg = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=1.0))
            except asyncio.TimeoutError:
                continue
            if match(msg):
                return msg["method"], msg.get("params")
        return "no_terminal_frame", None


def rows_naming(node, needle, out):
    """Every object in an RPC result that has a string field containing
    `needle` — the row a listing keeps for one command, whatever its envelope."""
    if isinstance(node, dict):
        if any(isinstance(v, str) and needle in v for v in node.values()):
            out.append(node)
        for v in node.values():
            rows_naming(v, needle, out)
    elif isinstance(node, list):
        for v in node:
            rows_naming(v, needle, out)
    return out


def one_line(text):
    return text.replace("\n", " \u23ce ")


def user_texts():
    """Every user-role text the mock was handed, across every request."""
    if not REQ_LOG.exists():
        return []
    out = []
    for line in REQ_LOG.read_text().splitlines():
        if not line.strip():
            continue
        for m in json.loads(line)["body"].get("messages", []):
            if m.get("role") != "user":
                continue
            c = m.get("content")
            if isinstance(c, str):
                out.append(c)
            elif isinstance(c, list):
                out.extend(b.get("text", "") for b in c if isinstance(b, dict))
    return out


async def main():
    async with ws_connect(WS) as ws:
        conn = Conn(ws)
        await conn.call("connect", {"client_info": {"name": "qa-command"}})

        L.log("\n--- the listing ---")
        listing = await conn.call("commands.list", {})
        cmds = json.dumps(listing)
        L.check("the plugin command is listed", SLASH in cmds, f"payload {len(cmds)}B")
        # Scoped to the plugin's own row: another command whose hint is also
        # `[name]` must not satisfy this.
        own = rows_naming(listing, SLASH, [])
        L.check("argument-hint reaches the plugin's own row (usage string)",
                any("[name]" in json.dumps(r) for r in own), f"{len(own)} row(s) name {SLASH}")

        # The listing is gated on the plugin being enabled: a stage whose
        # positive would also hold for a plugin nobody enabled proves nothing.
        L.log("\n--- disabled, the command is gone; enabled again, it is back ---")
        off = await conn.call("plugins.disable", {"name": PLUGIN})
        L.check("plugins.disable accepted", (off.get("result") or {}).get("ok") is True,
                json.dumps(off.get("error") or off.get("result"))[:200])
        L.check("a disabled plugin's command is NOT listed",
                SLASH not in json.dumps(await conn.call("commands.list", {})))
        on = await conn.call("plugins.enable", {"name": PLUGIN})
        L.check("plugins.enable accepted", (on.get("result") or {}).get("ok") is True,
                json.dumps(on.get("error") or on.get("result"))[:200])
        L.check("re-enabled, the command is listed again",
                SLASH in json.dumps(await conn.call("commands.list", {})))

        L.log("\n--- /cmd args ---")
        sent = await conn.call("chat.send", {"message": SENT, "session_key": "agent:main:qa-command"})
        L.check("chat.send accepted the /command", "result" in sent, json.dumps(sent.get("error", ""))[:200])
        result = sent.get("result") or {}
        outcome, params = await conn.terminal(result.get("run_id"))
        L.check("the command turn reached run_complete", outcome == "stream.run_complete",
                f"{outcome} {json.dumps(params)[:200] if params else ''}")

        texts = user_texts()
        hit = next((t for t in texts if MARKER in t), None)
        L.check("the rendered body reached the model ($1 substituted)", hit is not None,
                f"{len(texts)} user text(s) seen")
        if hit:
            L.check("wrapped as a <command> block naming the plugin and the invocation",
                    f'<command name="greet" plugin="qa-cmd-plugin" invoked="/{SLASH}">' in hit, one_line(hit[:120]))
            L.check("${2:-nobody} default applied", "Second: nobody." in hit)
            L.check("the un-approved !`cmd` did not run (its output is absent)", INLINE_OUTPUT not in hit)
            L.check("it is withheld with the OPERATOR placeholder (caller role: operator, loopback)",
                    OPERATOR_PLACEHOLDER in hit,
                    "guest placeholder instead — the turn ran as a non-operator" if GUEST_PLACEHOLDER in hit
                    else one_line(hit[hit.find("Inline:"):][:240]))
            # O-A (P4.13a): the review verbs exist only on `aleph-server hooks`;
            # `aleph hooks` is the hooks.json editor and has no `test`.
            L.check("the placeholder names the verbs that exist (`aleph-server hooks …`, never bare `aleph hooks`)",
                    "`aleph-server hooks list`" in hit and "`aleph hooks" not in hit,
                    one_line(hit[hit.find("not run:"):][:200]))

        L.log("\n--- what was persisted ---")
        hist = await conn.call("chat.history", {"session_key": result.get("session_key"), "limit": 10})
        text = json.dumps(hist.get("result") or {})
        L.check("chat.history answered", "result" in hist, json.dumps(hist.get("error", ""))[:200])
        L.check("the persisted user turn is the raw /cmd text", SENT in text, text[:200])
        L.check("the rendered body is NOT persisted", MARKER not in text)

    return L.verdict()


sys.exit(asyncio.run(main()))
