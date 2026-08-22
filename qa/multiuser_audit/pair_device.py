#!/usr/bin/env python3
"""Mint a bootstrap ticket bound to a principal and redeem it as a device.

    pair_device.py <loopback-ws-url> <remote-ws-url> <user_id> <device_id>

## Why two URLs

`resolve_connect_auth` returns `Authorized` for a loopback peer on its first
line, before it ever looks at a `bootstrap_ticket`. That is the single-machine
zero-credential guarantee and it is correct — but it means a ticket redeemed
over `127.0.0.1` creates no device row at all, silently and successfully. The
first version of this fixture assumed otherwise and reported four failures that
were its own.

So the two halves run over the two peers they belong to: minting needs operator
(loopback), redeeming is what a remote Panel does (LAN address). This is also
the only way the fixture exercises the real pairing path rather than a
loopback-shaped imitation of it.
"""
import asyncio
import json
import sys

import websockets


async def rpc(ws, method, params, rid):
    await ws.send(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}))
    while True:
        msg = json.loads(await ws.recv())
        # Event frames share the socket; only a reply carries our id.
        if msg.get("id") == rid:
            return msg


async def main(local_url, remote_url, user_id, device_id):
    async with websockets.connect(local_url) as ws:
        hello = await rpc(ws, "connect", {"client_type": "cli"}, 1)
        if "error" in hello:
            print(f"FAIL connect(loopback): {hello['error']}", file=sys.stderr)
            return 1
        made = await rpc(ws, "gateway.ticket.create", {"user_id": user_id}, 2)
        if "error" in made:
            print(f"FAIL ticket.create: {made['error']}", file=sys.stderr)
            return 1
        ticket = made["result"]["ticket"]

    async with websockets.connect(remote_url) as ws:
        redeemed = await rpc(
            ws,
            "connect",
            {"client_type": "panel", "bootstrap_ticket": ticket,
             "device_id": device_id, "device_name": "QA Panel"},
            1,
        )
        if "error" in redeemed:
            print(f"FAIL connect(ticket, remote): {redeemed['error']}", file=sys.stderr)
            return 1
        # A remote connection that was NOT handed a device token means the
        # ticket path did not run — which is exactly the failure the loopback
        # short-circuit produces, and it must not read as success.
        result = redeemed.get("result", {})
        if not result.get("device_token"):
            print("FAIL remote connect returned no device token; the ticket was not exchanged",
                  file=sys.stderr)
            print(json.dumps(result, indent=2)[:600], file=sys.stderr)
            return 1

    async with websockets.connect(local_url) as ws:
        await rpc(ws, "connect", {"client_type": "cli"}, 1)
        listed = await rpc(ws, "gateway.devices.list", {}, 2)
        devices = listed.get("result", {}).get("devices", [])
        mine = [d for d in devices if d.get("device_id") == device_id]
        if not mine:
            print(f"FAIL device {device_id} absent after redeeming its ticket", file=sys.stderr)
            print(json.dumps(devices, indent=2), file=sys.stderr)
            return 1
        # Round-4 gave this list an owner column; the deactivation receipt and
        # the audit line both claim to name the same principal, so check that
        # the three agree rather than trusting any one of them.
        if mine[0].get("user_id") != user_id:
            print(f"FAIL device bound to {mine[0].get('user_id')!r}, expected {user_id!r}",
                  file=sys.stderr)
            return 1
        print(f"OK device {device_id} paired and bound to {user_id}")
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main(sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4])))
