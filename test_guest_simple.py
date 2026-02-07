#!/usr/bin/env python3
"""
Simple test to debug guest authentication
"""

import json
import asyncio
import websockets

GATEWAY_URL = "ws://127.0.0.1:18789"

async def test():
    print("Connecting to Gateway...")
    async with websockets.connect(GATEWAY_URL) as ws:
        # Step 1: Create invitation
        print("\n1. Creating invitation...")
        req1 = {
            "jsonrpc": "2.0",
            "method": "guests.createInvitation",
            "params": {
                "guest_name": "Test",
                "scope": {
                    "allowed_tools": ["translate"],
                    "expires_at": None,
                    "display_name": "Test"
                }
            },
            "id": 1
        }
        print(f"Request: {json.dumps(req1, indent=2)}")
        await ws.send(json.dumps(req1))
        resp1 = await ws.recv()
        print(f"Response: {resp1}")
        result1 = json.loads(resp1)
        token = result1["result"]["invitation"]["token"]
        print(f"\nInvitation token: {token}")

        # Step 2: Connect as guest
        print("\n2. Connecting as guest...")
        async with websockets.connect(GATEWAY_URL) as guest_ws:
            req2 = {
                "jsonrpc": "2.0",
                "method": "connect",
                "params": {
                    "invitation_token": token
                },
                "id": 1
            }
            print(f"Request: {json.dumps(req2, indent=2)}")
            await guest_ws.send(json.dumps(req2))
            resp2 = await guest_ws.recv()
            print(f"Response: {resp2}")
            result2 = json.loads(resp2)

            if "result" in result2:
                guest_token = result2["result"]["token"]
                device_id = result2["result"]["device_id"]
                print(f"\nGuest token: {guest_token}")
                print(f"Device ID: {device_id}")
                print(f"Is guest token? {guest_token.startswith('guest:')}")
                print(f"Is guest device? {device_id.startswith('guest-')}")

asyncio.run(test())
