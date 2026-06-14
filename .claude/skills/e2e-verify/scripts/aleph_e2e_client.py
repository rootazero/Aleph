#!/usr/bin/env python3
"""
Reusable Aleph E2E client — WebSocket JSON-RPC with streaming + SQLite verification.

Patterns borrowed from tests/e2e/scripts/ws_test.py:
- _drain() after connect to clear stale push events
- deadline-based timeouts (not per-recv)
- run_id filtering for concurrent runs
- SQLite direct query for ground-truth state verification
- Global pass/fail counters with exit code for CI

Usage as library:
    from aleph_e2e_client import AlephClient, check, section, db_query, get_shared_token

Usage as standalone (connection + smoke test):
    python3 aleph_e2e_client.py [port]
"""

import asyncio
import json
import os
import sqlite3
import subprocess
import sys
from pathlib import Path

import websockets

# ── Paths ──
ALEPH_DATA = Path.home() / ".aleph" / "data"
SECURITY_DB = ALEPH_DATA / "security.db"
TEAMS_DB = ALEPH_DATA / "teams.db"
COORD_DB = ALEPH_DATA / "coord.db"

# ── Output formatting ──
GREEN, RED, YELLOW, CYAN, RESET, BOLD = (
    "\033[92m", "\033[91m", "\033[93m", "\033[96m", "\033[0m", "\033[1m"
)

_passed = 0
_failed = 0


def check(name, condition, detail=""):
    """Assert a condition. Returns bool. Tracks global pass/fail counts."""
    global _passed, _failed
    if condition:
        print(f"  {GREEN}✓{RESET} {name}")
        _passed += 1
    else:
        print(f"  {RED}✗{RESET} {name}" + (f" — {YELLOW}{detail}{RESET}" if detail else ""))
        _failed += 1
    return condition


def section(name):
    print(f"\n{BOLD}{CYAN}═══ {name} ═══{RESET}")


def summary():
    """Print summary and return exit-code-friendly bool."""
    total = _passed + _failed
    color = GREEN if _failed == 0 else RED
    print(f"\n{BOLD}{color}{'═' * 50}")
    print(f"RESULTS: {_passed} passed, {_failed} failed (of {total})")
    print(f"{'═' * 50}{RESET}")
    return _failed == 0


# ── SQLite ground-truth queries ──

def db_query(db_path, sql, params=()):
    """Query SQLite DB, return list of dicts."""
    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    rows = conn.execute(sql, params).fetchall()
    conn.close()
    return [dict(r) for r in rows]


def db_query_teams(sql, params=()):
    """Query teams.db (messages, artifacts, events, sessions)."""
    return db_query(TEAMS_DB, sql, params)


def db_query_coord(sql, params=()):
    """Query coord.db (coord_tasks, coord_task_dependencies)."""
    return db_query(COORD_DB, sql, params)


# ── Token ──

def get_shared_token():
    """Read shared token from Aleph security DB."""
    result = subprocess.run(
        ["sqlite3", str(SECURITY_DB), "SELECT plaintext_token FROM shared_token LIMIT 1;"],
        capture_output=True, text=True
    )
    token = result.stdout.strip()
    if not token:
        print(f"{RED}Cannot read shared token from {SECURITY_DB}{RESET}")
        sys.exit(1)
    return token


# ── WebSocket Client ──

