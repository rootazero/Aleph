#!/usr/bin/env python3
"""ws_send.py — one-shot JSON-RPC over Aleph gateway WebSocket.

Usage:
  ws_send.py --token TOKEN --method agent.run --params '{"message":"hi","session_key":"agent:main:dm:operator"}'
  ws_send.py --token TOKEN --method agent.run --params-file /path/to/params.json --stream-events

The script:
  1. Connects to ws://127.0.0.1:18790/ws
  2. Sends `connect` with the bearer token (always)
  3. Optionally subscribes to events.subscribe pattern '*'
  4. Sends the requested method call
  5. Streams incoming frames to stdout (one JSON per line) until EOF or --timeout-seconds
"""
import argparse
import asyncio
import json
import sys
import uuid

import websockets


async def run(args: argparse.Namespace) -> int:
    if args.params_file:
        with open(args.params_file) as f:
            params = json.load(f)
    else:
        params = json.loads(args.params or "{}")

    uri = args.uri
    async with websockets.connect(uri, max_size=8 * 1024 * 1024) as ws:
        # 1. connect
        connect_id = str(uuid.uuid4())
        await ws.send(json.dumps({
            "jsonrpc": "2.0",
            "id": connect_id,
            "method": "connect",
            "params": {
                "shared_token": args.token,
                "device_name": "e2e-validator",
                "device_id": "e2e-validator",
            },
        }))
        # await connect ack
        ack = await asyncio.wait_for(ws.recv(), timeout=10)
        print(ack, flush=True)

        # 2. optional events.subscribe
        if args.stream_events:
            sub_id = str(uuid.uuid4())
            topics = [t for t in args.event_pattern.split(",") if t]
            await ws.send(json.dumps({
                "jsonrpc": "2.0",
                "id": sub_id,
                "method": "events.subscribe",
                "params": {"topics": topics},
            }))
            sub_ack = await asyncio.wait_for(ws.recv(), timeout=5)
            print(sub_ack, flush=True)

        # 3. main RPC
        call_id = str(uuid.uuid4())
        await ws.send(json.dumps({
            "jsonrpc": "2.0",
            "id": call_id,
            "method": args.method,
            "params": params,
        }))

        # 4. stream until wall-clock timeout, EOF, or break-on marker
        deadline = asyncio.get_event_loop().time() + args.timeout_seconds
        break_markers = [m for m in args.break_on.split(",") if m] if args.break_on else []
        try:
            while True:
                remaining = deadline - asyncio.get_event_loop().time()
                if remaining <= 0:
                    break
                frame = await asyncio.wait_for(ws.recv(), timeout=remaining)
                print(frame, flush=True)
                if not args.stream_events:
                    break
                if break_markers and any(
                    f'"method":"{m}"' in frame or f'"topic":"{m}"' in frame
                    for m in break_markers
                ):
                    break
        except asyncio.TimeoutError:
            pass
        except websockets.ConnectionClosed:
            pass
    return 0


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--uri", default="ws://127.0.0.1:18790/ws")
    p.add_argument("--token", required=True)
    p.add_argument("--method", required=True)
    p.add_argument("--params", default=None, help="inline JSON params")
    p.add_argument("--params-file", default=None, help="path to JSON params file")
    p.add_argument("--stream-events", action="store_true")
    p.add_argument("--event-pattern", default="*")
    p.add_argument("--timeout-seconds", type=float, default=30.0,
                   help="wall-clock deadline for the streaming loop (not per-frame)")
    p.add_argument("--break-on", default="",
                   help="comma-separated substrings; if any matches a frame, exit early")
    args = p.parse_args()
    return asyncio.run(run(args))


if __name__ == "__main__":
    sys.exit(main())
