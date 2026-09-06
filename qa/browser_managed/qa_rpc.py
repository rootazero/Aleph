#!/usr/bin/env python3
"""Shared plumbing for the browser QA drivers: JSON-RPC, `tools.invoke`, the
out-of-band `playwright-cli` oracle, and the pass/fail ledger.

Extracted so `drive_browser.py` (launch/config claims) and `drive_tools.py`
(per-verb claims) cannot drift apart on *how* they talk to the gateway — the
two files disagree about what to assert, never about the wire.
"""
import asyncio
import json
import os
import subprocess
import urllib.request

import websockets

RPC_TIMEOUT_SECS = 180


def ws_connect(url):
    """`websockets.connect`, with the low-level ping/pong keepalive disabled.

    A real `browser_open` under the launch chain flip spawns Chromium and
    polls for its port file — real wall-clock seconds, not a mocked call. On
    a machine also running concurrent `cargo`/`clippy` builds (this plan's
    reviewers run them alongside an implementer), CPU contention can starve
    this process's asyncio loop long enough that the `websockets` library's
    own keepalive (ping every 20s, 20s to get a pong — a fixed schedule,
    unrelated to `RPC_TIMEOUT_SECS`) fires a false "keepalive ping timeout"
    disconnect that has nothing to do with the code under test: observed
    3-for-3 on this machine, always mid-`browser_open`, while `top`/`ps`
    showed two other clippy-driver processes pinned near 100% CPU. The
    gateway's own app-level keepalive (`idle_timeout_secs` in the `connect`
    response) and this module's `RPC_TIMEOUT_SECS` are the timeouts that
    should decide whether a call is actually stuck.
    """
    return websockets.connect(url, max_size=None, ping_interval=None)


class Ledger:
    """Pass/fail accumulator. `check` prints as it goes so a run that dies
    half-way still shows which claims had already been settled."""

    def __init__(self):
        self.failures = []

    @staticmethod
    def log(*a):
        print(*a, flush=True)

    def check(self, claim, ok, detail=""):
        print(f"  [{'PASS' if ok else 'FAIL'}] {claim}" + (f" — {detail}" if detail else ""), flush=True)
        if not ok:
            self.failures.append(f"{claim} ({detail})" if detail else claim)
        return bool(ok)

    def verdict(self):
        print("", flush=True)
        if self.failures:
            print(f"VERDICT: FAIL ({len(self.failures)} claim(s))", flush=True)
            for f in self.failures:
                print(f"  - {f}", flush=True)
            return 1
        print("VERDICT: PASS", flush=True)
        return 0


class Rpc:
    """One websocket, monotonic ids."""

    def __init__(self, ws):
        self.ws = ws
        self._id = 100

    async def call(self, method, params):
        self._id += 1
        rid = self._id
        await self.ws.send(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}))
        while True:
            msg = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=RPC_TIMEOUT_SECS))
            if msg.get("id") == rid:
                return msg

    async def invoke(self, tool, arguments):
        """One `tools.invoke`. Returns `(ok, result_or_error_object)`.

        `ok` is the tool's own success flag as the gateway reports it; a tool
        that degrades (returns `success: false` rather than erroring) still
        arrives here as a body, which is why callers assert on the body and not
        merely on the absence of an RPC error.
        """
        msg = await self.call("tools.invoke", {"tool_name": tool, "arguments": arguments})
        if "error" in msg:
            return False, {"rpc_error": msg["error"]}
        res = msg["result"]
        return bool(res.get("ok")), res.get("result", res)

    async def connect(self, name):
        msg = await self.call("connect", {"client_info": {"name": name}})
        Ledger.log("connect ->", json.dumps(msg.get("result", msg))[:160])
        return msg


def cli_sessions(cli, home, extra_args=()):
    """`playwright-cli list`, read with the scenario's scratch HOME.

    Out-of-band oracle: it is the only surface that answers "is a browser
    actually up, and what did it launch with" without going through the code
    under test.

    HOME is overridden (the CLI's session store is HOME-scoped, and the
    developer's own sessions are not ours to read) but PATH is inherited:
    `playwright-cli` is a node script and a hand-made PATH without `node` turns
    the oracle into `env: node: No such file or directory` — which reads exactly
    like "no sessions" and would have passed the check.
    """
    out = subprocess.run(
        [cli, "list", *extra_args],
        capture_output=True,
        text=True,
        timeout=60,
        env={**os.environ, "HOME": home},
    )
    return out.stdout + out.stderr


def session_status(listing):
    """`{session_name: status}` from the `### Browsers` section of `list`.

    Parsed from the section, not by counting `status: open` across the whole
    output: `playwright-cli list` prints a SECOND section, "Browser servers
    available for attach", which repeats `status: open` for each live browser.
    A naive count therefore reported two open sessions for one — the reaper
    scenario read that as "nothing was closed" while the listing plainly showed
    `status: closed` two lines above.
    """
    out, name = {}, None
    in_browsers = False
    for line in listing.splitlines():
        stripped = line.strip()
        if stripped.startswith("### "):
            in_browsers = stripped == "### Browsers"
            continue
        if not in_browsers:
            continue
        # `- <name>:` opens an entry; `  - status: <s>` is one of its fields.
        if stripped.startswith("- ") and stripped.endswith(":"):
            name = stripped[2:-1].strip()
        elif name and stripped.startswith("- status:"):
            out[name] = stripped.split(":", 1)[1].strip()
    return out


def open_session_count(listing):
    """How many sessions the oracle reports as open."""
    return sum(1 for v in session_status(listing).values() if v == "open")


def read_devtools_port_file(port_file):
    """(port, browser_path) from Chrome's own `DevToolsActivePort`, or None.

    This is the launch oracle that replaced `playwright-cli list` echoing
    `user-data-dir:` — under `attach --cdp` the CLI does not own the profile
    directory, so it has nothing to echo. The port file is written by Chrome
    itself, into the user-data-dir ALEPH chose, which is what makes it prove
    the browser rather than the CLI's copy of our config.
    """
    try:
        with open(port_file) as fh:
            lines = fh.read().splitlines()
    except OSError:
        return None
    if len(lines) < 2 or not lines[1].startswith("/"):
        return None
    try:
        return int(lines[0]), lines[1]
    except ValueError:
        return None


def http_json(port, path):
    """GET `path` off the CDP endpoint on `port`, parsed as JSON. Raises on
    any failure — the caller's claim IS the absence of an exception."""
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=10) as r:
        return r.status, json.loads(r.read().decode())
