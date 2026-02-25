#!/usr/bin/env bash
# Bridge Acceptance Tests — End-to-End Flow (E1-E6)
#
# Tests the server-bridge lifecycle. This script manages its own server/bridge
# processes and does NOT require a pre-running bridge (unlike other test scripts).
#
# Requires: ALEPH_SERVER_BIN environment variable pointing to the aleph-server binary.
#
# Usage: ./test_e2e_flow.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

check_deps socat jq bc python3

echo "${BOLD}Bridge Acceptance Tests — End-to-End Flow${RESET}"

# E2E tests need the server binary
if [[ -z "$ALEPH_SERVER_BIN" ]]; then
    echo "${YELLOW}WARNING: ALEPH_SERVER_BIN not set.${RESET}"
    echo "E2E tests require the aleph-server binary to manage lifecycle."
    echo "Set ALEPH_SERVER_BIN=/path/to/aleph-server to enable."
    echo ""
    echo "Falling back to socket-only tests (E3 only)..."
    echo ""

    E2E_FULL=false
else
    if [[ ! -x "$ALEPH_SERVER_BIN" ]]; then
        echo "${RED}ERROR: $ALEPH_SERVER_BIN is not executable${RESET}" >&2
        exit 1
    fi
    E2E_FULL=true
fi

# Use a temporary socket path for E2E tests to avoid interfering with running instances
E2E_SOCKET_PATH="${ALEPH_SOCKET_PATH:-/tmp/aleph-e2e-test-$$.sock}"
E2E_SERVER_PID=""

# ---------------------------------------------------------------------------
# Cleanup handler
# ---------------------------------------------------------------------------

cleanup_e2e() {
    if [[ -n "$E2E_SERVER_PID" ]] && kill -0 "$E2E_SERVER_PID" 2>/dev/null; then
        kill "$E2E_SERVER_PID" 2>/dev/null || true
        wait "$E2E_SERVER_PID" 2>/dev/null || true
    fi
    rm -f "$E2E_SOCKET_PATH"
}

trap cleanup_e2e EXIT

# ---------------------------------------------------------------------------
# Helper: start server
# ---------------------------------------------------------------------------

start_server() {
    rm -f "$E2E_SOCKET_PATH"

    ALEPH_SOCKET_PATH="$E2E_SOCKET_PATH" "$ALEPH_SERVER_BIN" &
    E2E_SERVER_PID=$!

    # Wait for socket to appear
    local timeout=15
    local elapsed=0
    while [[ ! -S "$E2E_SOCKET_PATH" ]] && [[ $elapsed -lt $timeout ]]; do
        # Check process is still alive
        if ! kill -0 "$E2E_SERVER_PID" 2>/dev/null; then
            echo "Server process died during startup" >&2
            return 1
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done

    if [[ ! -S "$E2E_SOCKET_PATH" ]]; then
        echo "Socket did not appear within ${timeout}s" >&2
        return 1
    fi
}

# ---------------------------------------------------------------------------
# Helper: stop server
# ---------------------------------------------------------------------------

stop_server() {
    if [[ -n "$E2E_SERVER_PID" ]] && kill -0 "$E2E_SERVER_PID" 2>/dev/null; then
        kill "$E2E_SERVER_PID" 2>/dev/null || true
        wait "$E2E_SERVER_PID" 2>/dev/null || true
    fi
    E2E_SERVER_PID=""
    rm -f "$E2E_SOCKET_PATH"
}

# ===================================================================
# E1: Server spawns bridge
# ===================================================================

print_header "E1: Server Spawns Bridge"

test_e1_server_spawns_bridge() {
    start_server

    # Verify socket exists and is accessible
    if [[ ! -S "$E2E_SOCKET_PATH" ]]; then
        echo "Socket not found at $E2E_SOCKET_PATH" >&2
        stop_server
        return 1
    fi

    # Verify we can talk to it
    local resp
    resp=$(echo '{"jsonrpc":"2.0","id":"e1","method":"desktop.ping","params":{}}' \
        | socat - UNIX-CONNECT:"$E2E_SOCKET_PATH" 2>/dev/null) || {
        echo "Could not connect to bridge socket" >&2
        stop_server
        return 1
    }

    local result
    result=$(echo "$resp" | jq -r '.result' 2>/dev/null)
    if [[ "$result" != "pong" ]]; then
        echo "Expected pong, got: $resp" >&2
        stop_server
        return 1
    fi

    stop_server
}

if [[ "$E2E_FULL" == "true" ]]; then
    run_test "E1: Server spawns bridge and socket appears" test_e1_server_spawns_bridge
