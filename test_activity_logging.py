#!/usr/bin/env python3
"""
Test script for guest session activity logging.

Tests the complete flow:
1. Create a guest invitation
2. Connect as a guest
3. Make RPC requests
4. Query activity logs
"""

import asyncio
import json
import websockets
import sys
from datetime import datetime

GATEWAY_URL = "ws://127.0.0.1:18789"

class Colors:
    GREEN = '\033[92m'
    RED = '\033[91m'
    YELLOW = '\033[93m'
    BLUE = '\033[94m'
    RESET = '\033[0m'

def log(message, color=Colors.RESET):
    timestamp = datetime.now().strftime("%H:%M:%S")
    print(f"{color}[{timestamp}] {message}{Colors.RESET}")

async def send_rpc(ws, method, params=None, request_id=1):
    """Send JSON-RPC request and return response."""
    request = {
        "jsonrpc": "2.0",
        "method": method,
        "id": request_id
    }
    if params:
        request["params"] = params

    await ws.send(json.dumps(request))
    response_text = await ws.recv()
    return json.loads(response_text)

async def test_activity_logging():
    """Test the complete activity logging flow."""

    log("=== Guest Session Activity Logging Test ===", Colors.BLUE)

    # Step 1: Connect as owner and create invitation
    log("\n1. Connecting as owner...", Colors.YELLOW)
    try:
        async with websockets.connect(GATEWAY_URL) as ws:
            # Connect without auth (require_auth=false)
            response = await send_rpc(ws, "connect", {
                "client_type": "test",
                "client_version": "1.0.0"
            })

            if "error" in response:
                log(f"❌ Connect failed: {response['error']}", Colors.RED)
                return False

            log(f"✅ Connected as owner", Colors.GREEN)

            # Create guest invitation
            log("\n2. Creating guest invitation...", Colors.YELLOW)
            response = await send_rpc(ws, "guests.createInvitation", {
                "guest_name": "Test Guest",
                "scope": {
                    "allowed_tools": ["translate", "summarize"],
                    "expires_at": None,
                    "display_name": "Test Guest"
                }
            }, request_id=2)

            if "error" in response:
                log(f"❌ Create invitation failed: {response['error']}", Colors.RED)
                return False

            invitation = response["result"]["invitation"]
            guest_token = invitation["token"]
            guest_id = invitation["guest_id"]

            log(f"✅ Invitation created", Colors.GREEN)
            log(f"   Token: {guest_token[:20]}...", Colors.BLUE)
            log(f"   Guest ID: {guest_id}", Colors.BLUE)

    except Exception as e:
        log(f"❌ Owner connection failed: {e}", Colors.RED)
        return False

    # Step 2: Connect as guest
    log("\n3. Connecting as guest...", Colors.YELLOW)
    try:
        async with websockets.connect(GATEWAY_URL) as ws:
            # Connect with invitation token
            response = await send_rpc(ws, "connect", {
                "client_type": "test-guest",
                "client_version": "1.0.0",
                "invitation_token": guest_token
            })

            if "error" in response:
                log(f"❌ Guest connect failed: {response['error']}", Colors.RED)
                return False

            result = response["result"]
            session_token = result["token"]
            session_id = session_token.split(":")[1] if ":" in session_token else "unknown"

            log(f"✅ Connected as guest", Colors.GREEN)
            log(f"   Session ID: {session_id}", Colors.BLUE)
            log(f"   Device ID: {result['device_id']}", Colors.BLUE)

            # Step 3: Make some RPC requests to generate activity
            log("\n4. Making RPC requests to generate activity...", Colors.YELLOW)

            # Request 1: List sessions
            response = await send_rpc(ws, "sessions.list", {}, request_id=3)
            if "error" not in response:
                log(f"✅ sessions.list succeeded", Colors.GREEN)
            else:
                log(f"⚠️  sessions.list failed: {response['error']}", Colors.YELLOW)

            await asyncio.sleep(0.5)

            # Request 2: Get config
            response = await send_rpc(ws, "config.get", {}, request_id=4)
            if "error" not in response:
                log(f"✅ config.get succeeded", Colors.GREEN)
            else:
                log(f"⚠️  config.get failed: {response['error']}", Colors.YELLOW)

            await asyncio.sleep(0.5)

            # Request 3: List providers
            response = await send_rpc(ws, "providers.list", {}, request_id=5)
            if "error" not in response:
                log(f"✅ providers.list succeeded", Colors.GREEN)
            else:
                log(f"⚠️  providers.list failed: {response['error']}", Colors.YELLOW)

            log(f"\n   Generated 3 RPC requests", Colors.BLUE)

    except Exception as e:
        log(f"❌ Guest connection failed: {e}", Colors.RED)
        return False

    # Step 4: Query activity logs
    log("\n5. Querying activity logs...", Colors.YELLOW)
    try:
        async with websockets.connect(GATEWAY_URL) as ws:
            # Connect as owner
            response = await send_rpc(ws, "connect", {
                "client_type": "test",
                "client_version": "1.0.0"
            })

            if "error" in response:
                log(f"❌ Owner reconnect failed: {response['error']}", Colors.RED)
                return False

            # Query activity logs
            response = await send_rpc(ws, "guests.getActivityLogs", {
                "session_id": session_id,
                "limit": 100
            }, request_id=6)

            if "error" in response:
                log(f"❌ Get activity logs failed: {response['error']}", Colors.RED)
                log(f"   Error details: {json.dumps(response['error'], indent=2)}", Colors.RED)
                return False

            result = response["result"]["result"]
            logs = result["logs"]
            total = result["total"]

            log(f"✅ Activity logs retrieved", Colors.GREEN)
            log(f"   Total logs: {total}", Colors.BLUE)
            log(f"   Logs returned: {len(logs)}", Colors.BLUE)

            # Display logs
            if logs:
                log("\n6. Activity Log Details:", Colors.YELLOW)
                for i, log_entry in enumerate(logs, 1):
                    activity_type = log_entry["activity_type"]
                    status = log_entry["status"]
                    timestamp = log_entry["timestamp"]

                    # Format activity type
                    if "ToolCall" in activity_type:
                        type_str = f"Tool: {activity_type['ToolCall']['tool_name']}"
                    elif "RpcRequest" in activity_type:
                        type_str = f"RPC: {activity_type['RpcRequest']['method']}"
                    elif "SessionEvent" in activity_type:
                        type_str = f"Event: {activity_type['SessionEvent']['event']}"
                    else:
                        type_str = str(activity_type)

                    # Color code by status
                    status_color = Colors.GREEN if status == "Success" else Colors.RED

                    log(f"\n   Log #{i}:", Colors.BLUE)
                    log(f"     Type: {type_str}")
                    log(f"     Status: {status}", status_color)
                    log(f"     Time: {datetime.fromtimestamp(timestamp/1000).strftime('%H:%M:%S.%f')[:-3]}")
                    log(f"     ID: {log_entry['id']}")

                # Verify expected logs
                log("\n7. Verification:", Colors.YELLOW)

                # Check for session connected event
                connected_logs = [l for l in logs if "SessionEvent" in l["activity_type"]
                                 and l["activity_type"]["SessionEvent"]["event"] == "connected"]
                if connected_logs:
                    log(f"✅ Found 'connected' session event", Colors.GREEN)
                else:
                    log(f"⚠️  Missing 'connected' session event", Colors.YELLOW)

                # Check for RPC requests
                rpc_logs = [l for l in logs if "RpcRequest" in l["activity_type"]]
                log(f"✅ Found {len(rpc_logs)} RPC request logs", Colors.GREEN)

                # List RPC methods
                if rpc_logs:
                    methods = [l["activity_type"]["RpcRequest"]["method"] for l in rpc_logs]
                    log(f"   Methods: {', '.join(methods)}", Colors.BLUE)

                return True
            else:
                log(f"⚠️  No activity logs found (expected at least 4)", Colors.YELLOW)
                return False

    except Exception as e:
        log(f"❌ Query activity logs failed: {e}", Colors.RED)
        import traceback
        traceback.print_exc()
        return False

async def main():
    """Main test function."""
    try:
        success = await test_activity_logging()

        if success:
            log("\n" + "="*50, Colors.GREEN)
            log("✅ ALL TESTS PASSED", Colors.GREEN)
            log("="*50, Colors.GREEN)
            sys.exit(0)
        else:
            log("\n" + "="*50, Colors.RED)
            log("❌ TESTS FAILED", Colors.RED)
            log("="*50, Colors.RED)
            sys.exit(1)

    except KeyboardInterrupt:
        log("\n\n⚠️  Test interrupted by user", Colors.YELLOW)
        sys.exit(1)
    except Exception as e:
        log(f"\n❌ Unexpected error: {e}", Colors.RED)
        import traceback
        traceback.print_exc()
        sys.exit(1)

if __name__ == "__main__":
    print("\n" + "="*50)
    print("Guest Session Activity Logging Test")
    print("="*50 + "\n")

    asyncio.run(main())
