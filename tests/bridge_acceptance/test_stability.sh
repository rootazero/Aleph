#!/usr/bin/env bash
# Bridge Acceptance Tests — Stability (S1-S4)
#
# Long-running stability and stress tests. Durations are configurable via
# environment variables.
#
# Usage: ./test_stability.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

# ---------------------------------------------------------------------------
# Configuration (all durations configurable)
# ---------------------------------------------------------------------------

STABILITY_LONG_RUN_SECS="${STABILITY_LONG_RUN_SECS:-300}"
STABILITY_HOTKEY_CYCLES="${STABILITY_HOTKEY_CYCLES:-100}"
STABILITY_UDS_CYCLES="${STABILITY_UDS_CYCLES:-10}"
STABILITY_STRESS_DURATION_SECS="${STABILITY_STRESS_DURATION_SECS:-300}"
STABILITY_STRESS_RPS="${STABILITY_STRESS_RPS:-10}"

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

check_deps socat jq bc python3
check_socket

echo "${BOLD}Bridge Acceptance Tests — Stability${RESET}"
echo "Socket: $SOCKET_PATH"
echo ""
echo "Configuration:"
echo "  S1 long run:       ${STABILITY_LONG_RUN_SECS}s"
echo "  S2 hotkey cycles:  ${STABILITY_HOTKEY_CYCLES}"
echo "  S3 UDS cycles:     ${STABILITY_UDS_CYCLES}"
echo "  S4 stress:         ${STABILITY_STRESS_RPS} req/s for ${STABILITY_STRESS_DURATION_SECS}s"

# ===================================================================
# S1: Long-Running Memory Leak Check
# ===================================================================

print_header "S1: Memory Leak Check (${STABILITY_LONG_RUN_SECS}s)"

test_s1_memory_leak() {
    # Find the bridge process
    local bridge_pid
    bridge_pid=$(lsof -U 2>/dev/null | grep "desktop.sock" | awk '{print $2}' | sort -u | head -1 || true)

    if [[ -z "$bridge_pid" ]]; then
        bridge_pid=$(pgrep -f "aleph-bridge\|aleph-desktop\|tauri" 2>/dev/null | head -1 || true)
    fi

    if [[ -z "$bridge_pid" ]]; then
        echo "  ${YELLOW}Cannot identify bridge PID. Skipping memory leak check.${RESET}"
        return 0
    fi

    # Record initial memory
    local initial_rss
    initial_rss=$(get_process_memory_kb "$bridge_pid")
    if [[ -z "$initial_rss" || "$initial_rss" -eq 0 ]]; then
        echo "  ${YELLOW}Cannot read memory for PID $bridge_pid${RESET}"
        return 0
    fi

    local initial_mb
    initial_mb=$(echo "scale=1; $initial_rss / 1024" | bc)
    echo "  Initial RSS: ${initial_mb} MB (PID $bridge_pid)"

    # Send periodic pings for the configured duration
    local interval=5
    local elapsed=0
    local max_rss=$initial_rss
    local samples=0

    while [[ $elapsed -lt $STABILITY_LONG_RUN_SECS ]]; do
        # Send a burst of pings
        for (( i=0; i<5; i++ )); do
            send_rpc "desktop.ping" '{}' "s1-$elapsed-$i" >/dev/null 2>&1 || true
        done

        sleep "$interval"
        elapsed=$((elapsed + interval))
        samples=$((samples + 1))

        # Sample memory
        local current_rss
        current_rss=$(get_process_memory_kb "$bridge_pid" 2>/dev/null || echo "0")
        if [[ "$current_rss" -gt "$max_rss" ]]; then
            max_rss=$current_rss
        fi

        # Progress every 60 seconds
        if (( elapsed % 60 == 0 )); then
            local current_mb
            current_mb=$(echo "scale=1; $current_rss / 1024" | bc)
            echo "  [${elapsed}s] RSS: ${current_mb} MB"
        fi
    done

    # Check final memory
    local final_rss
    final_rss=$(get_process_memory_kb "$bridge_pid" 2>/dev/null || echo "0")
    local final_mb max_mb
    final_mb=$(echo "scale=1; $final_rss / 1024" | bc)
    max_mb=$(echo "scale=1; $max_rss / 1024" | bc)

    echo "  Final RSS:   ${final_mb} MB"
    echo "  Peak RSS:    ${max_mb} MB"

    # Check for significant memory growth (> 50% increase from initial)
    if [[ "$initial_rss" -gt 0 ]]; then
        local growth_pct
        growth_pct=$(echo "scale=0; ($final_rss - $initial_rss) * 100 / $initial_rss" | bc)
        echo "  Growth:      ${growth_pct}%"

        if [[ "$growth_pct" -gt 50 ]]; then
            echo "  ${RED}WARNING: Memory grew by ${growth_pct}% (threshold: 50%)${RESET}"
            return 1
        fi
    fi

    return 0
}

