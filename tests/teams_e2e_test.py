#!/usr/bin/env python3
"""
Teams Evolution E2E Test — Production-level verification via WebSocket JSON-RPC.

Tests the full three-layer communication system:
1. Team creation with roles
2. Task delegation + artifact system
3. Message send/receive with to/cc routing
4. Threading + inbox
5. Review scoring with validation
6. Collaborative session lifecycle
7. Team digest
8. Disbandment cleanup

Usage: python3 tests/teams_e2e_test.py
Requires: Aleph server running on localhost:18790
"""

import asyncio
import json
import sys
import websockets

ALEPH_WS = "ws://127.0.0.1:18790/ws"
SHARED_TOKEN = None  # Will be read from DB

# Colors for output
GREEN = "\033[92m"
RED = "\033[91m"
YELLOW = "\033[93m"
CYAN = "\033[96m"
RESET = "\033[0m"
BOLD = "\033[1m"

class AlephClient:
    def __init__(self):
        self.ws = None
        self.device_token = None
        self.msg_id = 0

    async def connect(self, shared_token):
        self.ws = await websockets.connect(ALEPH_WS, close_timeout=5)
        resp = await self.rpc("connect", {
            "shared_token": shared_token,
            "device_name": "Teams E2E Test"
        })
        self.device_token = resp.get("token")
        return resp

    async def rpc(self, method, params=None):
        self.msg_id += 1
        msg = {"jsonrpc": "2.0", "id": self.msg_id, "method": method}
        if params:
            msg["params"] = params
        await self.ws.send(json.dumps(msg))
        while True:
            raw = await asyncio.wait_for(self.ws.recv(), timeout=30)
            data = json.loads(raw)
            # Skip notifications (no id)
            if "id" in data and data["id"] == self.msg_id:
                if "error" in data:
                    return {"_error": data["error"]}
                return data.get("result", {})

    async def chat(self, message, agent_id="main", timeout=120):
        """Send a chat message and wait for the complete response."""
        resp = await self.rpc("chat.send", {
            "message": message,
            "agent_id": agent_id,
        })
        if "_error" in resp:
            return resp

        run_id = resp.get("run_id", "")

        # Collect streaming response via RunEvent notifications
        full_text = ""
        tool_calls = []
        start = asyncio.get_event_loop().time()

        while True:
            try:
                raw = await asyncio.wait_for(self.ws.recv(), timeout=timeout)
                data = json.loads(raw)

                # RunEvents come as {"method": "event", "params": {...}} or direct event objects
                event = None
                if "method" in data and data["method"] == "event":
                    event = data.get("params", {})
                elif "type" in data:
                    event = data

                # Handle stream.* methods (Aleph's actual streaming format)
                method = data.get("method", "")

                # Params are nested under data["params"] for stream.* methods
                p = data.get("params", {}) if isinstance(data.get("params"), dict) else {}

                if method == "stream.response_chunk":
                    full_text += p.get("delta", "")

                elif method == "stream.tool_start":
                    tool_calls.append({
                        "name": p.get("tool_name", "") or p.get("tool_id", ""),
                        "input": p.get("params", {}),
                    })

                elif method == "stream.tool_end":
                    if tool_calls:
                        tool_calls[-1]["result"] = p.get("result", "")
                        tool_calls[-1]["error"] = p.get("error")

                elif method == "stream.run_complete":
                    break

                elif method == "stream.run_failed":
                    return {"_error": p.get("error", "run failed")}

                # Also handle "event" wrapper format
                elif method == "event":
                    event = data.get("params", {})
                    etype = event.get("type", "") if isinstance(event, dict) else ""
                    # event type notifications are informational, continue

                elapsed = asyncio.get_event_loop().time() - start
                if elapsed > timeout:
                    break

            except asyncio.TimeoutError:
                break

        return {
            "text": full_text,
            "tool_calls": tool_calls,
        }

    async def close(self):
        if self.ws:
            await self.ws.close()


def print_test(name, passed, detail=""):
    icon = f"{GREEN}PASS{RESET}" if passed else f"{RED}FAIL{RESET}"
    print(f"  [{icon}] {name}")
    if detail and not passed:
        print(f"         {YELLOW}{detail}{RESET}")


def print_section(name):
    print(f"\n{BOLD}{CYAN}=== {name} ==={RESET}")


async def test_rpc_tools(client):
    """Test teams tools directly via RPC (teams.list, teams.get)."""
    print_section("Phase 0: RPC API Verification")

    # Test teams.list
    resp = await client.rpc("teams.list")
    is_list = isinstance(resp, (list, dict))
    print_test("teams.list returns data", is_list, f"got: {type(resp)}")

    return True


