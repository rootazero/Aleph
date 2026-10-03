#!/usr/bin/env python3
"""Real-machine driver for the MCP server face (spec §3.7, QA §6).

    drive.py <stage> <gateway_port> <expose-csv> [lan_ip]

Every assertion reads a WIRE effect — a status code, a header, a JSON-RPC body,
an SSE frame — never "the call returned". `Ledger` prints PASS/FAIL as it goes
so a run that dies half-way still shows what was settled.

Only `list_changed` and `auth` open a WebSocket, and only for the RPCs that
have no HTTP face (plugin toggling, fetching the shared token). `deny` opens
none on purpose: a loopback WS client is an operator surface, and the claim is
about what happens when there is none.
"""
import asyncio
import json
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger  # noqa: E402

import websockets  # noqa: E402

STAGE = sys.argv[1]
PORT = int(sys.argv[2])
EXPOSE = [n for n in sys.argv[3].split(",") if n]
LAN_IP = sys.argv[4] if len(sys.argv) > 4 else ""
LOCAL = f"http://127.0.0.1:{PORT}"
WS = f"ws://127.0.0.1:{PORT}/ws"
SUPPORTED = ["2025-11-25", "2025-06-18", "2025-03-26"]
L = Ledger()


class Mcp:
    """One MCP client over Streamable HTTP. Holds the session id once
    `initialize` mints it and sends it on every later request."""

    def __init__(self, base, bearer=None):
        self.base, self.bearer, self.session, self.n = base, bearer, None, 0

    def _headers(self, extra=None):
        h = {"content-type": "application/json", "accept": "application/json, text/event-stream"}
        if self.bearer:
            h["authorization"] = f"Bearer {self.bearer}"
        if self.session:
            h["mcp-session-id"] = self.session
        h.update(extra or {})
        return h

    def request(self, method, body=None, headers=None, timeout=45):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(self.base + "/mcp", data=data, method=method, headers=self._headers(headers))
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                raw = r.read()
                return r.status, {k.lower(): v for k, v in r.headers.items()}, (json.loads(raw) if raw else None)
        except urllib.error.HTTPError as e:
            raw = e.read()
            try:
                parsed = json.loads(raw) if raw else None
            except ValueError:
                parsed = raw.decode(errors="replace")
            return e.code, {k.lower(): v for k, v in e.headers.items()}, parsed

    def call(self, method, params=None, id=None):
        self.n += 1
        msg = {"jsonrpc": "2.0", "id": self.n if id is None else id, "method": method}
        if params is not None:
            msg["params"] = params
        return self.request("POST", msg)

    def notify(self, method, params=None):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        return self.request("POST", msg)

    def initialize(self, version="2025-03-26", client="qa-mcp-face"):
        st, hd, body = self.call("initialize", {
            "protocolVersion": version, "capabilities": {},
            "clientInfo": {"name": client, "version": "0"}})
        sid = hd.get("mcp-session-id")
        if st == 200 and sid:
            self.session = sid
        return st, sid, body

    def tools(self):
        st, _, body = self.call("tools/list", {})
        return st, body, {t["name"]: t for t in (body or {}).get("result", {}).get("tools", [])}

    def sse(self):
        """Open GET /mcp on a thread; returns (frames, close). `frames` fills
        with every `data:` line's JSON as it arrives."""
        frames, stop = [], threading.Event()
        req = urllib.request.Request(self.base + "/mcp", method="GET", headers=self._headers({"accept": "text/event-stream"}))
        resp = urllib.request.urlopen(req, timeout=120)
        status = resp.status

        def pump():
            try:
                while not stop.is_set():
                    line = resp.readline()
                    if not line:
                        break
                    line = line.decode(errors="replace").rstrip("\r\n")
                    if line.startswith("data:"):
                        try:
                            frames.append(json.loads(line[5:].strip()))
                        except ValueError:
                            frames.append({"raw": line})
            except Exception as e:  # noqa: BLE001 — the pump's job is to keep reading until closed
                frames.append({"pump_error": str(e)})

        t = threading.Thread(target=pump, daemon=True)
        t.start()

        def close():
            stop.set()
            try:
                resp.close()
            except Exception:  # noqa: BLE001
                pass

        return status, frames, close


def wait_for(frames, method, secs=10.0):
    deadline = time.time() + secs
    while time.time() < deadline:
        if any(f.get("method") == method for f in frames):
            return True
        time.sleep(0.2)
    return False