run_test "S1: Memory leak check (${STABILITY_LONG_RUN_SECS}s)" test_s1_memory_leak

# ===================================================================
# S2: Hotkey Show/Hide Cycles
# ===================================================================

print_header "S2: Hotkey Show/Hide Cycles (${STABILITY_HOTKEY_CYCLES} cycles)"

test_s2_hotkey_cycles() {
    # We cannot directly trigger Tauri hotkeys from the shell, but we can
    # simulate the effect by sending rapid canvas_show/canvas_hide pairs
    # (which exercises the same window lifecycle code).
    local failures=0

    for (( i=1; i<=STABILITY_HOTKEY_CYCLES; i++ )); do
        # Show
        local resp
        resp=$(send_rpc "desktop.canvas_show" '{"html":"<p>cycle</p>","position":{"x":100,"y":100,"width":200,"height":100}}' "s2-show-$i" 2>/dev/null) || {
            failures=$((failures + 1))
            continue
        }

        # Brief pause
        if (( i % 10 == 0 )); then
            sleep 0.1
        fi

        # Hide
        resp=$(send_rpc "desktop.canvas_hide" '{}' "s2-hide-$i" 2>/dev/null) || {
            failures=$((failures + 1))
            continue
        }

        # Progress
        if (( i % 25 == 0 )); then
            echo "  [${i}/${STABILITY_HOTKEY_CYCLES}] completed"
        fi
    done

    echo "  Completed: $((STABILITY_HOTKEY_CYCLES - failures))/${STABILITY_HOTKEY_CYCLES} cycles"

    # Allow up to 5% failure rate
    local max_failures
    max_failures=$(( STABILITY_HOTKEY_CYCLES * 5 / 100 ))
    if [[ $failures -gt $max_failures ]]; then
        echo "  ${RED}Too many failures: $failures (max: $max_failures)${RESET}"
        return 1
    fi

    return 0
}

run_test "S2: Show/hide cycles (${STABILITY_HOTKEY_CYCLES}x)" test_s2_hotkey_cycles

# ===================================================================
# S3: UDS Connection Cycling
# ===================================================================

print_header "S3: UDS Connection Cycling (${STABILITY_UDS_CYCLES} cycles)"

test_s3_uds_cycling() {
    local failures=0

    for (( i=1; i<=STABILITY_UDS_CYCLES; i++ )); do
        # Open connection, send ping, close
        local resp
        resp=$(send_rpc "desktop.ping" '{}' "s3-$i") || {
            failures=$((failures + 1))
            echo "  ${YELLOW}Cycle $i: connection failed${RESET}"
            sleep 1  # Brief recovery pause
            continue
        }

        local result
        result=$(echo "$resp" | jq -r '.result' 2>/dev/null)
        if [[ "$result" != "pong" ]]; then
            failures=$((failures + 1))
            echo "  ${YELLOW}Cycle $i: bad response${RESET}"
        fi
    done

    echo "  Completed: $((STABILITY_UDS_CYCLES - failures))/${STABILITY_UDS_CYCLES} cycles"

    if [[ $failures -gt 0 ]]; then
        echo "  ${RED}$failures connection failures${RESET}"
        return 1
    fi

    return 0
}