async def test_team_creation_via_chat(client):
    """Test creating a team through natural language chat."""
    print_section("Phase 1: Team Creation via Chat")

    resp = await client.chat(
        "Use the team_create tool to create a team called 'research-alpha' "
        "with description 'AI research team'. Don't add any members yet, "
        "just create the team with yourself as leader. "
        "Return the team_id after creation.",
        timeout=60
    )

    if "_error" in resp:
        print_test("Team creation", False, str(resp["_error"]))
        return None

    has_tool = any(tc["name"] == "team_create" for tc in resp.get("tool_calls", []))
    print_test("LLM called team_create tool", has_tool,
               f"tools called: {[tc['name'] for tc in resp.get('tool_calls', [])]}")

    # Extract team_id from response
    text = resp.get("text", "")
    print_test("Got response text", bool(text), text[:100] if text else "empty")

    # Verify via RPC
    teams = await client.rpc("teams.list")
    if isinstance(teams, dict) and "teams" in teams:
        teams = teams["teams"]
    elif isinstance(teams, dict) and "_error" in teams:
        teams = []

    found = any(t.get("name") == "research-alpha" for t in (teams if isinstance(teams, list) else []))
    print_test("Team 'research-alpha' exists in teams.list", found,
               f"teams: {json.dumps(teams, ensure_ascii=False)[:200] if teams else 'empty'}")

    return resp


async def test_task_artifact_system(client):
    """Test task_submit and task_read_artifact tools."""
    print_section("Phase 2: Task Artifact System")

    # Create a task first
    resp = await client.chat(
        "Use the task_create tool to create a task with subject 'Explore quantum computing trends' "
        "and description 'Research current state of quantum computing'. "
        "Then use task_submit to submit a discovery artifact for that task with: "
        "artifact_type='discovery', title='QC Trends 2026', "
        "content='## Key Findings\\n1. Error correction improving\\n2. Hybrid algorithms gaining traction'. "
        "Finally use task_read_artifact to read back the artifact you just submitted. "
        "Report the artifact_id and confirm the content matches.",
        timeout=90
    )

    if "_error" in resp:
        print_test("Task + Artifact flow", False, str(resp["_error"]))
        return

    tools = [tc["name"] for tc in resp.get("tool_calls", [])]
    print_test("Called task_create", "task_create" in tools, f"tools: {tools}")
    print_test("Called task_submit", "task_submit" in tools, f"tools: {tools}")
    print_test("Called task_read_artifact", "task_read_artifact" in tools, f"tools: {tools}")


async def test_message_system(client):
    """Test message_send and inbox_read tools."""
    print_section("Phase 3: Message System (to/cc routing)")

    resp = await client.chat(
        "Use message_send to send a message within team 'research-alpha' (use the team_id from earlier). "
        "Send it to yourself (your own agent_id as the 'to' recipient). "
        "Use msg_type='message', subject='Test Message', content='Hello from teams e2e test'. "
        "Then use inbox_read (mode='inbox') to read your inbox for this team. "
        "Report how many messages you see.",
        timeout=60
    )

    if "_error" in resp:
        print_test("Message system", False, str(resp["_error"]))
        return

    tools = [tc["name"] for tc in resp.get("tool_calls", [])]
    print_test("Called message_send", "message_send" in tools, f"tools: {tools}")
    print_test("Called inbox_read", "inbox_read" in tools, f"tools: {tools}")


async def test_review_score(client):
    """Test review_score tool with validation."""
    print_section("Phase 4: Review Score Validation")

    resp = await client.chat(
        "Use the review_score tool to submit a review. Use the team_id from 'research-alpha'. "
        "Use any task_id and artifact_id (you can make them up for this test). "
        "Submit with: "
        "scores=[{dimension:'credibility',score:8,rationale:'Well sourced'}, "
        "{dimension:'logic',score:9,rationale:'Sound reasoning'}], "
        "overall_pass=true, "
        "challenges=[{point:'Missing comparison',severity:'major',evidence:'No baseline comparison provided'}, "
        "{point:'Sample size',severity:'minor',evidence:'Only 3 papers cited'}, "
        "{point:'Recency bias',severity:'minor',evidence:'All sources from 2026'}], "
        "improvement_suggestions=['Add historical comparison'], "
        "risks_if_accepted=['May overstate progress']. "
        "Report whether the review was accepted or rejected and any validation errors.",
        timeout=60
    )

    if "_error" in resp:
        print_test("Review score", False, str(resp["_error"]))
        return

    tools = [tc["name"] for tc in resp.get("tool_calls", [])]
    print_test("Called review_score", "review_score" in tools, f"tools: {tools}")

    text = resp.get("text", "").lower()
    # With 3 challenges and all scores >= 7, should be accepted
    print_test("Review accepted (3 challenges, scores>=7)",
               "accept" in text or "success" in text or "pass" in text,
               text[:200])


