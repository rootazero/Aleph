#!/usr/bin/env python3
"""RPC driver for the per-principal spend budget real-machine fixture.

One process per call, one JSON object printed to stdout as the LAST line, so
`run.sh` can read it with `python3 -c 'import json,sys; ...'`. Every
subcommand follows the round-6 loopback-mint / LAN-redeem pattern
(`qa/multiuser_audit/pair_device.mjs` — that fixture's driver was later
ported to Node; this one stays Python): `resolve_connect_auth` authorises a
loopback peer unconditionally, on its FIRST line, before it ever reads a
device token — so testing "member" identity means connecting over the LAN
address with the device token on EVERY call, not just the ticket redemption.
A loopback connection with a device token attached is still just the
operator.

Subcommands:

  chat <url> <session_key> <message> [--model ID] [--device-token TOK]
       [--timeout SECS]
    Send `chat.send` (with an optional qualified model_override pinned to
    the `anthropic` provider — the QA mock) and wait for this run's own
    terminal frame. Prints
    `{"outcome": "complete"|"error"|"no_terminal_frame", "run_id": ...,
      "session_key": ..., "error_code": ..., "error": ..., "summary": {...}}`.
    `outcome: "no_terminal_frame"` is deliberately NOT called "timeout" —
    silence is exactly the failure mode task-12 exists to catch, and the two
    must not read the same in a log a human skims.

  query <url> [--device-token TOK]
    Call `spend.query {}`. Prints `{"ok": true, "result": {...}}` on success
    or `{"ok": false, "code": ..., "message": ...}` on an RPC error — the
    caller distinguishes AUTH_REQUIRED (admin gate) from RESOURCE_NOT_FOUND
    (visibility gate) by `code`, never by string-matching `message` alone.

  patch <url> <path> <patch_json>
    Call `config.patch {path, patch}` as operator (loopback). Prints the raw
    result object, including `reload_impact` when the patcher attached one.

  mint_and_redeem <local_url> <remote_url> <user_id> <device_id>
    The round-6 pattern, but returning the device token instead of just a
    pass/fail: mint a bootstrap ticket over LOOPBACK (operator), redeem it
    over the LAN address (what a remote Panel does). Prints
    `{"ok": true, "device_token": "..."}` or `{"ok": false, "error": "..."}`.
"""
import argparse
import asyncio
import json
import sys

import websockets


async def rpc(ws, method, params, rid=1):
    await ws.send(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}))
    while True:
        msg = json.loads(await ws.recv())
        if msg.get("id") == rid:
            return msg


async def connect(ws, device_token=None, client_type="cli"):
    params = {"client_type": client_type}
    if device_token:
        params["device_token"] = device_token
    return await rpc(ws, "connect", params, rid=0)


async def cmd_chat(args):
    async with websockets.connect(args.url) as ws:
        hello = await connect(ws, device_token=args.device_token)
        if "error" in hello:
            print(json.dumps({"outcome": "connect_failed", "error": hello["error"]}))
            return 1

        payload = {
            "message": args.message,
            "session_key": args.session_key,
            "stream": True,
        }
        if args.model:
            payload["model_override"] = {
                "kind": "qualified",
                "provider": "anthropic",
                "model": args.model,
            }
        sent = await rpc(ws, "chat.send", payload, rid=2)
        if "error" in sent:
            print(json.dumps({"outcome": "send_failed", "error": sent["error"]}))
            return 1
        run_id = sent["result"]["run_id"]
        session_key = sent["result"]["session_key"]

        deadline = asyncio.get_event_loop().time() + args.timeout
        while asyncio.get_event_loop().time() < deadline:
            try:
                raw = await asyncio.wait_for(ws.recv(), timeout=1.0)
            except asyncio.TimeoutError:
                continue
            msg = json.loads(raw)
            method = msg.get("method", "")
            params = msg.get("params", {}) or {}
            if method == "stream.run_complete" and params.get("run_id") == run_id:
                print(json.dumps({
                    "outcome": "complete",
                    "run_id": run_id,
                    "session_key": session_key,
                    "summary": params.get("summary", {}),
                }))
                return 0
            if method == "stream.run_error" and params.get("run_id") == run_id:
                print(json.dumps({
                    "outcome": "error",
                    "run_id": run_id,
                    "session_key": session_key,
                    "error_code": params.get("error_code"),
                    "error": params.get("error"),
                }))
                return 0

        # Silence, not a timeout in the ordinary sense: exactly the failure
        # `execute()`'s admission-arm early-return used to produce before
        # `deny_if_over_spend_and_report` existed (task-12's own finding).
        print(json.dumps({
            "outcome": "no_terminal_frame",
            "run_id": run_id,
            "session_key": session_key,
        }))
        return 0