run_test "S3: UDS connection cycling (${STABILITY_UDS_CYCLES}x)" test_s3_uds_cycling

# ===================================================================
# S4: High-Frequency RPC Stress
# ===================================================================

print_header "S4: High-Frequency RPC Stress (${STABILITY_STRESS_RPS} req/s for ${STABILITY_STRESS_DURATION_SECS}s)"

test_s4_stress() {
    local total_requests=0
    local total_failures=0
    local interval_ms
    interval_ms=$(echo "scale=3; 1000 / $STABILITY_STRESS_RPS" | bc)
    local interval_s
    interval_s=$(echo "scale=3; 1 / $STABILITY_STRESS_RPS" | bc)

    local start_time
    start_time=$(python3 -c "import time; print(int(time.time()))")
    local end_time=$((start_time + STABILITY_STRESS_DURATION_SECS))

    echo "  Target: ${STABILITY_STRESS_RPS} requests/second"
    echo "  Duration: ${STABILITY_STRESS_DURATION_SECS}s"
    echo "  Expected total: $(( STABILITY_STRESS_RPS * STABILITY_STRESS_DURATION_SECS )) requests"

    local last_report=$start_time
    local period_requests=0
    local period_failures=0

    while true; do
        local now
        now=$(python3 -c "import time; print(int(time.time()))")
        if [[ $now -ge $end_time ]]; then
            break
        fi

        # Send a ping
        local resp
        resp=$(send_rpc "desktop.ping" '{}' "s4-$total_requests" 2>/dev/null) || {
            total_failures=$((total_failures + 1))
            period_failures=$((period_failures + 1))
        }
        total_requests=$((total_requests + 1))
        period_requests=$((period_requests + 1))

        # Report every 30 seconds
        if [[ $((now - last_report)) -ge 30 ]]; then
            local elapsed=$((now - start_time))
            local actual_rps
            if [[ $elapsed -gt 0 ]]; then
                actual_rps=$(echo "scale=1; $total_requests / $elapsed" | bc)
            else
                actual_rps="N/A"
            fi
            echo "  [${elapsed}s] total=$total_requests failures=$total_failures actual_rps=$actual_rps"
            last_report=$now
            period_requests=0
            period_failures=0
        fi

        # Throttle to target RPS
        # Using python3 for sub-second sleep
        python3 -c "import time; time.sleep($interval_s)" 2>/dev/null || sleep 1
    done

    local duration
    duration=$(( $(python3 -c "import time; print(int(time.time()))") - start_time ))
    local actual_rps
    if [[ $duration -gt 0 ]]; then
        actual_rps=$(echo "scale=1; $total_requests / $duration" | bc)
    else
        actual_rps="N/A"
    fi

    echo ""
    echo "  Results:"
    echo "    Total requests: $total_requests"
    echo "    Total failures: $total_failures"
    echo "    Duration:       ${duration}s"
    echo "    Actual RPS:     $actual_rps"

    # Failure threshold: < 1%
    if [[ $total_requests -gt 0 ]]; then
        local failure_pct
        failure_pct=$(echo "scale=2; $total_failures * 100 / $total_requests" | bc)
        echo "    Failure rate:   ${failure_pct}%"

        local over_threshold
        over_threshold=$(echo "$failure_pct > 1" | bc -l)
        if [[ "$over_threshold" -eq 1 ]]; then
            echo "  ${RED}Failure rate ${failure_pct}% exceeds 1% threshold${RESET}"
            return 1
        fi
    fi

    return 0
}

run_test "S4: High-frequency stress (${STABILITY_STRESS_RPS} rps, ${STABILITY_STRESS_DURATION_SECS}s)" test_s4_stress

# ===================================================================
# Summary
# ===================================================================

print_summary
