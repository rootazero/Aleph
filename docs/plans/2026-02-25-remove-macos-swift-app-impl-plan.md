# Remove macOS Swift App — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Verify Tauri Bridge functional parity via acceptance tests, then safely remove the deprecated Swift macOS app and all associated artifacts.

**Architecture:** Bash test scripts communicate with the Tauri Bridge via UDS JSON-RPC 2.0 (`socat`). Tests are organized into 4 suites: functional parity, end-to-end flow, performance benchmarks, and stability. After all tests pass, cleanup proceeds in 5 phases (A-E) with strict dependency ordering.

**Tech Stack:** Bash (test scripts), socat (UDS), jq (JSON), the running `aleph-server` + `aleph-tauri` bridge

**Design Doc:** `docs/plans/2026-02-25-remove-macos-swift-app-design.md`

---

## Task 1: Create Test Infrastructure

**Files:**
- Create: `tests/bridge_acceptance/lib.sh`
- Create: `tests/bridge_acceptance/README.md`

**Step 1: Create the test directory**

Run: `mkdir -p tests/bridge_acceptance`

**Step 2: Write the shared test library**

Create `tests/bridge_acceptance/lib.sh`:

```bash
#!/usr/bin/env bash
# Shared helpers for bridge acceptance tests.
# Source this file: source "$(dirname "$0")/lib.sh"

set -euo pipefail

# --- Configuration ---
SOCKET_PATH="${ALEPH_SOCKET_PATH:-$HOME/.aleph/bridge.sock}"
PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
RESULTS=()

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# --- Core helpers ---

send_rpc() {
  local payload="$1"
  local timeout="${2:-5}"
  echo "$payload" | socat -t"$timeout" - UNIX-CONNECT:"$SOCKET_PATH" 2>/dev/null
}

assert_json_has() {
  local json="$1"
  local path="$2"
  local value
  value=$(echo "$json" | jq -r "$path" 2>/dev/null)
  if [[ -z "$value" || "$value" == "null" ]]; then
    echo "FAIL: expected $path to exist in response"
    return 1
  fi
  return 0
}

assert_json_eq() {
  local json="$1"
  local path="$2"
  local expected="$3"
  local actual
  actual=$(echo "$json" | jq -r "$path" 2>/dev/null)
  if [[ "$actual" != "$expected" ]]; then
    echo "FAIL: $path = '$actual', expected '$expected'"
    return 1
  fi
  return 0
}

assert_json_gt() {
  local json="$1"
  local path="$2"
  local threshold="$3"
  local actual
  actual=$(echo "$json" | jq -r "$path" 2>/dev/null)
  if [[ -z "$actual" || "$actual" == "null" ]]; then
    echo "FAIL: $path is null/missing"
    return 1
  fi
  if (( $(echo "$actual <= $threshold" | bc -l) )); then
    echo "FAIL: $path = $actual, expected > $threshold"
    return 1
  fi
  return 0
}

check_deps() {
  for cmd in socat jq bc; do
    if ! command -v "$cmd" &>/dev/null; then
      echo "ERROR: '$cmd' is required but not found. Install it first."
      exit 1
    fi
  done
}

check_socket() {
  if [[ ! -S "$SOCKET_PATH" ]]; then
    echo "ERROR: Bridge socket not found at $SOCKET_PATH"
    echo "Make sure aleph-server and aleph-tauri bridge are running."
    exit 1
  fi
}

# --- Test runner ---

run_test() {
  local name="$1"
  local func="$2"
  printf "  %-50s " "$name"
  local output
  if output=$($func 2>&1); then
    printf "${GREEN}PASS${NC}\n"
    PASS_COUNT=$((PASS_COUNT + 1))
    RESULTS+=("PASS|$name")
  else
    printf "${RED}FAIL${NC}\n"
    if [[ -n "$output" ]]; then
      echo "    $output"
    fi
    FAIL_COUNT=$((FAIL_COUNT + 1))
    RESULTS+=("FAIL|$name")
  fi
}

skip_test() {
  local name="$1"
  local reason="$2"
  printf "  %-50s ${YELLOW}SKIP${NC} (%s)\n" "$name" "$reason"
  SKIP_COUNT=$((SKIP_COUNT + 1))
  RESULTS+=("SKIP|$name")
}

print_summary() {
  local suite="$1"
  echo ""
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  printf "  ${CYAN}%s Summary${NC}\n" "$suite"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  printf "  ${GREEN}PASS: %d${NC}  ${RED}FAIL: %d${NC}  ${YELLOW}SKIP: %d${NC}\n" \
    "$PASS_COUNT" "$FAIL_COUNT" "$SKIP_COUNT"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

  if [[ "$FAIL_COUNT" -gt 0 ]]; then
    echo ""
    echo "Failed tests:"
    for r in "${RESULTS[@]}"; do
      if [[ "$r" == FAIL* ]]; then
        echo "  - ${r#FAIL|}"
      fi
    done
    return 1
  fi
  return 0
}

# --- Benchmark helper ---

benchmark_rpc() {
  local method="$1"
  local params="${2:-{}}"
  local iterations="${3:-10}"
  local timings=()

  for i in $(seq 1 "$iterations"); do
    local start end elapsed
    start=$(python3 -c 'import time; print(time.time())')
    send_rpc "{\"jsonrpc\":\"2.0\",\"id\":\"bench-$i\",\"method\":\"$method\",\"params\":$params}" >/dev/null
    end=$(python3 -c 'import time; print(time.time())')
    elapsed=$(echo "$end - $start" | bc -l)
    timings+=("$elapsed")
  done

  # Sort and compute stats
  local sorted
  sorted=$(printf '%s\n' "${timings[@]}" | sort -n)
  local count=${#timings[@]}
  local median_idx=$(( count / 2 ))
  local p95_idx=$(( count * 95 / 100 ))

  local min median p95 max
  min=$(echo "$sorted" | head -1)
  median=$(echo "$sorted" | sed -n "$((median_idx + 1))p")
  p95=$(echo "$sorted" | sed -n "$((p95_idx + 1))p")
  max=$(echo "$sorted" | tail -1)

  printf "  %-30s min=%.3fs  median=%.3fs  p95=%.3fs  max=%.3fs  (n=%d)\n" \
    "$method" "$min" "$median" "$p95" "$max" "$count"
}
```