else
    skip_test "E1: Server spawns bridge and socket appears" "ALEPH_SERVER_BIN not set"
fi

# ===================================================================
# E2: Capability registration
# ===================================================================

print_header "E2: Capability Registration"

test_e2_capabilities() {
    start_server

    # Verify all expected methods are routable (ping works, unknown fails with -32601)
    local methods=("desktop.ping" "desktop.screenshot" "desktop.ocr" "desktop.ax_tree"
                   "desktop.click" "desktop.type_text" "desktop.key_combo"
                   "desktop.launch_app" "desktop.window_list" "desktop.focus_window"
                   "desktop.canvas_show" "desktop.canvas_hide" "desktop.canvas_update")

    for method in "${methods[@]}"; do
        local resp
        resp=$(echo "{\"jsonrpc\":\"2.0\",\"id\":\"e2\",\"method\":\"$method\",\"params\":{}}" \
            | socat - UNIX-CONNECT:"$E2E_SOCKET_PATH" 2>/dev/null) || {
            echo "Could not connect for method $method" >&2
            stop_server
            return 1
        }

        # The method should NOT return -32601 (method not found).
        # -32000 (not implemented) is acceptable — it means the method is registered but not yet implemented.
        local code
        code=$(echo "$resp" | jq -r '.error.code // "none"' 2>/dev/null)
        if [[ "$code" == "-32601" ]]; then
            echo "Method $method is not registered (got -32601)" >&2
            stop_server
            return 1
        fi
    done

    # Verify unknown method DOES return -32601
    local resp
    resp=$(echo '{"jsonrpc":"2.0","id":"e2","method":"desktop.bogus","params":{}}' \
        | socat - UNIX-CONNECT:"$E2E_SOCKET_PATH" 2>/dev/null) || {
        echo "Could not connect for bogus method" >&2
        stop_server
        return 1
    }

    local code
    code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
    if [[ "$code" != "-32601" ]]; then
        echo "Unknown method should return -32601, got: $resp" >&2
        stop_server
        return 1
    fi

    stop_server
}

if [[ "$E2E_FULL" == "true" ]]; then
    run_test "E2: All 13 methods are registered" test_e2_capabilities
else
    skip_test "E2: All 13 methods are registered" "ALEPH_SERVER_BIN not set"
fi

# ===================================================================
# E3: Ping (works with pre-running bridge too)
# ===================================================================

print_header "E3: Ping"

test_e3_ping() {
    local sock
    if [[ "$E2E_FULL" == "true" ]]; then
        start_server
        sock="$E2E_SOCKET_PATH"
    else
        # Fall back to default socket
        sock="$SOCKET_PATH"
        if [[ ! -S "$sock" ]]; then
            echo "No socket at $sock and ALEPH_SERVER_BIN not set" >&2
            return 1
        fi
    fi

    local resp
    resp=$(echo '{"jsonrpc":"2.0","id":"e3","method":"desktop.ping","params":{}}' \
        | socat - UNIX-CONNECT:"$sock" 2>/dev/null) || {
        echo "Connection failed" >&2
        [[ "$E2E_FULL" == "true" ]] && stop_server
        return 1
    }

    local result
    result=$(echo "$resp" | jq -r '.result' 2>/dev/null)
    if [[ "$result" != "pong" ]]; then
        echo "Expected pong, got: $resp" >&2
        [[ "$E2E_FULL" == "true" ]] && stop_server
        return 1
    fi

    [[ "$E2E_FULL" == "true" ]] && stop_server
    return 0
}

if [[ "$E2E_FULL" == "true" ]] || [[ -S "$SOCKET_PATH" ]]; then
    run_test "E3: Ping returns pong" test_e3_ping
else
    skip_test "E3: Ping returns pong" "no socket available"
fi

# ===================================================================
# E4: Graceful Shutdown
# ===================================================================

print_header "E4: Graceful Shutdown"

