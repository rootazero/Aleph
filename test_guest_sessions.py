#!/usr/bin/env python3
"""
Test script for Guest Session Monitoring
"""

import json
import asyncio
import websockets
import sys

GATEWAY_URL = "ws://127.0.0.1:18789"

async def send_rpc(ws, method, params=None, request_id=1):
    """Send JSON-RPC request and get response"""
    request = {
        "jsonrpc": "2.0",
        "method": method,
        "id": request_id
    }
    if params:
        request["params"] = params

    await ws.send(json.dumps(request))
    response = await ws.recv()
    return json.loads(response)

async def test_guest_sessions():
    """Test the complete guest session flow"""

    print("=" * 60)
    print("Guest Session Monitoring Test")
    print("=" * 60)

    # Connect to Gateway
    print("\n1. Connecting to Gateway...")
    async with websockets.connect(GATEWAY_URL) as ws:
        print("   ✓ Connected to Gateway")

        # Step 1: Create a guest invitation
        print("\n2. Creating guest invitation...")
        result = await send_rpc(ws, "guests.createInvitation", {
            "guest_name": "Test Guest",
            "scope": {
                "allowed_tools": ["translate", "summarize", "search"],
                "expires_at": None,
                "display_name": "Test Guest Session"
            }
        }, request_id=1)

        if "error" in result:
            print(f"   ✗ Error: {result['error']}")
            return

        invitation = result["result"]["invitation"]
        token = invitation["token"]
        guest_id = invitation["guest_id"]

        print(f"   ✓ Invitation created")
        print(f"     Token: {token[:20]}...")
        print(f"     Guest ID: {guest_id}")
        print(f"     URL: {invitation['url']}")

        # Step 2: Connect as guest using the invitation
        print("\n3. Connecting as guest...")
        async with websockets.connect(GATEWAY_URL) as guest_ws:
            # Send connect request with invitation token
            connect_result = await send_rpc(guest_ws, "connect", {
                "device_id": f"test-device-{guest_id}",
                "device_name": "Test Device",
                "invitation_token": token
            }, request_id=1)

            if "error" in connect_result:
                print(f"   ✗ Error: {connect_result['error']}")
                return

            guest_token = connect_result["result"]["token"]
            print(f"   ✓ Guest connected")
            print(f"     Guest Token: {guest_token[:30]}...")
            print(f"     Full connect result: {json.dumps(connect_result, indent=2)}")

            # Extract session_id from guest token (format: guest:{session_id}:{token})
            session_id = guest_token.split(":")[1] if guest_token.startswith("guest:") else None
            print(f"     Session ID: {session_id}")

            # Step 3: List active sessions
            print("\n4. Listing active sessions...")
            sessions_result = await send_rpc(ws, "guests.listSessions", request_id=2)
            print(f"     Sessions result: {json.dumps(sessions_result, indent=2)}")

            if "error" in sessions_result:
                print(f"   ✗ Error: {sessions_result['error']}")
                return

            sessions = sessions_result.get("result", {}).get("sessions", [])
            print(f"   ✓ Found {len(sessions)} active session(s)")

            for session in sessions:
                print(f"\n     Session Details:")
                print(f"       Session ID: {session['session_id']}")
                print(f"       Guest ID: {session['guest_id']}")
                print(f"       Guest Name: {session['guest_name']}")
                print(f"       Connection ID: {session['connection_id']}")
                print(f"       Connected At: {session['connected_at']}")
                print(f"       Request Count: {session['request_count']}")
                print(f"       Tools Used: {session['tools_used']}")
                print(f"       Allowed Tools: {session['scope']['allowed_tools']}")

            # Step 4: Wait a bit to simulate activity
            print("\n5. Simulating guest activity...")
            await asyncio.sleep(2)

            # Step 5: Terminate the session
            if session_id:
                print(f"\n6. Terminating session {session_id}...")
                terminate_result = await send_rpc(ws, "guests.terminateSession", {
                    "session_id": session_id
                }, request_id=3)

                if "error" in terminate_result:
                    print(f"   ✗ Error: {terminate_result['error']}")
                else:
                    print(f"   ✓ Session terminated successfully")

            # Step 6: Verify session is removed
            print("\n7. Verifying session removal...")
            sessions_result = await send_rpc(ws, "guests.listSessions", request_id=4)
            sessions = sessions_result["result"]["sessions"]
            print(f"   ✓ Active sessions: {len(sessions)}")

            if len(sessions) == 0:
                print("   ✓ Session successfully removed from active list")
            else:
                print("   ⚠ Session still in active list")

    print("\n" + "=" * 60)
    print("Test completed!")
    print("=" * 60)

if __name__ == "__main__":
    try:
        asyncio.run(test_guest_sessions())
    except KeyboardInterrupt:
        print("\n\nTest interrupted by user")
        sys.exit(0)
    except Exception as e:
        print(f"\n\n✗ Test failed with error: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)
