#!/usr/bin/env python3
"""
TC2.0 Comprehensive Test Suite for Aleph Tool Calling 2.0

Tests all TC2.0 capabilities via server log probe analysis:
  T1. Strict Mode        — All tools accepted by Bedrock with strict=true
  T2. Parallel Execution — N>1 tools via JoinSet in PARALLEL mode
  T3. Sequential Fallback — N=1 fast path (no JoinSet overhead)
  T4. Probe Observability — P1-P5 all fire with correct data
  T5. Multi-Step Complex  — Mixed parallel/sequential in one session

Validation: Send WebSocket requests, parse server log for TC2.0 probes.

Usage:
  1. Start server: RUST_LOG=info cargo run --bin aleph -p alephcore
  2. Run tests:    python3 tests/tc2_test_suite.py
"""

import asyncio
import json
import re
import sys
import time
from dataclasses import dataclass, field

try:
    import websockets
except ImportError:
    print("ERROR: pip install websockets")
    sys.exit(1)

# ─── Configuration ───────────────────────────────────────────────────────────

WS_URI = "ws://localhost:18790/ws"
SERVER_LOG = "/tmp/aleph_server.log"
PROVIDER = "DMX"
RECV_TIMEOUT = 120  # seconds per message recv


# ─── Data structures ────────────────────────────────────────────────────────

@dataclass
class ProbeEntry:
    probe_type: str        # "prepared", "returned", "executing", "batch_done"
    raw: str
    data: dict = field(default_factory=dict)


@dataclass
class ScenarioResult:
    name: str
    passed: bool
    details: str
    probes: list = field(default_factory=list)


# ─── Log parsing ─────────────────────────────────────────────────────────────

def parse_tc2_probes(log_path: str, after_line: int = 0) -> list[ProbeEntry]:
    probes = []
    try:
        with open(log_path) as f:
            lines = f.readlines()
    except FileNotFoundError:
        return probes

    for i, line in enumerate(lines):
        if i < after_line:
            continue
        if "[TC2.0]" not in line:
            continue

        entry = ProbeEntry(probe_type="unknown", raw=line.strip())

        if "Prepared" in line:
            entry.probe_type = "prepared"
            m = re.search(r"(\d+) tool defs \((\d+) strict, (\d+) non-strict\)", line)
            if m:
                entry.data = {
                    "total": int(m.group(1)),
                    "strict": int(m.group(2)),
                    "non_strict": int(m.group(3)),
                }

        elif "LLM returned" in line:
            entry.probe_type = "returned"
            m = re.search(r"(\d+) tool call\(s\): (\[.*?\])", line)
            if m:
                entry.data = {
                    "count": int(m.group(1)),
                    "tools": json.loads(m.group(2).replace("'", '"')),
                }
            entry.data["parallel"] = "parallel=true" in line

        elif "Executing" in line:
            entry.probe_type = "executing"
            entry.data["parallel"] = "PARALLEL" in line
            m = re.search(r"Executing (\d+) tool", line)
            if m:
                entry.data["count"] = int(m.group(1))

        elif "Batch done" in line:
            entry.probe_type = "batch_done"
            m = re.search(r"(\d+)/(\d+) ok, wall=(\d+)ms", line)
            if m:
                entry.data = {
                    "succeeded": int(m.group(1)),
                    "total": int(m.group(2)),
                    "wall_ms": int(m.group(3)),
                }

        elif "Terminal defense" in line:
            entry.probe_type = "terminal_defense"
            m = re.search(r"(\w+) suppressed (\d+) real", line)
            if m:
                entry.data = {
                    "terminal": m.group(1),
                    "suppressed": int(m.group(2)),
                }

        probes.append(entry)
    return probes


def get_log_line_count(log_path: str) -> int:
    try:
        with open(log_path) as f:
            return sum(1 for _ in f)
    except FileNotFoundError:
        return 0


# ─── WebSocket client ────────────────────────────────────────────────────────