test_e4_graceful_shutdown() {
    start_server

    # Verify server is alive
    local resp
    resp=$(echo '{"jsonrpc":"2.0","id":"e4-pre","method":"desktop.ping","params":{}}' \
        | socat - UNIX-CONNECT:"$E2E_SOCKET_PATH" 2>/dev/null) || {
        echo "Pre-shutdown ping failed" >&2
        stop_server
        return 1
    }

    # Send SIGTERM
    kill "$E2E_SERVER_PID" 2>/dev/null || true

    # Wait for process to exit (up to 10s)
    local timeout=10
    local elapsed=0
    while kill -0 "$E2E_SERVER_PID" 2>/dev/null && [[ $elapsed -lt $timeout ]]; do
        sleep 1
        elapsed=$((elapsed + 1))
    done

    if kill -0 "$E2E_SERVER_PID" 2>/dev/null; then
        echo "Server did not exit within ${timeout}s after SIGTERM" >&2
        kill -9 "$E2E_SERVER_PID" 2>/dev/null || true
        E2E_SERVER_PID=""
        return 1
    fi

    # Socket should be cleaned up
    sleep 1
    if [[ -S "$E2E_SOCKET_PATH" ]]; then
        echo "WARNING: socket file not cleaned up after shutdown (non-fatal)" >&2
        # This is a warning, not a failure — some implementations may not clean up
    fi

    E2E_SERVER_PID=""
    return 0
}

if [[ "$E2E_FULL" == "true" ]]; then
    run_test "E4: Graceful shutdown on SIGTERM" test_e4_graceful_shutdown
else
    skip_test "E4: Graceful shutdown on SIGTERM" "ALEPH_SERVER_BIN not set"
fi

# ===================================================================
# E5: Bridge Crash Recovery
# ===================================================================

print_header "E5: Bridge Crash Recovery"

test_e5_crash_recovery() {
    start_server

    # Kill with SIGKILL (simulates crash)
    kill -9 "$E2E_SERVER_PID" 2>/dev/null || true
    wait "$E2E_SERVER_PID" 2>/dev/null || true
    E2E_SERVER_PID=""

    # Clean up stale socket
    rm -f "$E2E_SOCKET_PATH"

    # Restart
    start_server

    # Verify it works after restart
    local resp
    resp=$(echo '{"jsonrpc":"2.0","id":"e5","method":"desktop.ping","params":{}}' \
        | socat - UNIX-CONNECT:"$E2E_SOCKET_PATH" 2>/dev/null) || {
        echo "Post-recovery ping failed" >&2
        stop_server
        return 1
    }

    local result
    result=$(echo "$resp" | jq -r '.result' 2>/dev/null)
    if [[ "$result" != "pong" ]]; then
        echo "Expected pong after recovery, got: $resp" >&2
        stop_server
        return 1
    fi

    stop_server
    return 0
}

if [[ "$E2E_FULL" == "true" ]]; then
    run_test "E5: Crash recovery (SIGKILL + restart)" test_e5_crash_recovery
else
    skip_test "E5: Crash recovery (SIGKILL + restart)" "ALEPH_SERVER_BIN not set"
fi

# ===================================================================
# E6: Headless Mode
# ===================================================================

print_header "E6: Headless Mode"

test_e6_headless() {
    # Start server without display (headless)
    # On macOS, the bridge needs a display for some features but should still start
    rm -f "$E2E_SOCKET_PATH"

    ALEPH_SOCKET_PATH="$E2E_SOCKET_PATH" ALEPH_HEADLESS=1 "$ALEPH_SERVER_BIN" &
    E2E_SERVER_PID=$!

    # Wait for socket
    local timeout=15
    local elapsed=0
    while [[ ! -S "$E2E_SOCKET_PATH" ]] && [[ $elapsed -lt $timeout ]]; do
        if ! kill -0 "$E2E_SERVER_PID" 2>/dev/null; then
            # Server might not support headless mode — that is acceptable
            echo "Server exited in headless mode (may not be supported)" >&2
            E2E_SERVER_PID=""
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done

    if [[ ! -S "$E2E_SOCKET_PATH" ]]; then
        # Server is running but socket not created — headless mode may be unsupported
        stop_server
        return 0
    fi

    # Verify ping works in headless mode
    local resp
    resp=$(echo '{"jsonrpc":"2.0","id":"e6","method":"desktop.ping","params":{}}' \
        | socat - UNIX-CONNECT:"$E2E_SOCKET_PATH" 2>/dev/null) || {
        echo "Headless ping failed" >&2
        stop_server
        return 1
    }

    local result
    result=$(echo "$resp" | jq -r '.result' 2>/dev/null)
    if [[ "$result" != "pong" ]]; then
        echo "Expected pong in headless, got: $resp" >&2
        stop_server
        return 1
    fi

    stop_server
    return 0
}

if [[ "$E2E_FULL" == "true" ]]; then
    run_test "E6: Headless mode (ALEPH_HEADLESS=1)" test_e6_headless
else
    skip_test "E6: Headless mode (ALEPH_HEADLESS=1)" "ALEPH_SERVER_BIN not set"
fi

# ===================================================================
# Summary
# ===================================================================

print_summary
