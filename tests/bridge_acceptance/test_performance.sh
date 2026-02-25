#!/usr/bin/env bash
# Bridge Acceptance Tests — Performance Benchmarks (P1-P5)
#
# Measures latency and resource usage of the Desktop Bridge.
# Requires a running bridge.
#
# Usage: ./test_performance.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

check_deps socat jq bc python3
check_socket

echo "${BOLD}Bridge Acceptance Tests — Performance Benchmarks${RESET}"
echo "Socket: $SOCKET_PATH"

# ===================================================================
# P1: Screenshot Latency
# ===================================================================

print_header "P1: Screenshot Latency (100 iterations)"

test_p1_screenshot_latency() {
    benchmark_rpc \
        "Screenshot (fullscreen)" \
        "desktop.screenshot" \
        '{}' \
        100 \
        2000  # p95 should be under 2000ms
}

run_test "P1: Screenshot latency (p95 < 2000ms)" test_p1_screenshot_latency

# ===================================================================
# P2: OCR Latency (macOS only)
# ===================================================================

print_header "P2: OCR Latency (50 iterations) — macOS only"

test_p2_ocr_latency() {
    # First check if OCR is implemented
    local probe
    probe=$(send_rpc "desktop.ocr" '{}')
    local code
    code=$(echo "$probe" | jq -r '.error.code // "none"' 2>/dev/null)
    if [[ "$code" == "-32000" ]]; then
        echo "  OCR not implemented on this platform, testing with image_base64 param"
        # Use a tiny test image
        local white_png="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg=="
        benchmark_rpc \
            "OCR (base64 image)" \
            "desktop.ocr" \
            "{\"image_base64\":\"$white_png\"}" \
            50 \
            3000  # p95 should be under 3000ms
    else
        benchmark_rpc \
            "OCR (from screen)" \
            "desktop.ocr" \
            '{}' \
            50 \
            3000
    fi
}

if is_macos; then
    run_test "P2: OCR latency (p95 < 3000ms)" test_p2_ocr_latency
else
    skip_test "P2: OCR latency" "macOS only"
fi

# ===================================================================
# P3: Input Latency (Type Text)
# ===================================================================

print_header "P3: Input Latency (50 iterations)"

test_p3_input_latency() {
    # Test type_text latency
    # Note: actual keystrokes may or may not happen depending on implementation.
    # We are measuring the RPC round-trip time.
    benchmark_rpc \
        "Type text" \
        "desktop.type_text" \
        '{"text":"a"}' \
        50 \
        500  # p95 should be under 500ms
}

test_p3_click_latency() {
    benchmark_rpc \
        "Click" \
        "desktop.click" \
        '{"x":0,"y":0,"button":"left"}' \
        50 \
        500
}

run_test "P3a: Type text latency (p95 < 500ms)" test_p3_input_latency
run_test "P3b: Click latency (p95 < 500ms)" test_p3_click_latency

# ===================================================================
# P4: Handshake Latency (Ping)
# ===================================================================

print_header "P4: Handshake Latency (20 iterations)"

test_p4_handshake_latency() {
    benchmark_rpc \
        "Ping (handshake)" \
        "desktop.ping" \
        '{}' \
        20 \
        100  # p95 should be under 100ms for simple ping
}

run_test "P4: Ping latency (p95 < 100ms)" test_p4_handshake_latency

# ===================================================================
# P5: Memory Usage
# ===================================================================

print_header "P5: Memory Usage"

test_p5_memory_usage() {
    # Find the bridge process
    local bridge_pids
    bridge_pids=$(pgrep -f "aleph-bridge\|aleph-desktop\|tauri" 2>/dev/null || true)

    if [[ -z "$bridge_pids" ]]; then
        echo "  ${YELLOW}Could not identify bridge process. Checking for any process on the socket...${RESET}"
        # Try lsof to find who owns the socket
        bridge_pids=$(lsof -U 2>/dev/null | grep "desktop.sock" | awk '{print $2}' | sort -u | head -1 || true)
    fi

    if [[ -z "$bridge_pids" ]]; then
        echo "  ${YELLOW}WARNING: Cannot determine bridge PID for memory measurement${RESET}"
        echo "  Memory test requires the bridge process to be identifiable."
        return 0  # Non-fatal
    fi

    local total_rss=0
    local pid_count=0
    for pid in $bridge_pids; do
        local rss
        rss=$(get_process_memory_kb "$pid")
        if [[ -n "$rss" && "$rss" -gt 0 ]]; then
            local rss_mb
            rss_mb=$(echo "scale=1; $rss / 1024" | bc)
            echo "  PID $pid: ${rss_mb} MB RSS"
            total_rss=$((total_rss + rss))
            pid_count=$((pid_count + 1))
        fi
    done

    if [[ $pid_count -gt 0 ]]; then
        local total_mb
        total_mb=$(echo "scale=1; $total_rss / 1024" | bc)
        echo "  Total: ${total_mb} MB RSS across $pid_count process(es)"

        # Warn if total memory exceeds 500 MB
        local over_limit
        over_limit=$(echo "$total_rss > 512000" | bc)
        if [[ "$over_limit" -eq 1 ]]; then
            echo "  ${RED}WARNING: Memory usage exceeds 500 MB${RESET}"
            return 1
        fi
    fi

    return 0
}

run_test "P5: Memory usage (< 500 MB)" test_p5_memory_usage

# ===================================================================
# Summary
# ===================================================================

print_summary