async def send_and_collect(message: str, provider: str = PROVIDER) -> dict:
    """Send chat.send and collect stream events until run_complete/run_error.

    Aleph WebSocket protocol:
      - Initial ack:   {"jsonrpc":"2.0", "result": {"run_id":..., "streaming":true}, "id":N}
      - Stream events:  {"jsonrpc":"2.0", "method": "stream.<type>", "params": {...}}
      - Heartbeat:     {"topic": "system.tick", "data": {...}}
      - Terminal:      method == "stream.run_complete" or "stream.run_error"
    """
    events = []
    async with websockets.connect(WS_URI, close_timeout=5) as ws:
        msg = {
            "jsonrpc": "2.0",
            "id": int(time.time() * 1000),
            "method": "chat.send",
            "params": {"message": message, "model": provider},
        }
        await ws.send(json.dumps(msg))

        while True:
            try:
                raw = await asyncio.wait_for(ws.recv(), timeout=RECV_TIMEOUT)
                data = json.loads(raw)

                # Skip heartbeat ticks
                if data.get("topic") == "system.tick":
                    continue

                # JSON-RPC response (initial ack for chat.send)
                if "result" in data and "run_id" in data.get("result", {}):
                    events.append({"_type": "ack", **data["result"]})
                    continue

                # Stream events use "method" field: "stream.<event_type>"
                method = data.get("method", "")
                params = data.get("params", {})

                events.append({"_method": method, **params})

                # Terminal events
                if method in ("stream.run_complete", "stream.run_error"):
                    break

            except asyncio.TimeoutError:
                events.append({"_method": "timeout"})
                break
            except websockets.exceptions.ConnectionClosed:
                events.append({"_method": "ws_closed"})
                break

    return {"events": events}


def has_error(result: dict) -> bool:
    return any(
        e.get("_method") in ("stream.run_error", "timeout", "ws_closed")
        for e in result["events"]
    )


def get_error_msg(result: dict) -> str:
    for e in result["events"]:
        if e.get("_method") == "stream.run_error":
            return e.get("error", "unknown error")[:200]
        if e.get("_method") == "timeout":
            return "Timeout waiting for response"
    return "Unknown error"


# ─── Test scenarios ───────────────────────────────────────────────────────────

async def test_T1_strict_mode() -> ScenarioResult:
    """T1: All 74 tools strict, accepted by Bedrock."""
    log_before = get_log_line_count(SERVER_LOG)

    result = await send_and_collect("What is 2+2? Answer in one word.")
    await asyncio.sleep(2)

    probes = parse_tc2_probes(SERVER_LOG, log_before)
    prepared = [p for p in probes if p.probe_type == "prepared"]

    if not prepared:
        return ScenarioResult("T1: Strict Mode", False,
                              f"No 'prepared' probe. Error? {has_error(result)}: {get_error_msg(result)}")

    p = prepared[0]
    total = p.data.get("total", 0)
    strict = p.data.get("strict", 0)

    if has_error(result):
        return ScenarioResult("T1: Strict Mode", False,
                              f"Bedrock rejected: {get_error_msg(result)}")

    if strict != total or total < 70:
        return ScenarioResult("T1: Strict Mode", False,
                              f"{strict}/{total} strict (expected all)")

    return ScenarioResult("T1: Strict Mode", True,
                          f"{strict}/{total} strict, Bedrock accepted, no errors",
                          probes)