class AlephClient:
    """Persistent WebSocket JSON-RPC client with streaming event collection.

    Three interaction modes:
    - rpc():   direct request/response (skip push events)
    - slash(): fast-path commands (/team create, /task list) — no LLM, <5ms
    - chat():  LLM chat — collects tool_calls + text, returns when run completes
    """

    def __init__(self, port=18790):
        self.ws_url = f"ws://127.0.0.1:{port}/ws"
        self.ws = None
        self.msg_id = 0

    async def connect(self, shared_token):
        """Connect, authenticate, drain stale events."""
        self.ws = await websockets.connect(
            self.ws_url, max_size=10 * 1024 * 1024, close_timeout=5
        )
        self.msg_id += 1
        await self.ws.send(json.dumps({
            "jsonrpc": "2.0", "method": "connect",
            "params": {"shared_token": shared_token, "device_name": "E2E Verify"},
            "id": self.msg_id
        }))
        resp = json.loads(await self.ws.recv())
        assert "result" in resp, f"Connect failed: {resp}"
        await self._drain()
        return resp["result"]

    async def _drain(self, timeout=0.5):
        """Consume all pending push events after connect."""
        try:
            while True:
                await asyncio.wait_for(self.ws.recv(), timeout=timeout)
        except Exception:
            pass

    async def rpc(self, method, params=None, timeout=10):
        """Direct JSON-RPC call. Skips push events. Returns result or {"_error": ...}."""
        self.msg_id += 1
        rid = self.msg_id
        await self.ws.send(json.dumps({
            "jsonrpc": "2.0", "method": method, "params": params or {}, "id": rid
        }))
        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            remaining = deadline - asyncio.get_event_loop().time()
            if remaining <= 0:
                return {"_error": "timeout"}
            raw = await asyncio.wait_for(self.ws.recv(), timeout=remaining)
            data = json.loads(raw)
            if data.get("id") == rid:
                if "error" in data:
                    return {"_error": data["error"]}
                return data.get("result", {})

    async def slash(self, cmd, timeout=20):
        """Send slash command via chat.send, wait for run_complete.

        Fast-path commands execute tools directly (no LLM call).
        Returns the assembled response text.
        """
        self.msg_id += 1
        rid = self.msg_id
        await self.ws.send(json.dumps({
            "jsonrpc": "2.0", "method": "chat.send",
            "params": {"message": cmd, "channel": "gui:chat", "stream": True},
            "id": rid
        }))

        run_id = None
        text_parts = []
        final_response = ""
        completed = False
        deadline = asyncio.get_event_loop().time() + timeout

        while not completed:
            remaining = deadline - asyncio.get_event_loop().time()
            if remaining <= 0:
                break
            try:
                raw = await asyncio.wait_for(self.ws.recv(), timeout=min(remaining, 5.0))
                d = json.loads(raw)
                m = d.get("method", "")
                p = d.get("params", {})

                if d.get("id") == rid and "result" in d:
                    run_id = d["result"].get("run_id", "") or run_id
                    if completed:
                        break
                    continue

                evt_run = p.get("run_id", "")
                if run_id and evt_run and evt_run != run_id:
                    continue

                if m == "stream.response_chunk":
                    text_parts.append(p.get("content", "") or p.get("delta", ""))
                elif m == "stream.run_complete":
                    s = p.get("summary", {})
                    final_response = s.get("final_response", "") if isinstance(s, dict) else ""
                    completed = True
                    if run_id:
                        break
                elif m in ("stream.error", "stream.run_failed"):
                    text_parts.append(f"ERROR: {p}")
                    completed = True
                    break
            except asyncio.TimeoutError:
                break

        return "".join(text_parts) or final_response

    async def chat(self, message, agent_id="main", timeout=120):
        """Send LLM chat, collect streaming tool calls + text.

        Returns: {"text": str, "tool_calls": [...], "completed": bool}
        """
        resp = await self.rpc("chat.send", {
            "message": message, "agent_id": agent_id, "stream": True
        })
        if "_error" in resp:
            return {"text": "", "tool_calls": [], "completed": False, "_error": resp["_error"]}

        run_id = resp.get("run_id", "")
        full_text, tool_calls = "", []
        completed = False
        deadline = asyncio.get_event_loop().time() + timeout

        while not completed:
            remaining = deadline - asyncio.get_event_loop().time()
            if remaining <= 0:
                break
            try:
                raw = await asyncio.wait_for(self.ws.recv(), timeout=min(remaining, 5.0))
                data = json.loads(raw)
                m = data.get("method", "")
                p = data.get("params", {}) if isinstance(data.get("params"), dict) else {}

                evt_run = p.get("run_id", "")
                if run_id and evt_run and evt_run != run_id:
                    continue

                if m == "stream.response_chunk":
                    full_text += p.get("delta", "") or p.get("content", "")
                elif m == "stream.tool_start":
                    tool_calls.append({
                        "name": p.get("tool_name", "") or p.get("tool_id", ""),
                        "input": p.get("params", {}),
                    })
                elif m == "stream.tool_end":
                    if tool_calls:
                        tool_calls[-1]["result"] = p.get("result", "")
                        tool_calls[-1]["error"] = p.get("error")
                        tool_calls[-1]["duration_ms"] = p.get("duration_ms", 0)
                elif m == "stream.run_complete":
                    completed = True
                    break
                elif m in ("stream.run_failed", "stream.error"):
                    return {"text": full_text, "tool_calls": tool_calls,
                            "completed": False, "_error": p.get("error", str(p))}
            except asyncio.TimeoutError:
                break

        return {"text": full_text, "tool_calls": tool_calls, "completed": completed}

    async def close(self):
        if self.ws:
            await self.ws.close()


# ── Standalone smoke test ──

async def _selftest():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18790
    token = get_shared_token()
    client = AlephClient(port)

    section("Connection")
    try:
        resp = await client.connect(token)
        check("WebSocket connected", "token" in resp or "device_id" in resp)

        section("RPC")
        tools = await client.rpc("agents.tools_schema")
        groups = tools.get("groups", [])
        check("Tool schema loaded", len(groups) > 0, f"{len(groups)} groups")
        team_cat = next((g for g in groups if g["id"] == "team"), None)
        if team_cat:
            tnames = [t["name"] for t in team_cat.get("tools", [])]
            check("Team tools registered", len(tnames) >= 8, f"{len(tnames)}: {tnames}")

        section("Slash Command")
        text = await client.slash("/task list")
        check("Slash /task list", len(text) > 0, text[:100])

        section("SQLite")
        try:
            rows = db_query_teams("SELECT COUNT(*) as n FROM team_messages")
            check("teams.db readable", True, f"{rows[0]['n']} messages")
        except Exception as e:
            check("teams.db readable", False, str(e))

        summary()
    except Exception as e:
        print(f"\n{RED}Error: {e}{RESET}")
        import traceback; traceback.print_exc()
    finally:
        await client.close()

    sys.exit(0 if _failed == 0 else 1)


if __name__ == "__main__":
    asyncio.run(_selftest())