async def ws_rpc(method, params):
    async with websockets.connect(WS, max_size=None, ping_interval=None) as ws:
        n = 0

        async def call(m, p):
            nonlocal n
            n += 1
            await ws.send(json.dumps({"jsonrpc": "2.0", "id": n, "method": m, "params": p}))
            while True:
                msg = json.loads(await ws.recv())
                if msg.get("id") == n:
                    return msg

        await call("connect", {"client": "qa-mcp-face", "version": "1"})
        return await call(method, params)


def rpc(method, params):
    return asyncio.run(ws_rpc(method, params))


def text_of(body):
    try:
        return body["result"]["content"][0]["text"]
    except (KeyError, IndexError, TypeError):
        return json.dumps(body)[:400]


# ── stages ─────────────────────────────────────────────────────────────────

def stage_handshake():
    for v in SUPPORTED:
        c = Mcp(LOCAL)
        st, sid, body = c.initialize(v)
        r = (body or {}).get("result", {})
        L.check(f"initialize {v}: 200 with a session id", st == 200 and bool(sid), f"status={st} sid={sid}")
        L.check(f"initialize {v}: protocolVersion echoed", r.get("protocolVersion") == v, json.dumps(r)[:200])
        L.check(f"initialize {v}: capabilities is exactly tools.listChanged",
                r.get("capabilities") == {"tools": {"listChanged": True}}, json.dumps(r.get("capabilities")))
        L.check(f"initialize {v}: serverInfo.name == aleph", r.get("serverInfo", {}).get("name") == "aleph")
        st, _, _ = c.notify("notifications/initialized")
        L.check(f"{v}: notifications/initialized is 202", st == 202, f"status={st}")
        st, _, body = c.call("ping", {}, id="ping-1")
        L.check(f"{v}: ping answers {{}} and echoes the string id",
                st == 200 and body.get("result") == {} and body.get("id") == "ping-1", json.dumps(body)[:200])
        st, _, _ = c.request("DELETE")
        L.check(f"{v}: DELETE ends the session", st == 200, f"status={st}")

    c = Mcp(LOCAL)
    st, sid, body = c.initialize("1999-01-01")
    L.check("an unsupported version is answered with 2025-11-25",
            st == 200 and body["result"]["protocolVersion"] == "2025-11-25", json.dumps(body)[:200])
    st, _, body = Mcp(LOCAL).call("tools/list", {})
    L.check("a request without a session is 400", st == 400, f"status={st}")
    c2 = Mcp(LOCAL)
    c2.session = "00000000-0000-0000-0000-000000000000"
    st, _, _ = c2.call("tools/list", {})
    L.check("an unknown session is 404", st == 404, f"status={st}")


def stage_tools():
    c = Mcp(LOCAL)
    st, _, _ = c.initialize("2025-06-18")
    L.check("initialize", st == 200)
    st, body, tools = c.tools()
    L.check("tools/list is 200", st == 200, f"status={st}")
    strangers = sorted(set(tools) - set(EXPOSE))
    L.check("tools/list ⊆ expose", not strangers, f"not in expose: {strangers}")
    for name in ("grep", "file_read", "agent_list"):
        L.check(f"{name} is listed with an object inputSchema",
                tools.get(name, {}).get("inputSchema", {}).get("type") == "object",
                json.dumps(tools.get(name))[:200])
    L.check("bash is NOT listed", "bash" not in tools)
    st, _, body = c.call("tools/call", {"name": "agent_list", "arguments": {}})
    r = (body or {}).get("result", {})
    L.check("agent_list returns a text content block, isError false",
            st == 200 and r.get("isError") is False and r.get("content", [{}])[0].get("type") == "text",
            json.dumps(body)[:300])
    L.check("the text names the main agent", "main" in text_of(body), text_of(body)[:200])


