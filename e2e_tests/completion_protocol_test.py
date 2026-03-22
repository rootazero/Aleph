#!/usr/bin/env python3
"""
E2E Test: Persistent Completion Protocol
=========================================
Verifies the completion protocol works in production by:
1. Pure Q&A stops naturally (no nudge, no tag required)
2. Tool-using tasks trigger completion protocol (tag required)
3. Multi-step tool tasks show full protocol in action
4. Monitors gateway.log for completion protocol log probes
"""

import asyncio
import json
import os
import sys
import time
import websockets

TOKEN = "e93e1b11-0c29-43e4-8164-590852d2d45d:545048137890cd796382dfb9f880f0b82b216da3f8a122b0ed4d83974aad4c90"
WS_URL = "ws://127.0.0.1:18790/ws"
LOG_FILE = os.path.expanduser("~/.aleph/logs/gateway.log")

passed = 0
failed = 0


def check(name, condition, detail=""):
    global passed, failed
    if condition:
        print(f"  ✓ {name}")
        passed += 1
    else:
        print(f"  ✗ {name}" + (f" — {detail}" if detail else ""))
        failed += 1
    return condition


def get_log_position():
    try:
        with open(LOG_FILE, "r") as f:
            f.seek(0, 2)
            return f.tell()
    except FileNotFoundError:
        return 0


def read_new_logs(start_pos):
    try:
        with open(LOG_FILE, "r") as f:
            f.seek(start_pos)
            return f.read()
    except FileNotFoundError:
        return ""


class AlephClient:
    def __init__(self):
        self.ws = None
        self.msg_id = 0

    async def connect(self):
        self.ws = await websockets.connect(WS_URL, max_size=10 * 1024 * 1024)
        self.msg_id += 1
        await self.ws.send(json.dumps({
            "jsonrpc": "2.0", "method": "connect",
            "params": {"token": TOKEN}, "id": self.msg_id
        }))
        resp = json.loads(await self.ws.recv())
        assert "result" in resp, f"Connect failed: {resp}"
        await self._drain()

    async def _drain(self, timeout=0.5):
        try:
            while True:
                await asyncio.wait_for(self.ws.recv(), timeout=timeout)
        except Exception:
            pass

    async def chat(self, message, timeout=180):
        """Send chat message and collect streaming response until run_complete."""
        # Drain any leftover messages from previous runs
        await self._drain(timeout=1.0)

        self.msg_id += 1
        rid = self.msg_id
        await self.ws.send(json.dumps({
            "jsonrpc": "2.0", "method": "chat.send",
            "params": {"message": message, "channel": "gui:chat", "stream": True},
            "id": rid
        }))

        run_id = None
        text_parts = []
        final_response = ""
        tool_calls = []
        completed = False
        got_rpc_response = False
        deadline = asyncio.get_event_loop().time() + timeout

        while not completed:
            remaining = deadline - asyncio.get_event_loop().time()
            if remaining <= 0:
                print(f"  ⚠ Timeout after {timeout}s")
                break
            try:
                raw = await asyncio.wait_for(self.ws.recv(), timeout=min(remaining, 5.0))
                d = json.loads(raw)
                m = d.get("method", "")
                p = d.get("params", {})

                # RPC response with run_id
                if d.get("id") == rid and "result" in d:
                    new_run_id = d["result"].get("run_id", "")
                    if new_run_id:
                        run_id = new_run_id
                    got_rpc_response = True
                    if completed:
                        break
                    continue

                if d.get("id") == rid and "error" in d:
                    print(f"  !! RPC Error: {d['error']}")
                    break

                # Only process events for OUR run
                evt_run_id = p.get("run_id", "")
                if run_id and evt_run_id and evt_run_id != run_id:
                    continue
                # If we don't have run_id yet, skip events that have a different run_id
                if not run_id and evt_run_id:
                    # Accept this event's run_id as ours if we haven't seen our RPC response yet
                    if not got_rpc_response:
                        continue

                if m == "stream.response_chunk":
                    content = p.get("content", "")
                    text_parts.append(content)
                elif m == "stream.tool_start":
                    tool_name = p.get("name", p.get("tool", "unknown"))
                    tool_calls.append(tool_name)
                    print(f"    🔧 Tool: {tool_name}")
                elif m == "stream.run_complete":
                    summary = p.get("summary", {})
                    final_response = summary.get("final_response", "")
                    # Debug: show what's in run_complete
                    iterations = summary.get("iterations", "?")
                    tc = summary.get("tool_calls_made", summary.get("tool_calls", "?"))
                    print(f"    📊 run_complete: iterations={iterations}, tool_calls={tc}")
                    print(f"    📊 final_response (last 200): ...{final_response[-200:]}")
                    completed = True
                    break
                elif m == "stream.error":
                    print(f"  !! Stream error: {p}")
                    completed = True
                    break
            except asyncio.TimeoutError:
                continue

        streamed_text = "".join(text_parts)
        # Prefer final_response (contains complete text including completion tags)
        # over streamed chunks (which may only have intermediate text)
        full_text = final_response or streamed_text
        return {
            "text": full_text,
            "tool_calls": tool_calls,
            "completed": completed,
        }

    async def close(self):
        if self.ws:
            await self.ws.close()