**Step 3: Write README**

Create `tests/bridge_acceptance/README.md`:

```markdown
# Bridge Acceptance Tests

Acceptance tests for the Tauri Bridge before removing the deprecated macOS Swift app.

## Prerequisites

- `socat`, `jq`, `bc` installed (all available via Homebrew)
- `aleph-server` running with `aleph-tauri` bridge connected
- Bridge socket at `~/.aleph/bridge.sock` (or set `ALEPH_SOCKET_PATH`)

## Usage

```bash
# Run all tests
./tests/bridge_acceptance/run_all.sh

# Run individual suites
./tests/bridge_acceptance/test_functional_parity.sh
./tests/bridge_acceptance/test_e2e_flow.sh
./tests/bridge_acceptance/test_performance.sh
./tests/bridge_acceptance/test_stability.sh
```

## Design Reference

See `docs/plans/2026-02-25-remove-macos-swift-app-design.md` for the full acceptance matrix.
```

**Step 4: Commit**

```bash
git add tests/bridge_acceptance/lib.sh tests/bridge_acceptance/README.md
git commit -m "test: add bridge acceptance test infrastructure (lib.sh)"
```

---

## Task 2: Write Functional Parity Tests (F1-F12)

**Files:**
- Create: `tests/bridge_acceptance/test_functional_parity.sh`

**Step 1: Write the test script**

Create `tests/bridge_acceptance/test_functional_parity.sh`:

```bash
#!/usr/bin/env bash
# Functional Parity Tests (F1-F12)
# Verifies Tauri Bridge matches Swift app capabilities.
source "$(dirname "$0")/lib.sh"

check_deps
check_socket

echo ""
echo "╔══════════════════════════════════════════════════╗"
echo "║  Functional Parity Tests (F1-F12)               ║"
echo "╚══════════════════════════════════════════════════╝"
echo ""

# --- F1: Screenshot ---
test_f1_screenshot() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f1","method":"desktop.screenshot","params":{}}')
  assert_json_has "$result" ".result.image" || return 1
  assert_json_gt "$result" ".result.width" 0 || return 1
  assert_json_gt "$result" ".result.height" 0 || return 1
}
run_test "F1: Screenshot returns valid base64 PNG" test_f1_screenshot

# --- F2: OCR ---
test_f2_ocr() {
  # First take a screenshot to get image data
  local screenshot
  screenshot=$(send_rpc '{"jsonrpc":"2.0","id":"f2-ss","method":"desktop.screenshot","params":{}}')
  local image
  image=$(echo "$screenshot" | jq -r '.result.image')
  if [[ -z "$image" || "$image" == "null" ]]; then
    echo "FAIL: could not get screenshot for OCR test"
    return 1
  fi
  local result
  result=$(send_rpc "{\"jsonrpc\":\"2.0\",\"id\":\"f2\",\"method\":\"desktop.ocr\",\"params\":{\"image_base64\":\"$image\"}}" 10)
  assert_json_has "$result" ".result.text" || return 1
}

if [[ "$(uname)" == "Darwin" ]]; then
  run_test "F2: OCR recognizes text from screenshot" test_f2_ocr
else
  skip_test "F2: OCR recognizes text from screenshot" "macOS only"
fi

# --- F3: AX Tree ---
test_f3_ax_tree() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f3","method":"desktop.ax_tree","params":{"app":"frontmost"}}' 10)
  assert_json_has "$result" ".result.role" || return 1
  # Check tree has some depth (children exist)
  local child_count
  child_count=$(echo "$result" | jq '.result.children | length' 2>/dev/null)
  if [[ -z "$child_count" || "$child_count" -lt 1 ]]; then
    echo "FAIL: AX tree has no children"
    return 1
  fi
}

if [[ "$(uname)" == "Darwin" ]]; then
  run_test "F3: AX Tree returns element hierarchy" test_f3_ax_tree
else
  skip_test "F3: AX Tree returns element hierarchy" "macOS only"
fi

# --- F4: Mouse click ---
test_f4_click() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f4","method":"desktop.click","params":{"x":100,"y":100,"button":"left"}}')
  # Should succeed (no error)
  local error
  error=$(echo "$result" | jq -r '.error' 2>/dev/null)
  if [[ "$error" != "null" && -n "$error" ]]; then
    echo "FAIL: click returned error: $error"
    return 1
  fi
}
run_test "F4: Mouse click at coordinates" test_f4_click

# --- F5: Keyboard input ---
test_f5_type_text() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f5","method":"desktop.type_text","params":{"text":"hello"}}')
  local error
  error=$(echo "$result" | jq -r '.error' 2>/dev/null)
  if [[ "$error" != "null" && -n "$error" ]]; then
    echo "FAIL: type_text returned error: $error"
    return 1
  fi
}
run_test "F5: Keyboard text input" test_f5_type_text

# --- F5b: Key combo ---
test_f5b_key_combo() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f5b","method":"desktop.key_combo","params":{"modifiers":["meta"],"key":"a"}}')
  local error
  error=$(echo "$result" | jq -r '.error' 2>/dev/null)
  if [[ "$error" != "null" && -n "$error" ]]; then
    echo "FAIL: key_combo returned error: $error"
    return 1
  fi
}
run_test "F5b: Key combo (Cmd+A)" test_f5b_key_combo

# --- F6: App launch ---
test_f6_launch_app() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f6","method":"desktop.launch_app","params":{"bundle_id":"com.apple.Calculator"}}')
  local error
  error=$(echo "$result" | jq -r '.error' 2>/dev/null)
  if [[ "$error" != "null" && -n "$error" ]]; then
    echo "FAIL: launch_app returned error: $error"
    return 1
  fi
  # Give app a moment to launch
  sleep 1
}

if [[ "$(uname)" == "Darwin" ]]; then
  run_test "F6: Launch app by bundle ID" test_f6_launch_app
else
  skip_test "F6: Launch app by bundle ID" "macOS bundle ID"
fi

# --- F7: Window list ---
test_f7_window_list() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f7","method":"desktop.window_list","params":{}}')
  assert_json_has "$result" ".result.windows" || return 1
  local count
  count=$(echo "$result" | jq '.result.windows | length' 2>/dev/null)
  if [[ -z "$count" || "$count" -lt 1 ]]; then
    echo "FAIL: window list is empty"
    return 1
  fi
}
run_test "F7: Window list returns entries" test_f7_window_list

# --- F7b: Focus window ---
test_f7b_focus_window() {
  # Get first window from list
  local list
  list=$(send_rpc '{"jsonrpc":"2.0","id":"f7b-list","method":"desktop.window_list","params":{}}')
  local first_pid
  first_pid=$(echo "$list" | jq -r '.result.windows[0].pid' 2>/dev/null)
  if [[ -z "$first_pid" || "$first_pid" == "null" ]]; then
    echo "FAIL: no windows to focus"
    return 1
  fi
  local result
  result=$(send_rpc "{\"jsonrpc\":\"2.0\",\"id\":\"f7b\",\"method\":\"desktop.focus_window\",\"params\":{\"pid\":$first_pid}}")
  local error
  error=$(echo "$result" | jq -r '.error' 2>/dev/null)
  if [[ "$error" != "null" && -n "$error" ]]; then
    echo "FAIL: focus_window returned error: $error"
    return 1
  fi
}
run_test "F7b: Focus window by PID" test_f7b_focus_window

# --- F8: Canvas overlay ---
test_f8_canvas_show() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f8","method":"desktop.canvas_show","params":{"html":"<div>Test</div>","x":100,"y":100,"width":200,"height":200}}')
  local error
  error=$(echo "$result" | jq -r '.error' 2>/dev/null)
  if [[ "$error" != "null" && -n "$error" ]]; then
    echo "FAIL: canvas_show returned error: $error"
    return 1
  fi
  # Clean up
  send_rpc '{"jsonrpc":"2.0","id":"f8-hide","method":"desktop.canvas_hide","params":{}}' >/dev/null
}
run_test "F8: Canvas overlay show/hide" test_f8_canvas_show

# --- F9: Tray status ---
test_f9_tray() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f9","method":"tray.update_status","params":{"status":"idle","tooltip":"Test"}}')
  local error
  error=$(echo "$result" | jq -r '.error' 2>/dev/null)
  if [[ "$error" != "null" && -n "$error" ]]; then
    echo "FAIL: tray.update_status returned error: $error"
    return 1
  fi
}
run_test "F9: Tray status update" test_f9_tray

# --- F10: Halo window (WebView show/hide) ---
test_f10_halo() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f10-show","method":"webview.show","params":{"window":"halo"}}')
  local error
  error=$(echo "$result" | jq -r '.error' 2>/dev/null)
  if [[ "$error" != "null" && -n "$error" ]]; then
    echo "FAIL: webview.show returned error: $error"
    return 1
  fi
  sleep 0.5
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f10-hide","method":"webview.hide","params":{"window":"halo"}}')
  error=$(echo "$result" | jq -r '.error' 2>/dev/null)
  if [[ "$error" != "null" && -n "$error" ]]; then
    echo "FAIL: webview.hide returned error: $error"
    return 1
  fi
}
run_test "F10: Halo window show/hide" test_f10_halo

# --- F11: Settings window ---
test_f11_settings() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f11-show","method":"webview.show","params":{"window":"settings"}}')
  local error
  error=$(echo "$result" | jq -r '.error' 2>/dev/null)
  if [[ "$error" != "null" && -n "$error" ]]; then
    echo "FAIL: settings window show error: $error"
    return 1
  fi
  sleep 0.5
  send_rpc '{"jsonrpc":"2.0","id":"f11-hide","method":"webview.hide","params":{"window":"settings"}}' >/dev/null
}
run_test "F11: Settings window show/hide" test_f11_settings

# --- F12: Handshake ---
test_f12_handshake() {
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"f12","method":"aleph.handshake","params":{"protocol_version":"1.0"}}')
  assert_json_eq "$result" ".result.bridge_type" "desktop" || return 1
  assert_json_has "$result" ".result.platform" || return 1
  assert_json_has "$result" ".result.capabilities" || return 1
  local cap_count
  cap_count=$(echo "$result" | jq '.result.capabilities | length' 2>/dev/null)
  if [[ -z "$cap_count" || "$cap_count" -lt 3 ]]; then
    echo "FAIL: too few capabilities registered ($cap_count)"
    return 1
  fi
}
run_test "F12: Handshake returns capabilities" test_f12_handshake

# --- Summary ---
print_summary "Functional Parity"
```

**Step 2: Make executable and verify syntax**

Run: `chmod +x tests/bridge_acceptance/test_functional_parity.sh && bash -n tests/bridge_acceptance/test_functional_parity.sh`
Expected: No output (syntax valid)

**Step 3: Commit**

```bash
git add tests/bridge_acceptance/test_functional_parity.sh
git commit -m "test: add functional parity tests (F1-F12) for bridge acceptance"
```

---

## Task 3: Write End-to-End Flow Tests (E1-E6)

**Files:**
- Create: `tests/bridge_acceptance/test_e2e_flow.sh`

**Step 1: Write the test script**

Create `tests/bridge_acceptance/test_e2e_flow.sh`:

```bash
#!/usr/bin/env bash
# End-to-End Integration Tests (E1-E6)
# Tests server-bridge lifecycle: startup, handshake, shutdown, crash recovery.
#
# IMPORTANT: This script manages its own server/bridge processes.
# Do NOT run while another aleph-server is active on the same socket.
source "$(dirname "$0")/lib.sh"

check_deps

SERVER_BIN="${ALEPH_SERVER_BIN:-aleph-server}"
BRIDGE_SOCKET="$HOME/.aleph/bridge.sock"
SERVER_PID=""
LOG_FILE="/tmp/aleph-e2e-test.log"

cleanup() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null
    wait "$SERVER_PID" 2>/dev/null
  fi
  rm -f "$LOG_FILE"
}
trap cleanup EXIT

wait_for_log() {
  local pattern="$1"
  local timeout_secs="${2:-10}"
  local elapsed=0
  while [[ $elapsed -lt $timeout_secs ]]; do
    if grep -q "$pattern" "$LOG_FILE" 2>/dev/null; then
      return 0
    fi
    sleep 0.5
    elapsed=$((elapsed + 1))
  done
  echo "FAIL: timed out waiting for '$pattern' in logs ($timeout_secs s)"
  return 1
}

wait_for_socket() {
  local socket="$1"
  local timeout_secs="${2:-10}"
  local elapsed=0
  while [[ $elapsed -lt $timeout_secs ]]; do
    if [[ -S "$socket" ]]; then
      return 0
    fi
    sleep 0.5
    elapsed=$((elapsed + 1))
  done
  echo "FAIL: socket $socket not found within ${timeout_secs}s"
  return 1
}

echo ""
echo "╔══════════════════════════════════════════════════╗"
echo "║  End-to-End Integration Tests (E1-E6)           ║"
echo "╚══════════════════════════════════════════════════╝"
echo ""

# --- E1: Server startup spawns Bridge ---
test_e1_server_spawns_bridge() {
  $SERVER_BIN --features control-plane > "$LOG_FILE" 2>&1 &
  SERVER_PID=$!
  wait_for_log "Desktop bridge started" 10 || return 1
  wait_for_socket "$BRIDGE_SOCKET" 5 || return 1
}
run_test "E1: Server auto-spawns Bridge subprocess" test_e1_server_spawns_bridge

# --- E2: Bridge capability registration ---
test_e2_capability_registration() {
  wait_for_log "capabilities" 5 || return 1
  # Verify bridge responds to handshake
  SOCKET_PATH="$BRIDGE_SOCKET"
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"e2","method":"aleph.handshake","params":{"protocol_version":"1.0"}}')
  assert_json_has "$result" ".result.capabilities" || return 1
}
run_test "E2: Bridge registers capabilities" test_e2_capability_registration

# --- E3: Ping works (basic liveness) ---
test_e3_ping() {
  SOCKET_PATH="$BRIDGE_SOCKET"
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"e3","method":"system.ping","params":{}}')
  assert_json_eq "$result" ".result" "pong" || return 1
}
run_test "E3: Bridge responds to ping" test_e3_ping

# --- E4: Graceful shutdown ---
test_e4_graceful_shutdown() {
  if [[ -z "$SERVER_PID" ]] || ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "FAIL: server not running"
    return 1
  fi
  kill "$SERVER_PID"
  local elapsed=0
  while kill -0 "$SERVER_PID" 2>/dev/null && [[ $elapsed -lt 5 ]]; do
    sleep 0.5
    elapsed=$((elapsed + 1))
  done
  if kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "FAIL: server did not exit within 5s"
    return 1
  fi
  # Check no zombie bridge process
  if pgrep -f "aleph-tauri.*bridge-mode" >/dev/null 2>&1; then
    echo "FAIL: bridge process still running after server shutdown"
    return 1
  fi
  SERVER_PID=""
}
run_test "E4: Graceful shutdown cleans up processes" test_e4_graceful_shutdown

# --- E5: Bridge crash recovery ---
test_e5_crash_recovery() {
  # Start fresh server
  rm -f "$LOG_FILE"
  $SERVER_BIN --features control-plane > "$LOG_FILE" 2>&1 &
  SERVER_PID=$!
  wait_for_log "Desktop bridge started" 10 || return 1
  wait_for_socket "$BRIDGE_SOCKET" 5 || return 1

  # Kill bridge process
  local bridge_pid
  bridge_pid=$(pgrep -f "aleph-tauri.*bridge-mode" 2>/dev/null | head -1)
  if [[ -z "$bridge_pid" ]]; then
    echo "FAIL: could not find bridge process to kill"
    return 1
  fi
  kill -9 "$bridge_pid"
  sleep 1

  # Wait for reconnection (DesktopBridgeManager default restart delay is 3s)
  wait_for_log "Bridge reconnected\|bridge started\|handshake.*success" 15 || return 1

  # Verify bridge works after recovery
  wait_for_socket "$BRIDGE_SOCKET" 5 || return 1
  SOCKET_PATH="$BRIDGE_SOCKET"
  local result
  result=$(send_rpc '{"jsonrpc":"2.0","id":"e5","method":"system.ping","params":{}}')
  assert_json_eq "$result" ".result" "pong" || return 1

  # Clean up
  kill "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
}
run_test "E5: Bridge crash recovery" test_e5_crash_recovery

# --- E6: Headless mode (no bridge) ---
test_e6_headless() {
  # Start server with bridge disabled (set binary to nonexistent)
  rm -f "$LOG_FILE"
  ALEPH_BRIDGE_BINARY="/nonexistent/aleph-tauri" $SERVER_BIN > "$LOG_FILE" 2>&1 &
  SERVER_PID=$!
  wait_for_log "running headless\|bridge not started\|Desktop bridge not started" 10 || return 1
  # Server should still be alive
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "FAIL: server died in headless mode"
    return 1
  fi
  kill "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
}
run_test "E6: Server runs headless without bridge" test_e6_headless

# --- Summary ---
print_summary "End-to-End Integration"
```

**Step 2: Make executable and verify syntax**

Run: `chmod +x tests/bridge_acceptance/test_e2e_flow.sh && bash -n tests/bridge_acceptance/test_e2e_flow.sh`
Expected: No output (syntax valid)

**Step 3: Commit**

```bash
git add tests/bridge_acceptance/test_e2e_flow.sh
git commit -m "test: add end-to-end integration tests (E1-E6) for bridge acceptance"
```

---

## Task 4: Write Performance Benchmark Tests (P1-P5)

**Files:**
- Create: `tests/bridge_acceptance/test_performance.sh`

**Step 1: Write the test script**

Create `tests/bridge_acceptance/test_performance.sh`:

```bash
#!/usr/bin/env bash
# Performance Benchmark Tests (P1-P5)
# Measures latency and resource usage of the Tauri Bridge.
source "$(dirname "$0")/lib.sh"

check_deps
check_socket

echo ""
echo "╔══════════════════════════════════════════════════╗"
echo "║  Performance Benchmarks (P1-P5)                 ║"
echo "╚══════════════════════════════════════════════════╝"
echo ""

# --- P1: Screenshot latency ---
echo "P1: Screenshot latency (100 iterations)"
benchmark_rpc "desktop.screenshot" '{}' 100

# --- P2: OCR latency ---
if [[ "$(uname)" == "Darwin" ]]; then
  # Capture one screenshot for OCR tests
  SS_RESULT=$(send_rpc '{"jsonrpc":"2.0","id":"p2-ss","method":"desktop.screenshot","params":{}}')
  IMAGE=$(echo "$SS_RESULT" | jq -r '.result.image')
  if [[ -n "$IMAGE" && "$IMAGE" != "null" ]]; then
    echo "P2: OCR latency (50 iterations)"
    benchmark_rpc "desktop.ocr" "{\"image_base64\":\"$IMAGE\"}" 50
  else
    echo "P2: OCR latency — SKIPPED (no screenshot available)"
  fi
else
  echo "P2: OCR latency — SKIPPED (macOS only)"
fi

# --- P3: Input latency ---
echo "P3: Type text latency (50 iterations, 'hello')"
benchmark_rpc "desktop.type_text" '{"text":"a"}' 50

# --- P4: Bridge startup time ---
echo "P4: Handshake latency (proxy for bridge responsiveness, 20 iterations)"
benchmark_rpc "aleph.handshake" '{"protocol_version":"1.0"}' 20

# --- P5: Memory usage ---
echo ""
echo "P5: Current Bridge memory usage"
BRIDGE_PID=$(pgrep -f "aleph-tauri" 2>/dev/null | head -1)
if [[ -n "$BRIDGE_PID" ]]; then
  RSS_KB=$(ps -o rss= -p "$BRIDGE_PID" 2>/dev/null | tr -d ' ')
  RSS_MB=$((RSS_KB / 1024))
  echo "  Bridge PID: $BRIDGE_PID"
  echo "  RSS: ${RSS_MB} MB (${RSS_KB} KB)"
else
  echo "  Bridge process not found — SKIPPED"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Performance benchmarks complete. Review data above."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
```

**Step 2: Make executable and verify syntax**

Run: `chmod +x tests/bridge_acceptance/test_performance.sh && bash -n tests/bridge_acceptance/test_performance.sh`
Expected: No output (syntax valid)

**Step 3: Commit**

```bash
git add tests/bridge_acceptance/test_performance.sh
git commit -m "test: add performance benchmark tests (P1-P5) for bridge acceptance"
```

---

## Task 5: Write Stability Tests (S1-S4)

**Files:**
- Create: `tests/bridge_acceptance/test_stability.sh`

**Step 1: Write the test script**

Create `tests/bridge_acceptance/test_stability.sh`:

```bash
#!/usr/bin/env bash
# Stability Tests (S1-S4)
# Tests long-running behavior, rapid cycling, reconnection, and stress.
source "$(dirname "$0")/lib.sh"

check_deps
check_socket

# Configurable durations (override with env vars for CI)
LONG_RUN_SECS="${STABILITY_LONG_RUN_SECS:-300}"  # default 5 min (use 28800 for full 8h)
HOTKEY_CYCLES="${STABILITY_HOTKEY_CYCLES:-100}"
RECONNECT_CYCLES="${STABILITY_RECONNECT_CYCLES:-10}"
STRESS_RPS="${STABILITY_STRESS_RPS:-10}"
STRESS_DURATION_SECS="${STABILITY_STRESS_DURATION:-300}"

echo ""
echo "╔══════════════════════════════════════════════════╗"
echo "║  Stability Tests (S1-S4)                        ║"
echo "╚══════════════════════════════════════════════════╝"
echo ""

# --- S1: Long-running (memory leak check) ---
test_s1_long_running() {
  local bridge_pid
  bridge_pid=$(pgrep -f "aleph-tauri" 2>/dev/null | head -1)
  if [[ -z "$bridge_pid" ]]; then
    echo "FAIL: bridge process not found"
    return 1
  fi

  local rss_before
  rss_before=$(ps -o rss= -p "$bridge_pid" 2>/dev/null | tr -d ' ')
  echo "    RSS before: $((rss_before / 1024)) MB"
  echo "    Running for ${LONG_RUN_SECS}s (set STABILITY_LONG_RUN_SECS to override)..."

  # Periodic pings during the wait
  local elapsed=0
  while [[ $elapsed -lt $LONG_RUN_SECS ]]; do
    sleep 10
    elapsed=$((elapsed + 10))
    # Ping to keep bridge active
    send_rpc '{"jsonrpc":"2.0","id":"s1-ping","method":"system.ping","params":{}}' >/dev/null 2>&1
    if ! kill -0 "$bridge_pid" 2>/dev/null; then
      echo "FAIL: bridge crashed at ${elapsed}s"
      return 1
    fi
  done

  local rss_after
  rss_after=$(ps -o rss= -p "$bridge_pid" 2>/dev/null | tr -d ' ')
  echo "    RSS after: $((rss_after / 1024)) MB"

  local growth_pct
  growth_pct=$(echo "scale=1; ($rss_after - $rss_before) * 100 / $rss_before" | bc -l)
  echo "    Growth: ${growth_pct}%"

  if (( $(echo "$growth_pct > 20" | bc -l) )); then
    echo "FAIL: RSS grew by ${growth_pct}% (threshold 20%)"
    return 1
  fi
}
run_test "S1: Long-running stability (${LONG_RUN_SECS}s)" test_s1_long_running

# --- S2: Hotkey show/hide cycles ---
test_s2_hotkey_cycles() {
  local i failures=0
  for i in $(seq 1 "$HOTKEY_CYCLES"); do
    local show_result hide_result
    show_result=$(send_rpc '{"jsonrpc":"2.0","id":"s2-show","method":"webview.show","params":{"window":"halo"}}')
    if echo "$show_result" | jq -e '.error' >/dev/null 2>&1; then
      failures=$((failures + 1))
    fi
    sleep 0.1
    hide_result=$(send_rpc '{"jsonrpc":"2.0","id":"s2-hide","method":"webview.hide","params":{"window":"halo"}}')
    if echo "$hide_result" | jq -e '.error' >/dev/null 2>&1; then
      failures=$((failures + 1))
    fi
    sleep 0.1
  done
  if [[ $failures -gt 0 ]]; then
    echo "FAIL: $failures failures in $HOTKEY_CYCLES cycles"
    return 1
  fi
  echo "    $HOTKEY_CYCLES cycles completed, 0 failures"
}
run_test "S2: Hotkey show/hide cycles ($HOTKEY_CYCLES)" test_s2_hotkey_cycles

# --- S3: UDS disconnect/reconnect ---
test_s3_reconnect() {
  # This test rapidly opens and closes connections
  local i failures=0
  for i in $(seq 1 "$RECONNECT_CYCLES"); do
    local result
    result=$(send_rpc '{"jsonrpc":"2.0","id":"s3-'$i'","method":"system.ping","params":{}}')
    if ! echo "$result" | jq -e '.result == "pong"' >/dev/null 2>&1; then
      failures=$((failures + 1))
    fi
    sleep 0.5
  done
  if [[ $failures -gt 0 ]]; then
    echo "FAIL: $failures failed reconnections out of $RECONNECT_CYCLES"
    return 1
  fi
  echo "    $RECONNECT_CYCLES connections, 0 failures"
}
run_test "S3: UDS connection cycling ($RECONNECT_CYCLES)" test_s3_reconnect

# --- S4: High-frequency RPC stress ---
test_s4_stress() {
  local total=$((STRESS_RPS * STRESS_DURATION_SECS))
  local failures=0
  local interval
  interval=$(echo "scale=4; 1.0 / $STRESS_RPS" | bc -l)
  local start_time
  start_time=$(python3 -c 'import time; print(time.time())')

  echo "    Sending $total requests at ${STRESS_RPS} req/s for ${STRESS_DURATION_SECS}s..."

  for i in $(seq 1 "$total"); do
    local result
    result=$(send_rpc '{"jsonrpc":"2.0","id":"s4-'$i'","method":"system.ping","params":{}}' 2)
    if ! echo "$result" | jq -e '.result == "pong"' >/dev/null 2>&1; then
      failures=$((failures + 1))
    fi
    sleep "$interval"
  done

  local end_time
  end_time=$(python3 -c 'import time; print(time.time())')
  local elapsed
  elapsed=$(echo "$end_time - $start_time" | bc -l)

  echo "    Completed: $total requests in ${elapsed}s, $failures failures"

  if [[ $failures -gt 0 ]]; then
    echo "FAIL: $failures timeouts/failures"
    return 1
  fi
}
run_test "S4: High-frequency RPC stress (${STRESS_RPS}rps x ${STRESS_DURATION_SECS}s)" test_s4_stress

# --- Summary ---
print_summary "Stability"
```

