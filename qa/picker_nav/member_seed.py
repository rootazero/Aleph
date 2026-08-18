#!/usr/bin/env python3
"""Seed the member-role scenario for the providers screens (checklist 12-15).

Why this exists
---------------
Every item in this fixture until now ran as loopback operator, and loopback is
*always* operator — so the entire refusal half of these screens had never been
exercised on a real machine. That half is not decoration: `providers.` sits in
`ADMIN_PREFIXES` (`src/gateway/method_admin.rs`), so for a member **every**
call this screen makes is refused — `providers.list` on mount, and
`providers.test` / `providers.delete` / `providers.update` on every button.

The failure mode the checklist is hunting is the one CLAUDE.md calls
"「被拒」不许读作「没有」": a refused `providers.list` folded into an empty
list renders as "no providers configured", which is a confident lie — the
member is looking at a machine that has providers, and the screen invites them
to add one they are not allowed to add. The second, newer half is the write
path: this round added Test-connection and Delete buttons to the phone screen,
and both are fed through `admin_refusal::settings_write_error`. A member is the
only way to reach that arm at all.

Duplication note: `qa/canvas/member_seed.py` shares this file's `Rpc` class and
its find-or-create member step. Two copies is deliberate — the rule of three —
and the canvas seeder is a validated fixture that should not be refactored
without re-running it.

Prerequisites (the fixture boots loopback-only on purpose; see run.sh):
  * scratch config edited to `[gateway] host = "0.0.0.0"` +
    `[gateway.tls] enabled = true`, server restarted;
  * run THIS script against loopback, which is the operator it needs.

Usage:
    python3 qa/picker_nav/member_seed.py <port> [--tls]

`--tls` matches the TLS-enabled restart: loopback then speaks wss:// behind the
gateway's self-signed cert, which the stdlib refuses. CERT_NONE here is the
same QA-only trust decision the browser makes interactively at the TOFU prompt.
"""

import asyncio
import json
import ssl
import sys

try:
    import websockets
except ImportError:
    sys.exit("needs the `websockets` package (pip install websockets)")

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18797
TLS = "--tls" in sys.argv

# Find-or-create key. `users` is keyed on `user_id`; `display_name` carries no
# uniqueness constraint (correctly — nothing resolves a principal by name), so a
# seeder that is not idempotent leaves a second "QA Member" behind on every
# retry and the member then holds two credentials for the same screen.
MEMBER_NAME = "QA Provider Member"


class Rpc:
    def __init__(self, ws):
        self.ws = ws
        self._id = 100

    async def call(self, method, params):
        self._id += 1
        rid = self._id
        await self.ws.send(
            json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        )
        while True:
            msg = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=30))
            if msg.get("id") == rid:
                if "error" in msg:
                    sys.exit(f"{method} failed: {json.dumps(msg['error'])}")
                return msg.get("result", {})


async def main():
    scheme = "wss" if TLS else "ws"
    ssl_ctx = None
    if TLS:
        ssl_ctx = ssl.create_default_context()
        ssl_ctx.check_hostname = False
        ssl_ctx.verify_mode = ssl.CERT_NONE
    async with websockets.connect(
        f"{scheme}://127.0.0.1:{PORT}/ws", ssl=ssl_ctx, max_size=2**24
    ) as ws:
        rpc = Rpc(ws)
        connect = await rpc.call("connect", {"client_info": {"name": "providers-member-seed"}})
        if not connect.get("authorized"):
            sys.exit(f"loopback connect not authorized: {json.dumps(connect)[:200]}")

        users = (await rpc.call("users.list", {})).get("users") or []
        existing = next(
            (
                u
                for u in users
                if u.get("display_name") == MEMBER_NAME
                and u.get("role") == "member"
                and u.get("status") == "active"
            ),
            None,
        )
        reused = existing is not None
        if existing:
            user_id = existing["user_id"]
        else:
            created = await rpc.call(
                "users.create", {"display_name": MEMBER_NAME, "role": "member"}
            )
            user_id = created["user"]["user_id"]

        # The operator control group: what the member's screen SHOULD have
        # shown if the role were not the thing stopping it. Printing the count
        # here is what turns item 13 from "the list looks empty" into an
        # assertion — an empty list is only a lie if there was something to see.
        configured = (await rpc.call("providers.list", {})).get("providers") or []

        ticket = await rpc.call("gateway.ticket.create", {"user_id": user_id})

        print(json.dumps(
            {
                "member_user_id": user_id,
                "member_reused": reused,
                "operator_sees_providers": [p.get("name") for p in configured],
                "ticket": ticket,
            },
            indent=2,
        ))
        if not configured:
            print(
                "\n⚠️  operator sees ZERO providers — item 13 cannot distinguish "
                "'refused' from 'genuinely empty' in this root. Add one as "
                "operator first (checklist item 9), then re-run."
            )
        urls = ticket.get("urls") or []
        if urls:
            print(f"\nmember URL (open from the LAN IP, not loopback):\n  {urls[0]}")


asyncio.run(main())