async def main():
    print("=" * 60)
    print("Persistent Completion Protocol — E2E Test")
    print("=" * 60)

    client = AlephClient()
    print("\n[0] Connecting...")
    await client.connect()
    print("  Connected!")

    # =========================================================================
    # Test 1: Pure Q&A — no tools, no completion protocol
    # =========================================================================
    print("\n" + "-" * 60)
    print("[Test 1] Pure Q&A — should stop naturally, no nudge")
    print("-" * 60)

    log_pos = get_log_position()
    result = await client.chat(
        "What is 2 + 2? Reply with just the number.",
        timeout=60
    )
    new_logs = read_new_logs(log_pos)

    print(f"  Response: {result['text'][:200]}")
    print(f"  Tools used: {result['tool_calls']}")

    check("Completed", result["completed"])
    check("No tools used", len(result["tool_calls"]) == 0)
    check("No completion nudge in logs",
          "Completion protocol:" not in new_logs,
          "Nudge fired for pure Q&A!")
    check("No <task-complete/> required",
          "<task-complete/>" not in result["text"] or len(result["tool_calls"]) == 0,
          "Tag not required for Q&A")

    # =========================================================================
    # Test 2: Single tool task — completion protocol expected
    # =========================================================================
    print("\n" + "-" * 60)
    print("[Test 2] Single tool task — completion protocol expected")
    print("-" * 60)

    log_pos = get_log_position()
    result = await client.chat(
        "Run this shell command: echo 'hello from aleph'. Show me the output.",
        timeout=120
    )
    new_logs = read_new_logs(log_pos)

    print(f"  Response (last 400 chars): ...{result['text'][-400:]}")
    print(f"  Tools used: {result['tool_calls']}")

    nudge_fired = "Completion protocol:" in new_logs and "injecting nudge" in new_logs
    has_tag = "<task-complete/>" in result["text"]
    has_check_block = "<completion-check>" in result["text"]

    check("Completed", result["completed"])
    check("Used at least one tool", len(result["tool_calls"]) > 0,
          f"Expected tool use, got: {result['tool_calls']}")

    if has_tag:
        check("Has <task-complete/> tag", True)
        check("Has <completion-check> block", has_check_block,
              "Tag present but no verification block")
    elif nudge_fired:
        check("Nudge was injected (protocol working)", True)
        print("  ℹ LLM didn't output tag initially, nudge corrected it")
    else:
        check("Completion protocol engaged", False,
              "Neither tag nor nudge detected — check logs")

    # =========================================================================
    # Test 3: Multi-step tool task — full protocol verification
    # =========================================================================
    print("\n" + "-" * 60)
    print("[Test 3] Multi-step tool task — full protocol verification")
    print("-" * 60)

    log_pos = get_log_position()
    result = await client.chat(
        "I need you to do two things: "
        "(1) Run 'date +%Y-%m-%d' in the shell and tell me today's date, "
        "(2) Run 'uname -s' and tell me my OS. "
        "Show both results clearly.",
        timeout=120
    )
    new_logs = read_new_logs(log_pos)

    print(f"  Response (last 400 chars): ...{result['text'][-400:]}")
    print(f"  Tools used: {result['tool_calls']}")

    nudge_count = new_logs.count("injecting nudge")
    has_tag = "<task-complete/>" in result["text"]
    has_check_block = "<completion-check>" in result["text"]

    check("Completed", result["completed"])
    check("Used multiple tools", len(result["tool_calls"]) >= 2,
          f"Expected >= 2 tools, got {len(result['tool_calls'])}")

    if has_tag:
        check("Has <task-complete/> tag", True)
        check("Has <completion-check> block", has_check_block,
              "Tag present but no verification block")
    elif nudge_count > 0:
        check(f"Nudge injected {nudge_count} time(s) (protocol active)", True)
    else:
        check("Completion protocol engaged", False,
              "Neither tag nor nudge detected")

    # =========================================================================
    # Summary
    # =========================================================================
    print("\n" + "=" * 60)
    print(f"Results: {passed} passed, {failed} failed")
    print("=" * 60)

    # Print all completion protocol log lines
    all_logs = read_new_logs(0)
    protocol_lines = [
        line.strip() for line in all_logs.split("\n")
        if "Completion protocol:" in line
    ]
    if protocol_lines:
        print(f"\n📋 Completion protocol log entries ({len(protocol_lines)}):")
        for line in protocol_lines[-20:]:
            print(f"  {line}")
    else:
        print("\n📋 No completion protocol log entries found")

    await client.close()
    return failed == 0


if __name__ == "__main__":
    ok = asyncio.run(main())
    sys.exit(0 if ok else 1)