async def cmd_query(args):
    async with websockets.connect(args.url) as ws:
        hello = await connect(ws, device_token=args.device_token)
        if "error" in hello:
            print(json.dumps({"ok": False, "code": None, "message": f"connect failed: {hello['error']}"}))
            return 1
        resp = await rpc(ws, "spend.query", {}, rid=2)
        if "error" in resp:
            err = resp["error"]
            print(json.dumps({"ok": False, "code": err.get("code"), "message": err.get("message")}))
            return 0
        print(json.dumps({"ok": True, "result": resp.get("result", {})}))
        return 0


async def cmd_patch(args):
    async with websockets.connect(args.url) as ws:
        hello = await connect(ws)
        if "error" in hello:
            print(json.dumps({"ok": False, "message": f"connect failed: {hello['error']}"}))
            return 1
        patch_value = json.loads(args.patch_json)
        resp = await rpc(ws, "config.patch", {"path": args.path, "patch": patch_value}, rid=2)
        if "error" in resp:
            print(json.dumps({"ok": False, "message": resp["error"]}))
            return 0
        print(json.dumps({"ok": True, "result": resp.get("result", {})}))
        return 0


async def cmd_mint_and_redeem(args):
    async with websockets.connect(args.local_url) as ws:
        hello = await connect(ws)
        if "error" in hello:
            print(json.dumps({"ok": False, "error": f"connect(loopback): {hello['error']}"}))
            return 1
        made = await rpc(ws, "gateway.ticket.create", {"user_id": args.user_id}, rid=2)
        if "error" in made:
            print(json.dumps({"ok": False, "error": f"ticket.create: {made['error']}"}))
            return 1
        ticket = made["result"]["ticket"]

    async with websockets.connect(args.remote_url) as ws:
        redeemed = await rpc(
            ws,
            "connect",
            {
                "client_type": "panel",
                "bootstrap_ticket": ticket,
                "device_id": args.device_id,
                "device_name": "QA Spend Budget",
            },
            rid=1,
        )
        if "error" in redeemed:
            print(json.dumps({"ok": False, "error": f"connect(ticket, remote): {redeemed['error']}"}))
            return 1
        token = redeemed.get("result", {}).get("device_token")
        if not token:
            print(json.dumps({"ok": False, "error": "remote connect returned no device_token"}))
            return 1
        print(json.dumps({"ok": True, "device_token": token}))
        return 0


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)

    c = sub.add_parser("chat")
    c.add_argument("url")
    c.add_argument("session_key")
    c.add_argument("message")
    c.add_argument("--model", default=None)
    c.add_argument("--device-token", dest="device_token", default=None)
    c.add_argument("--timeout", type=float, default=20.0)
    c.set_defaults(func=cmd_chat)

    q = sub.add_parser("query")
    q.add_argument("url")
    q.add_argument("--device-token", dest="device_token", default=None)
    q.set_defaults(func=cmd_query)

    pt = sub.add_parser("patch")
    pt.add_argument("url")
    pt.add_argument("path")
    pt.add_argument("patch_json")
    pt.set_defaults(func=cmd_patch)

    mr = sub.add_parser("mint_and_redeem")
    mr.add_argument("local_url")
    mr.add_argument("remote_url")
    mr.add_argument("user_id")
    mr.add_argument("device_id")
    mr.set_defaults(func=cmd_mint_and_redeem)

    args = p.parse_args()
    return asyncio.run(args.func(args))


if __name__ == "__main__":
    sys.exit(main())
