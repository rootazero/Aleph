#!/usr/bin/env python3
"""Drive one spend scenario over the gateway and print the result as JSON.

    drive_spend.py <ws-url> <verb> [args...]

Verbs
-----
  run <channel> [--ticket T] [--device D] [--message M]
      One agent turn. Prints {"ok", "code", "message", "run_id"} — `code` is
      the wire code of a terminal `stream.run_error` (e.g. `SPEND_EXHAUSTED`),
      empty on success.
  query [--ticket T] [--device D]
      `spend.query`. Prints the raw result, or {"error": {...}} on refusal —
      the DIFFERENCE between a refusal and an empty report is the assertion in
      several places here, so the two must never render the same.
  patch <path> <json-value>
      `config.patch`. Prints {"impact", "ok"} so the caller can assert `Live`.

Why a ticket/device pair rather than a second loopback connection: a loopback
peer is authorised on `resolve_connect_auth`'s first line, before it reads
`bootstrap_ticket`, so a member identity only exists over a non-loopback URL.
"""
import argparse
import asyncio
import json
import sys

import websockets

TERMINAL = ("stream.run_complete", "stream.run_error")


async def rpc(ws, method, params, rid):
    await ws.send(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}))
    while True:
        msg = json.loads(await ws.recv())
        if msg.get("id") == rid:
            return msg


async def connect(ws, ticket, device):
    params = {"client_type": "cli"}
    if ticket:
        params = {
            "client_type": "panel",
            "bootstrap_ticket": ticket,
            "device_id": device or "qa-spend-device",
            "device_name": "QA Spend",
        }
    return await rpc(ws, "connect", params, 1)


async def await_terminal(ws, run_id, budget):
    """Wait for this run's terminal frame.

    Matched on `run_id`, never on "the next terminal frame that arrives": the
    socket carries every run this connection can see, and a fixture that takes
    the first one would pass or fail on arrival order.
    """
    loop = asyncio.get_event_loop()
    end = loop.time() + budget
    while loop.time() < end:
        try:
            raw = await asyncio.wait_for(ws.recv(), timeout=max(0.5, end - loop.time()))
        except asyncio.TimeoutError:
            break
        msg = json.loads(raw)
        topic = msg.get("method") or msg.get("topic") or ""
        if topic not in TERMINAL:
            continue
        payload = msg.get("params") or {}
        if isinstance(payload, dict) and payload.get("run_id") not in (None, run_id):
            continue
        return topic, payload
    return None, {}


async def do_run(url, args):
    async with websockets.connect(url) as ws:
        hello = await connect(ws, args.ticket, args.device)
        if "error" in hello:
            return {"ok": False, "code": "CONNECT_REFUSED", "message": str(hello["error"])}
        params = {"message": args.message, "channel": args.channel, "exec_tier": "full"}
        if args.language:
            params["language"] = args.language
        sent = await rpc(ws, "chat.send", params, 2)
        if "error" in sent:
            return {"ok": False, "code": "SEND_REFUSED", "message": str(sent["error"])}
        run_id = sent["result"]["run_id"]
        topic, payload = await await_terminal(ws, run_id, args.budget)
        if topic is None:
            return {"ok": False, "code": "TIMEOUT", "message": "no terminal frame", "run_id": run_id}
        return {
            "ok": topic == "stream.run_complete",
            "code": payload.get("code") or payload.get("error_code") or "",
            "message": payload.get("message") or payload.get("error") or "",
            "run_id": run_id,
        }


async def do_query(url, args):
    async with websockets.connect(url) as ws:
        hello = await connect(ws, args.ticket, args.device)
        if "error" in hello:
            return {"error": {"code": -1, "message": f"connect: {hello['error']}"}}
        got = await rpc(ws, "spend.query", {}, 2)
        if "error" in got:
            return {"error": got["error"]}
        return got["result"]


async def do_patch(url, args):
    async with websockets.connect(url) as ws:
        hello = await connect(ws, args.ticket, args.device)
        if "error" in hello:
            return {"ok": False, "impact": "", "message": str(hello["error"])}
        got = await rpc(
            ws,
            "config.patch",
            {"path": args.path, "value": json.loads(args.value)},
            2,
        )
        if "error" in got:
            return {"ok": False, "impact": "", "message": str(got["error"])}
        res = got["result"] or {}
        return {"ok": True, "impact": res.get("reload_impact") or res.get("impact") or "", "raw": res}


def main():
    p = argparse.ArgumentParser()
    p.add_argument("url")
    sub = p.add_subparsers(dest="verb", required=True)

    r = sub.add_parser("run")
    r.add_argument("channel")
    r.add_argument("--message", default="Say hello.")
    r.add_argument("--language", default="")
    r.add_argument("--budget", type=float, default=180.0)

    q = sub.add_parser("query")

    c = sub.add_parser("patch")
    c.add_argument("path")
    c.add_argument("value")

    for s in (r, q, c):
        s.add_argument("--ticket", default="")
        s.add_argument("--device", default="")

    args = p.parse_args()
    fn = {"run": do_run, "query": do_query, "patch": do_patch}[args.verb]
    out = asyncio.get_event_loop().run_until_complete(fn(args.url, args))
    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