async def test_collaborative_session(client):
    """Test session_collaborate, session_turn, session_read."""
    print_section("Phase 5: Collaborative Session")

    resp = await client.chat(
        "Do the following steps in order: "
        "1. Use session_collaborate to start a collaborative session in team 'research-alpha' "
        "   with topic='Design review discussion', max_rounds=5, participants=[your own agent_id]. "
        "2. Use session_turn with mode='respond' to add a turn saying 'I propose approach B for the architecture'. "
        "3. Use session_turn with mode='conclude' to conclude the session with "
        "   conclusion='We agreed on approach B', agreed_by=[your agent_id]. "
        "4. Use session_read to verify the session is concluded. "
        "Report the session status and number of turns.",
        timeout=90
    )

    if "_error" in resp:
        print_test("Collaborative session", False, str(resp["_error"]))
        return

    tools = [tc["name"] for tc in resp.get("tool_calls", [])]
    print_test("Called session_collaborate", "session_collaborate" in tools, f"tools: {tools}")
    print_test("Called session_turn", "session_turn" in tools, f"tools: {tools}")
    print_test("Called session_read", "session_read" in tools, f"tools: {tools}")


async def test_team_digest(client):
    """Test team_digest tool."""
    print_section("Phase 6: Team Digest")

    resp = await client.chat(
        "Use team_digest for team 'research-alpha' (use the team_id) with hours=24. "
        "Summarize what the digest shows — how many events, what types of activity.",
        timeout=60
    )

    if "_error" in resp:
        print_test("Team digest", False, str(resp["_error"]))
        return

    tools = [tc["name"] for tc in resp.get("tool_calls", [])]
    print_test("Called team_digest", "team_digest" in tools, f"tools: {tools}")

    text = resp.get("text", "")
    print_test("Got digest response", bool(text), text[:200] if text else "empty")


async def test_team_disband(client):
    """Test team disbandment with cleanup."""
    print_section("Phase 7: Team Disbandment + Cleanup")

    resp = await client.chat(
        "Use team_disband to disband team 'research-alpha' (use the team_id). "
        "Report whether it succeeded and any cleanup that was performed.",
        timeout=60
    )

    if "_error" in resp:
        print_test("Team disband", False, str(resp["_error"]))
        return

    tools = [tc["name"] for tc in resp.get("tool_calls", [])]
    print_test("Called team_disband", "team_disband" in tools, f"tools: {tools}")

    # Verify team is disbanded
    teams = await client.rpc("teams.list")
    if isinstance(teams, dict) and "teams" in teams:
        teams = teams["teams"]

    disbanded = False
    if isinstance(teams, list):
        for t in teams:
            if t.get("name") == "research-alpha":
                disbanded = t.get("status") == "disbanded" or t.get("status", {}) == "Disbanded"
                break
    print_test("Team status is 'disbanded'", disbanded, f"teams: {json.dumps(teams, ensure_ascii=False)[:200] if teams else 'none'}")


async def main():
    import subprocess
    global SHARED_TOKEN

    # Get token from DB
    result = subprocess.run(
        ["sqlite3", f"{__import__('os').path.expanduser('~')}/.aleph/data/security.db",
         "SELECT plaintext_token FROM shared_token LIMIT 1;"],
        capture_output=True, text=True
    )
    SHARED_TOKEN = result.stdout.strip()
    if not SHARED_TOKEN:
        print(f"{RED}Cannot read shared token from DB{RESET}")
        sys.exit(1)

    print(f"{BOLD}Teams Evolution E2E Test{RESET}")
    print(f"Server: {ALEPH_WS}")
    print(f"Token: {SHARED_TOKEN[:20]}...")

    client = AlephClient()

    try:
        # Connect
        resp = await client.connect(SHARED_TOKEN)
        print_test("WebSocket connected", bool(resp.get("token")))

        # Run tests in order
        await test_rpc_tools(client)
        await test_team_creation_via_chat(client)
        await test_task_artifact_system(client)
        await test_message_system(client)
        await test_review_score(client)
        await test_collaborative_session(client)
        await test_team_digest(client)
        await test_team_disband(client)

        print(f"\n{BOLD}{GREEN}=== E2E Test Complete ==={RESET}")

    except Exception as e:
        print(f"\n{RED}Test failed with error: {e}{RESET}")
        import traceback
        traceback.print_exc()
    finally:
        await client.close()


if __name__ == "__main__":
    asyncio.run(main())