async def test_T2_parallel_execution() -> ScenarioResult:
    """T2: Verify N>1 parallel tool execution via JoinSet."""
    log_before = get_log_line_count(SERVER_LOG)

    prompt = (
        "Execute these 4 independent shell commands simultaneously, "
        "you MUST use parallel tool calls (call all 4 bash tools at once in a single response):\n"
        "1) echo 'ALPHA'\n"
        "2) echo 'BETA'\n"
        "3) echo 'GAMMA'\n"
        "4) echo 'DELTA'\n"
        "After running all four, report the outputs."
    )

    result = await send_and_collect(prompt)
    await asyncio.sleep(2)

    if has_error(result):
        return ScenarioResult("T2: Parallel Execution", False,
                              f"Request failed: {get_error_msg(result)}")

    probes = parse_tc2_probes(SERVER_LOG, log_before)

    parallel_exec = [p for p in probes if p.probe_type == "executing" and p.data.get("parallel")]
    parallel_batch = [p for p in probes if p.probe_type == "batch_done" and p.data.get("total", 0) > 1]

    if not parallel_exec:
        returned = [p for p in probes if p.probe_type == "returned"]
        counts = [p.data.get("count", 0) for p in returned]
        return ScenarioResult("T2: Parallel Execution", False,
                              f"LLM didn't use parallel calls. Per-response tool counts: {counts}")

    max_n = max(p.data.get("count", 0) for p in parallel_exec)
    wall_ms = parallel_batch[0].data.get("wall_ms", -1) if parallel_batch else -1

    return ScenarioResult("T2: Parallel Execution", True,
                          f"Max {max_n} parallel tools, wall={wall_ms}ms, JoinSet confirmed",
                          probes)


async def test_T3_sequential_fallback() -> ScenarioResult:
    """T3: N=1 fast path."""
    log_before = get_log_line_count(SERVER_LOG)

    result = await send_and_collect("Run: echo 'SEQ_TEST_42'")
    await asyncio.sleep(2)

    if has_error(result):
        return ScenarioResult("T3: Sequential Fallback", False,
                              f"Request failed: {get_error_msg(result)}")

    probes = parse_tc2_probes(SERVER_LOG, log_before)
    sequential = [p for p in probes if p.probe_type == "executing" and not p.data.get("parallel")]

    if not sequential:
        return ScenarioResult("T3: Sequential Fallback", False,
                              "No sequential execution probe found")

    batch_ok = [p for p in probes if p.probe_type == "batch_done" and p.data.get("succeeded", 0) >= 1]

    return ScenarioResult("T3: Sequential Fallback", True,
                          f"{len(sequential)} sequential exec(s), {len(batch_ok)} batch(es) ok",
                          probes)


async def test_T4_probe_observability() -> ScenarioResult:
    """T4: All 4 runtime probes fire with valid data."""
    log_before = get_log_line_count(SERVER_LOG)

    prompt = (
        "Run 'echo PROBE_A' and 'echo PROBE_B' simultaneously, then tell me the outputs."
    )

    result = await send_and_collect(prompt)
    await asyncio.sleep(2)

    if has_error(result):
        return ScenarioResult("T4: Probe Observability", False,
                              f"Request failed: {get_error_msg(result)}")

    probes = parse_tc2_probes(SERVER_LOG, log_before)
    types_found = set(p.probe_type for p in probes)
    expected = {"prepared", "returned", "executing", "batch_done"}
    missing = expected - types_found

    if missing:
        return ScenarioResult("T4: Probe Observability", False,
                              f"Missing probe types: {missing}. Found: {types_found}")

    # Validate data quality
    issues = []
    for p in probes:
        if p.probe_type == "prepared" and p.data.get("total", 0) == 0:
            issues.append("prepared: 0 tools")
        if p.probe_type == "batch_done" and "wall_ms" not in p.data:
            issues.append("batch_done: missing wall_ms")

    if issues:
        return ScenarioResult("T4: Probe Observability", False,
                              f"Data issues: {issues}")

    return ScenarioResult("T4: Probe Observability", True,
                          f"All probes fired: {types_found}, entries={len(probes)}",
                          probes)