**Step 2: Make executable and verify syntax**

Run: `chmod +x tests/bridge_acceptance/test_stability.sh && bash -n tests/bridge_acceptance/test_stability.sh`
Expected: No output (syntax valid)

**Step 3: Commit**

```bash
git add tests/bridge_acceptance/test_stability.sh
git commit -m "test: add stability tests (S1-S4) for bridge acceptance"
```

---

## Task 6: Write Test Runner and Manual Verification Checklist

**Files:**
- Create: `tests/bridge_acceptance/run_all.sh`
- Create: `tests/bridge_acceptance/manual_checklist.md`

**Step 1: Write the test runner**

Create `tests/bridge_acceptance/run_all.sh`:

```bash
#!/usr/bin/env bash
# Run all bridge acceptance test suites.
# Usage: ./tests/bridge_acceptance/run_all.sh [--skip-stability]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKIP_STABILITY=false

for arg in "$@"; do
  case "$arg" in
    --skip-stability) SKIP_STABILITY=true ;;
  esac
done

echo "╔══════════════════════════════════════════════════╗"
echo "║        Bridge Acceptance Test Suite              ║"
echo "║  Design: docs/plans/2026-02-25-remove-macos-    ║"
echo "║          swift-app-design.md                     ║"
echo "╚══════════════════════════════════════════════════╝"
echo ""
echo "Socket: ${ALEPH_SOCKET_PATH:-~/.aleph/bridge.sock}"
echo ""

OVERALL_PASS=true

echo "▶ Suite 1/4: Functional Parity (F1-F12)"
if ! "$SCRIPT_DIR/test_functional_parity.sh"; then
  OVERALL_PASS=false
  echo "⚠ Functional parity tests had failures."
fi

echo ""
echo "▶ Suite 2/4: Performance Benchmarks (P1-P5)"
"$SCRIPT_DIR/test_performance.sh"
echo "(Performance results are informational — review data above)"

if [[ "$SKIP_STABILITY" == "false" ]]; then
  echo ""
  echo "▶ Suite 3/4: Stability (S1-S4)"
  if ! "$SCRIPT_DIR/test_stability.sh"; then
    OVERALL_PASS=false
    echo "⚠ Stability tests had failures."
  fi
else
  echo ""
  echo "▶ Suite 3/4: Stability — SKIPPED (--skip-stability)"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [[ "$OVERALL_PASS" == "true" ]]; then
  echo "  ✓ All automated tests PASSED"
  echo ""
  echo "  Next steps:"
  echo "  1. Complete manual verification: tests/bridge_acceptance/manual_checklist.md"
  echo "  2. Run E2E tests: ./tests/bridge_acceptance/test_e2e_flow.sh"
  echo "     (requires no running server — it manages its own)"
  echo "  3. When all pass, proceed with cleanup"
else
  echo "  ✗ Some tests FAILED — fix issues before proceeding"
fi
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

$OVERALL_PASS
```

**Step 2: Write manual verification checklist**

Create `tests/bridge_acceptance/manual_checklist.md`:

```markdown
# Manual Verification Checklist (M1-M5)

Complete these checks with aleph-server + aleph-tauri running.
Mark each item as you verify it.

## M1: OCR Chinese Accuracy
- [ ] Take a screenshot of a Chinese webpage
- [ ] Run `desktop.ocr` via the Tauri bridge
- [ ] Verify Chinese characters are correctly recognized
- [ ] Compare with Swift version if available (optional baseline)

## M2: Canvas Overlay Visual Quality
- [ ] Send `desktop.canvas_show` with a styled HTML div
- [ ] Verify the overlay is transparent (can see desktop beneath)
- [ ] Verify the overlay is positioned correctly
- [ ] Send `desktop.canvas_update` with an A2UI patch
- [ ] Verify the patch applies visually
- [ ] Send `desktop.canvas_hide` and verify it disappears

## M3: Tray Menu Interaction
- [ ] Verify tray icon appears in menu bar
- [ ] Click tray icon — menu appears with provider list
- [ ] Switch provider — verify it sticks after re-opening menu
- [ ] Click "Settings..." — settings window opens
- [ ] Click "Quit Aleph" — app exits cleanly

## M4: Halo Window Experience
- [ ] Press Ctrl+Alt+/ (or configured hotkey)
- [ ] Verify Halo appears centered, bottom 30% of screen
- [ ] Verify Halo is transparent/borderless
- [ ] Press Escape — Halo hides
- [ ] Repeat 5 times — no visual glitches

## M5: Settings Page Functionality
- [ ] Open Settings window
- [ ] Navigate to General settings
- [ ] Change a setting (e.g., language)
- [ ] Save and close settings
- [ ] Re-open settings — verify change persisted
- [ ] Navigate to Providers — add/edit/delete a provider

---

**Result:** All items verified? [ ] YES / [ ] NO

**Date:** ___________
**Verified by:** ___________
```

**Step 3: Make runner executable and verify syntax**

Run: `chmod +x tests/bridge_acceptance/run_all.sh && bash -n tests/bridge_acceptance/run_all.sh`
Expected: No output (syntax valid)

**Step 4: Commit**

```bash
git add tests/bridge_acceptance/run_all.sh tests/bridge_acceptance/manual_checklist.md
git commit -m "test: add test runner and manual verification checklist for bridge acceptance"
```

---

## Task 7: Run Acceptance Tests

**Prerequisite**: `aleph-server` and `aleph-tauri` bridge running.

**Step 1: Verify bridge is running**

Run: `echo '{"jsonrpc":"2.0","id":"check","method":"system.ping","params":{}}' | socat - UNIX-CONNECT:$HOME/.aleph/bridge.sock`
Expected: `{"jsonrpc":"2.0","id":"check","result":"pong"}`

**Step 2: Run functional parity tests**

Run: `./tests/bridge_acceptance/test_functional_parity.sh`
Expected: All F1-F12 PASS (macOS-only tests may SKIP on other platforms)

**Step 3: Run performance benchmarks**

Run: `./tests/bridge_acceptance/test_performance.sh`
Expected: Output showing latency stats for each metric. Record these numbers.

**Step 4: Run stability tests (short mode for validation)**

Run: `STABILITY_LONG_RUN_SECS=60 STABILITY_STRESS_DURATION=30 ./tests/bridge_acceptance/test_stability.sh`
Expected: All S1-S4 PASS

**Step 5: Complete manual verification**

Open `tests/bridge_acceptance/manual_checklist.md` and complete M1-M5 manually.

**Step 6: Run E2E tests (requires no existing server)**

Stop the running server first, then:
Run: `./tests/bridge_acceptance/test_e2e_flow.sh`
Expected: All E1-E6 PASS

**Step 7: Record results and commit**

Update `tests/bridge_acceptance/manual_checklist.md` with results.

```bash
git add tests/bridge_acceptance/manual_checklist.md
git commit -m "test: record bridge acceptance test results — all pass"
```

---

## Task 8: Phase A — Remove Bindings and FFI (C1-C6)

**Prerequisite**: Task 7 acceptance tests all passed.

**Files:**
- Delete: `core/bindings/aleph.swift`
- Delete: `core/bindings/alephFFI.h`
- Delete: `apps/macos/Aleph/Frameworks/libalephcore.dylib`
- Delete: `apps/macos/Aleph/Sources/Generated/aleph.swift`
- Delete: `apps/macos/Aleph/Sources/Generated/alephFFI.h`
- Delete: `apps/macos/Aleph/Sources/FFICompatibilityLayer.swift`

**Step 1: Delete binding files**

```bash
rm -f core/bindings/aleph.swift
rm -f core/bindings/alephFFI.h
```

**Step 2: Verify no compile-time dependency on bindings**

Run: `cargo check --bin aleph-server --features control-plane`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add -u core/bindings/
git commit -m "cleanup: remove UniFFI Swift bindings (C1-C2)"
```

---

## Task 9: Phase B — Remove macOS App Directory (C7)

**Files:**
- Delete: `apps/macos/` (entire directory)

**Step 1: Delete the directory**

```bash
rm -rf apps/macos/
```

**Step 2: Verify Rust compiles**

Run: `cargo check --bin aleph-server --features control-plane`
Expected: Compiles successfully

**Step 3: Verify Tauri compiles**

Run: `cd apps/desktop && cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add -u apps/macos/
git rm -r --cached apps/macos/ 2>/dev/null || true
git commit -m "cleanup: remove deprecated macOS Swift app (C7)

