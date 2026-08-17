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

Every step is find-or-create, because a seeder that is not idempotent
corrupts the very assertions it seeds. Steps 1-3 have no natural key on the
server: `users` is keyed on `user_id` and `display_name` is a presentation
label with no uniqueness constraint (correctly so -- nothing resolves a
principal by name), and the same holds for project names and canvas titles.
So a run that dies halfway and is retried used to leave behind a second
"QA Member" principal and, worse, a second "Operator private" / "Room canvas"
pair -- which silently breaks item 8's counting assertion ("the operator
control group sees exactly three"). `projects.member.add` needs no such care:
it is already `INSERT OR IGNORE` server-side.

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

# The names this seeder owns inside a throwaway root. They are the find-or-create
# keys — see the module docstring for why the server cannot provide one.
MEMBER_NAME = "QA Member"
ROOM_NAME = "Canvas QA Room"
PRIVATE_CANVAS_TITLE = "Operator private"
ROOM_CANVAS_TITLE = "Room canvas"


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

        reused = []

        def note(kind, obj_id, was_reused):
            if was_reused:
                reused.append(f"{kind}={obj_id}")
            return obj_id

        # --- 1. member principal -------------------------------------------
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
        if existing:
            user_id = note("user", existing["user_id"], True)
        else:
            member = await rpc.call(
                "users.create", {"display_name": MEMBER_NAME, "role": "member"}
            )
            user_id = note("user", member["user"]["user_id"], False)

        # --- 2. room + roster ----------------------------------------------
        projects = (await rpc.call("projects.list", {})).get("projects") or []
        existing = next((p for p in projects if p.get("name") == ROOM_NAME), None)
        if existing:
            project_id = note("project", existing["id"], True)
        else:
            project = await rpc.call("projects.create", {"name": ROOM_NAME})
            project_id = note("project", project["project"]["id"], False)
        # Server-side `INSERT OR IGNORE`, so this is safe to repeat.
        await rpc.call("projects.member.add", {"id": project_id, "user_id": user_id})

        # --- 3. the two canvases --------------------------------------------
        canvases = (await rpc.call("canvas.list", {})).get("canvases") or []

        async def find_or_create_canvas(title, project):
            match = next(
                (
                    c
                    for c in canvases
                    if c.get("title") == title and c.get("project_id") == project
                ),
                None,
            )
            if match:
                return note("canvas", match["id"], True)
            params = {"title": title}
            if project is not None:
                params["project_id"] = project
            created = await rpc.call("canvas.create", params)
            return note("canvas", created["canvas"]["id"], False)

        private_id = await find_or_create_canvas(PRIVATE_CANVAS_TITLE, None)
        room_id = await find_or_create_canvas(ROOM_CANVAS_TITLE, project_id)

        ticket = await rpc.call("gateway.ticket.create", {"user_id": user_id})

        print(json.dumps(
            {
                "member_user_id": user_id,
                "project_id": project_id,
                "private_canvas_id": private_id,
                "room_canvas_id": room_id,
                "ticket": ticket,
                "reused": reused,
            },
            indent=2,
        ))
        if reused:
            print(
                f"\n(reused {len(reused)} pre-existing object(s) — this root has "
                "been seeded before; the operator control group should still "
                "show exactly the two canvases above)"
            )
        urls = ticket.get("urls") or []
        if urls:
            print(f"\nmember URL (open from the LAN IP, not loopback):\n  {urls[0]}")


asyncio.run(main())
