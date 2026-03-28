#!/usr/bin/env python3
"""
Reusable Aleph E2E client — WebSocket JSON-RPC with streaming event collection.

Usage as library:
    from aleph_e2e_client import AlephClient, print_test, print_section, get_shared_token

Usage as standalone (connection test):
    python3 scripts/aleph_e2e_client.py [port]
"""

import asyncio
import json
import os
import subprocess
import sys

import websockets

# === Output formatting ===
GREEN, RED, YELLOW, CYAN, RESET, BOLD = (
    "\033[92m", "\033[91m", "\033[93m", "\033[96m", "\033[0m", "\033[1m"
)

def print_test(name, passed, detail=""):
    icon = f"{GREEN}PASS{RESET}" if passed else f"{RED}FAIL{RESET}"
    print(f"  [{icon}] {name}")
    if detail and not passed:
        print(f"         {YELLOW}{detail}{RESET}")

def print_section(name):
    print(f"\n{BOLD}{CYAN}=== {name} ==={RESET}")

def get_shared_token():
    """Read shared token from Aleph security DB."""
    db = os.path.expanduser("~/.aleph/data/security.db")
    result = subprocess.run(
        ["sqlite3", db, "SELECT plaintext_token FROM shared_token LIMIT 1;"],
        capture_output=True, text=True
    )
    token = result.stdout.strip()
    if not token:
        print(f"{RED}Cannot read shared token from {db}{RESET}")
        sys.exit(1)
    return token


class AlephClient:
    """WebSocket JSON-RPC client with streaming event collection."""

    def __init__(self, port=18790):
        self.ws_url = f"ws://127.0.0.1:{port}/ws"
        self.ws = None
        self.msg_id = 0

    async def connect(self, shared_token):
        """Connect and authenticate. Returns connect response."""
        self.ws = await websockets.connect(self.ws_url, close_timeout=5)
        return await self.rpc("connect", {
            "shared_token": shared_token, "device_name": "E2E Verify"
        })

    async def rpc(self, method, params=None):
        """Direct JSON-RPC call. Returns result dict or {"_error": ...}."""
        self.msg_id += 1
        msg = {"jsonrpc": "2.0", "id": self.msg_id, "method": method}
        if params:
            msg["params"] = params
        await self.ws.send(json.dumps(msg))
        while True:
            raw = await asyncio.wait_for(self.ws.recv(), timeout=30)
            data = json.loads(raw)
            if "id" in data and data["id"] == self.msg_id:
                if "error" in data:
                    return {"_error": data["error"]}
                return data.get("result", {})

    async def chat(self, message, agent_id="main", timeout=120):
        """Send chat message, collect streaming tool calls + text response.

        Returns: {"text": str, "tool_calls": [{"name": str, "input": dict, "result": any}]}
        """
        resp = await self.rpc("chat.send", {"message": message, "agent_id": agent_id})
        if "_error" in resp:
            return resp

        run_id = resp.get("run_id", "")
        full_text, tool_calls = "", []
        start = asyncio.get_event_loop().time()

        while True:
            try:
                raw = await asyncio.wait_for(self.ws.recv(), timeout=timeout)
                data = json.loads(raw)
                method = data.get("method", "")
                p = data.get("params", {}) if isinstance(data.get("params"), dict) else {}

                if method == "stream.response_chunk":
                    full_text += p.get("delta", "")
                elif method == "stream.tool_start":
                    tool_calls.append({
                        "name": p.get("tool_name", "") or p.get("tool_id", ""),
                        "input": p.get("params", {}),
                    })
                elif method == "stream.tool_end":
                    if tool_calls:
                        tool_calls[-1]["result"] = p.get("result", "")
                elif method == "stream.run_complete":
                    break
                elif method == "stream.run_failed":
                    return {"_error": p.get("error", "run failed")}

                if asyncio.get_event_loop().time() - start > timeout:
                    break
            except asyncio.TimeoutError:
                break

        return {"text": full_text, "tool_calls": tool_calls}

    async def close(self):
        if self.ws:
            await self.ws.close()


# === Standalone: connection test ===
async def _selftest():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18790
    token = get_shared_token()
    client = AlephClient(port)
    try:
        resp = await client.connect(token)
        print_test("Connected", "token" in resp, str(resp)[:100])

        resp = await client.chat("Say 'hello' in one word.", timeout=30)
        print_test("Chat response", bool(resp.get("text")), resp.get("text", "")[:50])
    finally:
        await client.close()

if __name__ == "__main__":
    asyncio.run(_selftest())