Replaced by aleph-bridge (Tauri) with full functional parity.
Acceptance tests passed: F1-F12, E1-E6, M1-M5, P1-P5, S1-S4."
```

---

## Task 10: Phase C — Clean Residual References (C8-C12)

**Files:**
- Modify: `.github/workflows/macos-app.yml` (delete entire file)
- Modify: `Scripts/verify_swift_syntax.py` (delete entire file)
- Modify: `Scripts/build-macos.sh` (delete entire file)
- Modify: `Scripts/quick_build.sh` (delete entire file)
- Modify: `Scripts/gen_bindings.sh` (delete entire file)
- Modify: `Scripts/generate_bindings.py` (delete entire file)
- Modify: `Scripts/generate-bindings.sh` (delete entire file)
- Modify: `Scripts/copy_rust_libs.sh` (delete entire file)
- Modify: `.gitignore` (review Xcode patterns)

**Step 1: Delete macOS-specific CI workflow**

```bash
rm -f .github/workflows/macos-app.yml
```

**Step 2: Delete macOS-specific build scripts**

```bash
rm -f Scripts/verify_swift_syntax.py
rm -f Scripts/build-macos.sh
rm -f Scripts/quick_build.sh
rm -f Scripts/gen_bindings.sh
rm -f Scripts/generate_bindings.py
rm -f Scripts/generate-bindings.sh
rm -f Scripts/copy_rust_libs.sh
rm -f Scripts/create-dmg.sh
```

**Step 3: Search for remaining references**

Run: `grep -rn "apps/macos\|libalephcore\|alephFFI\|uniffi\|xcodegen\|xcodeproj\|verify_swift_syntax" --include="*.rs" --include="*.toml" --include="*.yml" --include="*.yaml" . | grep -v "docs/plans/" | grep -v "docs/legacy/" | grep -v "openspec/" | grep -v ".git/"`

Expected: No matches outside of planning/legacy docs (which are historical records and stay).

**Step 4: Commit**

```bash
git add -u .github/workflows/ Scripts/
git commit -m "cleanup: remove macOS-specific CI workflow and build scripts (C8-C12)"
```

---

## Task 11: Phase D — Update Documentation (C13-C16)

**Files:**
- Modify: `CLAUDE.md` (lines 247, 280, 300-301, 341-344)
- Create: `docs/plans/2026-02-25-macos-app-removal-record.md`

**Step 1: Update CLAUDE.md — project structure**

In `CLAUDE.md`, replace:

```
├── apps/
│   ├── cli/                        # Rust CLI 客户端
│   ├── macos/                      # [DEPRECATED] macOS App → replaced by Tauri Bridge
│   └── desktop/                    # Cross-platform Tauri Bridge (aleph-bridge)
```

With:

```
├── apps/
│   ├── cli/                        # Rust CLI 客户端
│   └── desktop/                    # Cross-platform Tauri Bridge (aleph-bridge)
```

**Step 2: Update CLAUDE.md — tech stack**

In `CLAUDE.md`, replace:

```
| **macOS App** | Swift + SwiftUI + AppKit |
| **Cross-platform** | Tauri + React |
```

With:

```
| **Desktop App** | Tauri (cross-platform bridge) |
```

**Step 3: Update CLAUDE.md — build commands**

In `CLAUDE.md`, remove the deprecated macOS build command block:

```
# [DEPRECATED] macOS App (保留仅供参考)
# cd apps/macos && xcodegen generate && open Aleph.xcodeproj
```

**Step 4: Update CLAUDE.md — environment section**

In `CLAUDE.md`, replace lines 341-344:

```
- Xcode generation: cd apps/macos && xcodegen generate
- Syntax validation: ~/.uv/python3/bin/python Scripts/verify_swift_syntax.py <file.swift>
- Xcode build cache cleanup: rm -rf ~/Library/Developer/Xcode/DerivedData/(Aleph)-*
- This project uses XcodeGen to manage the Xcode project. See docs/XCODEGEN_README.md for detailed workflow instructions.
```

With nothing (delete these 4 lines entirely).

**Step 5: Create migration completion record**

Create `docs/plans/2026-02-25-macos-app-removal-record.md` with acceptance test results captured in Task 7.

**Step 6: Commit**

```bash
git add CLAUDE.md docs/plans/2026-02-25-macos-app-removal-record.md
git commit -m "docs: update CLAUDE.md and create migration completion record (C13-C16)

- Remove apps/macos from project structure
- Remove Swift/Xcode from tech stack
- Remove deprecated build commands and environment notes
- Add migration completion record with acceptance test results"
```

---

## Task 12: Phase E — Verify Cleanup Completeness (C17-C20)

**Step 1: Rust compiles clean**

Run: `cargo check --bin aleph-server --features control-plane`
Expected: Compiles without errors

**Step 2: Tauri compiles clean**

Run: `cd apps/desktop && cargo check`
Expected: Compiles without errors

**Step 3: No dangling references**

Run: `grep -rn "apps/macos\|libalephcore\|uniffi\|alephFFI" --include="*.rs" --include="*.toml" . | grep -v docs/ | grep -v openspec/ | grep -v .git/`
Expected: No matches

**Step 4: Bridge E2E still works**

Run: `echo '{"jsonrpc":"2.0","id":"final","method":"system.ping","params":{}}' | socat - UNIX-CONNECT:$HOME/.aleph/bridge.sock`
Expected: `{"jsonrpc":"2.0","id":"final","result":"pong"}`

**Step 5: Final commit (squash-friendly tag)**

```bash
git tag pre-macos-removal-cleanup
git log --oneline -10  # Review all commits in this plan
```

---

## Summary

| Task | Description | Key Files | Commit |
|------|------------|-----------|--------|
| 1 | Test infrastructure | `tests/bridge_acceptance/lib.sh` | `test: add bridge acceptance test infrastructure` |
| 2 | Functional parity tests | `test_functional_parity.sh` | `test: add functional parity tests (F1-F12)` |
| 3 | E2E flow tests | `test_e2e_flow.sh` | `test: add end-to-end tests (E1-E6)` |
| 4 | Performance benchmarks | `test_performance.sh` | `test: add performance benchmarks (P1-P5)` |
| 5 | Stability tests | `test_stability.sh` | `test: add stability tests (S1-S4)` |
| 6 | Test runner + manual checklist | `run_all.sh`, `manual_checklist.md` | `test: add test runner and checklist` |
| 7 | **Run all acceptance tests** | — | `test: record acceptance results` |
| 8 | Phase A: Remove bindings | `core/bindings/` | `cleanup: remove UniFFI bindings` |
| 9 | Phase B: Remove macOS app | `apps/macos/` | `cleanup: remove deprecated macOS app` |
| 10 | Phase C: Clean references | workflows, scripts | `cleanup: remove macOS CI and scripts` |
| 11 | Phase D: Update docs | `CLAUDE.md`, record | `docs: update documentation` |
| 12 | Phase E: Verify completeness | — | (tag only) |