def stage_auth():
    if not LAN_IP:
        print("SKIP  auth: no non-loopback address on this host (every remote assertion is UNRUN, not PASS)")
        return
    remote = f"http://{LAN_IP}:{PORT}"
    # Pre-flight: a host with a non-loopback address can still be unable to
    # reach it from itself (macOS application firewall refuses inbound
    # connections to the unsigned debug binary; dual-NIC hosts route the
    # self-connection out a dead interface, so the TCP handshake succeeds but
    # the HTTP request hangs). In that case every remote assertion is UNRUN,
    # not PASS — SKIP rather than fail.
    try:
        with urllib.request.urlopen(f"{remote}/health", timeout=5) as r:
            r.read()
    except urllib.error.HTTPError:
        pass  # the server answered (reachable); the real assertions follow
    except (urllib.error.URLError, TimeoutError, OSError) as e:
        print(f"SKIP  auth: cannot reach {remote} ({e.__class__.__name__}); "
              "every remote assertion is UNRUN, not PASS")
        return
    st, hd, _ = Mcp(remote).initialize()
    L.check("remote without a bearer is 401", st == 401, f"status={st}")
    L.check("…with WWW-Authenticate: Bearer", hd.get("www-authenticate", "").startswith("Bearer"), str(hd.get("www-authenticate")))
    st, _, _ = Mcp(remote, bearer="aleph-not-a-token").initialize()
    L.check("remote with a wrong bearer is 401", st == 401, f"status={st}")

    cur = rpc("gateway.token.current", {})
    token = (cur.get("result") or {}).get("token")
    if not token:
        token = rpc("gateway.token.rotate", {})["result"]["token"]
    L.check("a shared gateway token exists", bool(token))
    c = Mcp(remote, bearer=token)
    st, sid, body = c.initialize()
    L.check("remote with the shared token is admitted with a session", st == 200 and bool(sid), f"status={st} body={json.dumps(body)[:200]}")
    st, _, tools = c.tools()
    L.check("…and can list tools", st == 200 and bool(tools), f"status={st} n={len(tools)}")
    st, sid, _ = Mcp(LOCAL).initialize()
    L.check("loopback still needs nothing", st == 200 and bool(sid), f"status={st}")


def stage_list_changed():
    c = Mcp(LOCAL)
    st, _, _ = c.initialize("2025-11-25")
    L.check("initialize", st == 200)
    st, _, tools = c.tools()
    L.check("the planted plugin tool is listed while enabled", "qa_mcp_probe" in tools, sorted(tools)[:20])
    sse_status, frames, close = c.sse()
    L.check("GET /mcp opens an event stream", sse_status == 200, f"status={sse_status}")
    try:
        off = rpc("tools.invoke", {"tool_name": "plugin_manage", "arguments": {"action": "disable", "name": "qa-mcp-probe"}})
        L.check("plugin_manage disable reported ok", "error" not in off, json.dumps(off)[:300])
        L.check("an SSE notifications/tools/list_changed frame arrives within 10 s",
                wait_for(frames, "notifications/tools/list_changed"), json.dumps(frames)[:300])
        st, _, tools = c.tools()
        L.check("the tool left tools/list", "qa_mcp_probe" not in tools, sorted(tools)[:20])
        frames.clear()
        on = rpc("tools.invoke", {"tool_name": "plugin_manage", "arguments": {"action": "enable", "name": "qa-mcp-probe"}})
        L.check("plugin_manage enable reported ok", "error" not in on, json.dumps(on)[:300])
        L.check("a second list_changed frame arrives", wait_for(frames, "notifications/tools/list_changed"), json.dumps(frames)[:300])
        st, _, tools = c.tools()
        L.check("the tool is back in tools/list", "qa_mcp_probe" in tools, sorted(tools)[:20])
    finally:
        close()


def stage_deny():
    c = Mcp(LOCAL)
    st, _, _ = c.initialize()
    L.check("initialize", st == 200)
    st, _, body = c.call("tools/call", {"name": "bash", "arguments": {"command": "id"}})
    err = (body or {}).get("error") or {}
    L.check("an unexposed tool is JSON-RPC -32602, not executed", st == 200 and err.get("code") == -32602, json.dumps(body)[:300])
    st, _, tools = c.tools()
    L.check("precondition: agent_delete is exposed AND registered on this host", "agent_delete" in tools, sorted(tools)[:30])
    t0 = time.time()
    st, _, body = c.call("tools/call", {"name": "agent_delete", "arguments": {"agent_id": "qa-no-such-agent"}})
    took = time.time() - t0
    r = (body or {}).get("result") or {}
    L.check("a confirmation-gated tool with no operator surface is isError:true", st == 200 and r.get("isError") is True, json.dumps(body)[:400])
    L.check("…and answered at once, not after a parked card", took < 5, f"took {took:.1f}s")
    L.check("…the reason says nobody was asked", "(Unavailable)" in text_of(body), text_of(body)[:300])
    L.check("…and names the Aleph Panel as where to grant it", "Aleph Panel" in text_of(body), text_of(body)[:300])


STAGES = {
    "handshake": stage_handshake,
    "tools": stage_tools,
    "auth": stage_auth,
    "list_changed": stage_list_changed,
    "deny": stage_deny,
}
STAGES[STAGE]()
sys.exit(len(L.failures))