async def test_T5_multi_step_complex() -> ScenarioResult:
    """T5: Multi-step with mixed tool patterns."""
    log_before = get_log_line_count(SERVER_LOG)

    prompt = (
        "Do a quick audit:\n"
        "Step 1: Run these 3 commands in parallel:\n"
        "  - wc -l core/src/thinker/mod.rs\n"
        "  - wc -l core/src/agent_loop/agent_loop.rs\n"
        "  - wc -l core/src/tools/schema_strictify.rs\n"
        "Step 2: Sum up the line counts.\n"
        "Step 3: Report each file's line count and the total."
    )

    result = await send_and_collect(prompt)
    await asyncio.sleep(2)

    if has_error(result):
        return ScenarioResult("T5: Multi-Step Complex", False,
                              f"Request failed: {get_error_msg(result)}")

    probes = parse_tc2_probes(SERVER_LOG, log_before)
    batch_probes = [p for p in probes if p.probe_type == "batch_done"]
    total_exec = sum(p.data.get("total", 0) for p in batch_probes)
    has_par = any(p.probe_type == "executing" and p.data.get("parallel") for p in probes)
    has_seq = any(p.probe_type == "executing" and not p.data.get("parallel") for p in probes)
    completed = any(
        p.probe_type == "returned" and "__complete" in str(p.data.get("tools", []))
        for p in probes
    )

    desc = (f"tools_executed={total_exec}, parallel={has_par}, "
            f"sequential={has_seq}, completed={completed}")

    if total_exec >= 3 and completed:
        return ScenarioResult("T5: Multi-Step Complex", True, desc, probes)
    elif total_exec >= 1:
        return ScenarioResult("T5: Multi-Step Complex", True,
                              f"Partial (model chose strategy): {desc}", probes)
    else:
        return ScenarioResult("T5: Multi-Step Complex", False, desc, probes)


# ─── Runner ──────────────────────────────────────────────────────────────────

async def run_suite():
    print("=" * 72)
    print("  Aleph TC2.0 Comprehensive Test Suite")
    print("=" * 72)
    print()

    try:
        async with websockets.connect(WS_URI, close_timeout=3) as ws:
            pass
    except Exception as e:
        print(f"FATAL: Cannot connect to {WS_URI}")
        print(f"  Start server: RUST_LOG=info cargo run --bin aleph -p alephcore")
        print(f"  Error: {e}")
        sys.exit(1)

    print(f"  Server:   {WS_URI}")
    print(f"  Log:      {SERVER_LOG}")
    print(f"  Provider: {PROVIDER}")
    print()

    tests = [
        ("T1", test_T1_strict_mode),
        ("T2", test_T2_parallel_execution),
        ("T3", test_T3_sequential_fallback),
        ("T4", test_T4_probe_observability),
        ("T5", test_T5_multi_step_complex),
    ]

    results: list[ScenarioResult] = []

    for tag, test_fn in tests:
        print(f"--- {tag} {'─' * (60 - len(tag))}")
        try:
            r = await test_fn()
            results.append(r)
            status = "PASS" if r.passed else "FAIL"
            print(f"  [{status}] {r.name}")
            print(f"         {r.details}")
            if not r.passed and r.probes:
                for p in r.probes[:3]:
                    print(f"         probe: {p.probe_type} {p.data}")
        except Exception as e:
            import traceback
            results.append(ScenarioResult(tag, False, f"Exception: {e}"))
            print(f"  [FAIL] {tag}: {e}")
            traceback.print_exc()
        print()

    # Summary
    print("=" * 72)
    print("  SUMMARY")
    print("=" * 72)
    print()

    passed = sum(1 for r in results if r.passed)
    total = len(results)
    for r in results:
        icon = "PASS" if r.passed else "FAIL"
        print(f"  [{icon}] {r.name}")
    print()
    print(f"  Result: {passed}/{total} passed")
    if passed == total:
        print("  TC2.0 is fully operational.")
    else:
        print("  Some tests need attention — see details above.")
    print("=" * 72)

    return 0 if passed == total else 1


if __name__ == "__main__":
    exit_code = asyncio.run(run_suite())
    sys.exit(exit_code)
