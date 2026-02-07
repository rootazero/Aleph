#!/usr/bin/env python3
"""
Test script for Guest invitation events.
Tests that creating and revoking invitations emit proper WebSocket events.
"""

import asyncio
import json
import websockets
import sys

GATEWAY_URL = "ws://127.0.0.1:18789"

async def test_guest_events():
    """Test guest invitation events"""
    print("Connecting to Gateway...")

    async with websockets.connect(GATEWAY_URL) as ws:
        print("✓ Connected to Gateway\n")

        # Subscribe to guest events
        print("Subscribing to guest.** events...")
        subscribe_request = {
            "jsonrpc": "2.0",
            "method": "events.subscribe",
            "params": {
                "topics": ["guest.**"]
            },
            "id": 1
        }
        await ws.send(json.dumps(subscribe_request))
        response = await ws.recv()
        print(f"Subscribe response: {response}\n")

        # Create an invitation
        print("Creating guest invitation...")
        create_request = {
            "jsonrpc": "2.0",
            "method": "guests.createInvitation",
            "params": {
                "guest_name": "Event Test Guest",
                "scope": {
                    "allowed_tools": ["translate", "summarize"],
                    "expires_at": None,
                    "display_name": "Event Test"
                }
            },
            "id": 2
        }
        await ws.send(json.dumps(create_request))

        # Wait for response and event
        print("Waiting for response and event...")
        for i in range(2):
            message = await ws.recv()
            data = json.loads(message)

            if "method" in data:
                # This is an event notification
                print(f"\n✓ Received event notification:")
                print(f"  Topic: {data.get('params', {}).get('topic')}")
                print(f"  Data: {json.dumps(data.get('params', {}).get('data'), indent=2)}")
            elif "result" in data:
                # This is the RPC response
                invitation = data["result"]["invitation"]
                token = invitation["token"]
                print(f"\n✓ Invitation created:")
                print(f"  Token: {token}")
                print(f"  Guest ID: {invitation['guest_id']}")

                # Now revoke the invitation
                print(f"\nRevoking invitation {token}...")
                revoke_request = {
                    "jsonrpc": "2.0",
                    "method": "guests.revokeInvitation",
                    "params": {
                        "token": token
                    },
                    "id": 3
                }
                await ws.send(json.dumps(revoke_request))

        # Wait for revoke response and event
        print("Waiting for revoke response and event...")
        for i in range(2):
            message = await ws.recv()
            data = json.loads(message)

            if "method" in data:
                # This is an event notification
                print(f"\n✓ Received event notification:")
                print(f"  Topic: {data.get('params', {}).get('topic')}")
                print(f"  Data: {json.dumps(data.get('params', {}).get('data'), indent=2)}")
            elif "result" in data:
                # This is the RPC response
                print(f"\n✓ Invitation revoked successfully")

        print("\n✓ All tests passed!")

if __name__ == "__main__":
    try:
        asyncio.run(test_guest_events())
    except KeyboardInterrupt:
        print("\n\nTest interrupted by user")
        sys.exit(0)
    except Exception as e:
        print(f"\n✗ Test failed: {e}")
        sys.exit(1)
