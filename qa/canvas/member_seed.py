#!/usr/bin/env python3
"""Seed the member-role scenario (checklist item 8) over loopback.

Item 8 could never be driven from the default fixture: loopback is always
operator, so member visibility needs a LAN bind + TLS + a member credential —
and until this script existed the recipe lived only in prose (README +
project memory). This turns the seeding half into an executable:

  1. `users.create` a member ("QA Member"),
  2. `projects.create` a room and `projects.member.add` the member,
  3. `canvas.create` an operator-private canvas (no project link) and a
     room canvas (linked to the project),
  4. `gateway.ticket.create` a one-time bootstrap ticket for the member,
  5. print the ids and the ready-to-open member URL.

The browser half stays manual/MCP-driven (that is the point of the QA):
open the printed URL from the machine's LAN IP — NOT loopback — accept the
self-signed cert (TOFU), and assert: the member's library shows ONLY the
room canvas; the private canvas id answers not-found; the room canvas is
editable from both sides and `canvas.updated` reaches both.

Prerequisites (the fixture boots loopback-only on purpose; see README):
  * scratch config edited to `[gateway] host = "0.0.0.0"` +
    `[gateway.tls] enabled = true`, server restarted;
  * run THIS script against loopback (it needs operator, which loopback is).

Usage:
    python3 qa/canvas/member_seed.py <port> [--tls]

`--tls` matches the TLS-enabled restart: loopback is then wss:// with the
gateway's self-signed cert, which the stdlib refuses — CERT_NONE here is the
QA-only trust decision the browser makes interactively (and the reason the
aleph CLI cannot drive this scenario: it has no --insecure).
"""

import asyncio
import json
import ssl
import sys

try:
    import websockets
except ImportError:
    sys.exit("needs the `websockets` package (pip install websockets)")

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18798
TLS = "--tls" in sys.argv


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
        connect = await rpc.call("connect", {"client_info": {"name": "canvas-member-seed"}})
        if not connect.get("authorized"):
            sys.exit(f"loopback connect not authorized: {json.dumps(connect)[:200]}")

        member = await rpc.call(
            "users.create", {"display_name": "QA Member", "role": "member"}
        )
        user_id = member["user"]["user_id"]

        project = await rpc.call("projects.create", {"name": "Canvas QA Room"})
        project_id = project["project"]["id"]
        await rpc.call("projects.member.add", {"id": project_id, "user_id": user_id})

        private = await rpc.call("canvas.create", {"title": "Operator private"})
        private_id = private["canvas"]["id"]
        room = await rpc.call(
            "canvas.create", {"title": "Room canvas", "project_id": project_id}
        )
        room_id = room["canvas"]["id"]

        ticket = await rpc.call("gateway.ticket.create", {"user_id": user_id})

        print(json.dumps(
            {
                "member_user_id": user_id,
                "project_id": project_id,
                "private_canvas_id": private_id,
                "room_canvas_id": room_id,
                "ticket": ticket,
            },
            indent=2,
        ))
        urls = ticket.get("urls") or []
        if urls:
            print(f"\nmember URL (open from the LAN IP, not loopback):\n  {urls[0]}")


asyncio.run(main())
