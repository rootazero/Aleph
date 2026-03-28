#!/usr/bin/env python3
"""
E2E test: Model Behavior System — verify different providers get different system prompts.

Verification strategy:
1. Connect to live server
2. Send a simple chat message to trigger an agent loop
3. Check server logs for "Model behavior resolved" with correct protocol + behavior_name
4. Verify the chat completes successfully (no crash from behavior injection)
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", ".claude", "skills", "aleph-e2e-verify", "scripts"))

import asyncio
import json
import subprocess
import time
from pathlib import Path

from aleph_e2e_client import (
    AlephClient, check, section, summary, get_shared_token,
)

LOG_LOCATIONS = [
    Path.home() / ".aleph" / "logs" / "server.log",
]

# Also check server stdout captured by background runner
_TMP_TASKS = Path("/private/tmp") / "claude-502"
for d in sorted(_TMP_TASKS.glob("*/*/tasks/*.output")) if _TMP_TASKS.exists() else []:
    LOG_LOCATIONS.append(d)


def grep_log(pattern, after_ts=None):
    """Search server logs for a pattern across all known log locations."""
    matches = []
    for log_path in LOG_LOCATIONS:
        if not log_path.exists():
            continue
        try:
            lines = log_path.read_text().splitlines()
        except Exception:
            continue
        for line in lines:
            if pattern in line:
                if after_ts:
                    ts_str = line[:23] if len(line) > 23 else ""
                    if ts_str < after_ts:
                        continue
                matches.append(line)
    return matches


async def main():
    section("Model Behavior System E2E Verification")

    token = get_shared_token()
    client = AlephClient(port=18790)

    # --- Phase 1: Connect ---
    section("Phase 1: Connection")
    try:
        result = await client.connect(token)
        check("WebSocket connected", "token" in result or "device_id" in result)
    except Exception as ex:
        check("WebSocket connected", False, str(ex))
        summary()
        sys.exit(1)

    # --- Phase 2: List providers to see what's configured ---
    section("Phase 2: Provider Discovery")
    providers_result = await client.rpc("providers.list")
    providers = providers_result.get("providers", [])
    check("Providers returned", len(providers) > 0, f"got {len(providers)}")

    enabled_providers = [p for p in providers if p.get("enabled")]
    check("At least one enabled provider", len(enabled_providers) > 0)

    for p in enabled_providers:
        protocol = p.get("protocol", "unknown")
        models = p.get("models", [])
        print(f"    Provider: {p['name']} | protocol={protocol} | models={models}")

    # --- Phase 3: Chat to trigger model behavior loading ---
    section("Phase 3: Chat + Behavior Loading Verification")

    # Record timestamp before chat
    ts_before = time.strftime("%Y-%m-%d %H:%M:%S")

    # Send a simple message that should complete quickly
    print("  Sending chat message to trigger agent loop...")
    try:
        chat_result = await client.chat("Say hello in one word.", timeout=30)
        completed = chat_result.get("completed", False)
        text = chat_result.get("text", "")
        check("Chat completed", completed, f"text={text[:100]}")
        check("Got non-empty response", len(text) > 0)
    except Exception as ex:
        check("Chat completed", False, str(ex))

    # Wait a moment for logs to flush
    await asyncio.sleep(1)

    # --- Phase 4: Log verification ---
    section("Phase 4: Log-Based Verification")

    # Check for our "Model behavior resolved" log line
    behavior_logs = grep_log("Model behavior resolved", after_ts=ts_before)
    check("Model behavior resolved log found", len(behavior_logs) > 0,
          f"found {len(behavior_logs)} entries")

    if behavior_logs:
        last_log = behavior_logs[-1]
        print(f"    Log: {last_log[:200]}")

        # Verify protocol is recorded
        has_protocol = "protocol=" in last_log or "protocol\":" in last_log
        check("Protocol recorded in log", has_protocol)

        # Verify behavior_name is recorded
        has_behavior = "behavior_name=" in last_log or "behavior_name\":" in last_log
        check("Behavior name recorded in log", has_behavior)

        # Verify loaded=true
        loaded = "loaded=true" in last_log or "loaded\":true" in last_log
        check("Behavior content loaded", loaded)

        # Identify which protocol was used
        if "anthropic" in last_log:
            print("    -> Protocol: anthropic (Claude)")
        elif "openai" in last_log:
            print("    -> Protocol: openai (GPT)")
        elif "gemini" in last_log:
            print("    -> Protocol: gemini")
        elif "ollama" in last_log:
            print("    -> Protocol: ollama")

    # --- Phase 5: Verify built-in behaviors compile correctly ---
    section("Phase 5: Built-in Behavior Content Verification")

    behaviors_dir = Path(__file__).parent.parent / "core" / "src" / "agent_loop" / "model_behaviors"
    for name in ["anthropic.md", "openai.md", "gemini.md", "ollama.md"]:
        path = behaviors_dir / name
        exists = path.exists()
        check(f"{name} exists", exists)
        if exists:
            content = path.read_text()
            check(f"{name} is non-empty", len(content) > 0, f"{len(content)} bytes")

    # Verify openai.md has execution directives
    openai_content = (behaviors_dir / "openai.md").read_text()
    check("openai.md has 'Execution Directives'",
          "Execution Directives" in openai_content)
    check("openai.md has 'ALWAYS call tools proactively'",
          "ALWAYS call tools proactively" in openai_content)

    # Verify anthropic.md is minimal
    anthropic_content = (behaviors_dir / "anthropic.md").read_text()
    check("anthropic.md is minimal (< 200 chars)",
          len(anthropic_content) < 200, f"{len(anthropic_content)} chars")

    # --- Summary ---
    ok = summary()
    await client.ws.close()
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    asyncio.run(main())
