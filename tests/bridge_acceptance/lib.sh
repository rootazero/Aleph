#!/usr/bin/env bash
# Bridge Acceptance Test Library
# Shared helpers for all bridge acceptance test scripts.
#
# Usage: source "$(dirname "$0")/lib.sh"

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

SOCKET_PATH="${ALEPH_SOCKET_PATH:-$HOME/.aleph/bridge.sock}"
ALEPH_SERVER_BIN="${ALEPH_SERVER_BIN:-}"

# ---------------------------------------------------------------------------
# Colors
# ---------------------------------------------------------------------------

if [[ -t 1 ]]; then
    GREEN=$'\033[0;32m'
    RED=$'\033[0;31m'
    YELLOW=$'\033[0;33m'
    CYAN=$'\033[0;36m'
    BOLD=$'\033[1m'
    RESET=$'\033[0m'
else
    GREEN=""
    RED=""
    YELLOW=""
    CYAN=""
    BOLD=""
    RESET=""
fi

# ---------------------------------------------------------------------------
# Counters
# ---------------------------------------------------------------------------

TESTS_PASSED=0
TESTS_FAILED=0
TESTS_SKIPPED=0
TESTS_TOTAL=0
FAILED_TESTS=""

# ---------------------------------------------------------------------------
# Dependency checks
# ---------------------------------------------------------------------------

# check_deps — verify required tools are available
# Arguments: tool names (varargs)
check_deps() {
    local missing=()
    for cmd in "$@"; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        echo "${RED}ERROR: Missing required tools: ${missing[*]}${RESET}" >&2
        echo "Install them before running tests." >&2
        return 1
    fi
}

# check_socket — verify the UDS socket exists and is connectable
check_socket() {
    if [[ ! -S "$SOCKET_PATH" ]]; then
        echo "${RED}ERROR: Socket not found at $SOCKET_PATH${RESET}" >&2
        echo "Make sure the Tauri bridge (aleph-bridge) is running." >&2
        echo "Set ALEPH_SOCKET_PATH to override the default path." >&2
        return 1
    fi
}

# is_macos — returns 0 on macOS, 1 otherwise
is_macos() {
    [[ "$(uname -s)" == "Darwin" ]]
}

# ---------------------------------------------------------------------------
# JSON-RPC helpers
# ---------------------------------------------------------------------------

# send_rpc — send a JSON-RPC 2.0 request over UDS, return the response
# Arguments:
#   $1 — method name (e.g. "desktop.ping")
#   $2 — params JSON (optional, default: "{}")
#   $3 — request id (optional, default: "test-<random>")
# Returns: JSON response on stdout
# Exit code: 0 on success, 1 on connection failure
send_rpc() {
    local method="$1"
    local params="${2:-"{}"}"
    local id="${3:-test-$(( RANDOM % 10000 ))}"

    local request
    request=$(printf '{"jsonrpc":"2.0","id":"%s","method":"%s","params":%s}\n' \
        "$id" "$method" "$params")

    local response
    response=$(echo "$request" | socat -T10 - UNIX-CONNECT:"$SOCKET_PATH" 2>/dev/null) || {
        echo "${RED}ERROR: Failed to connect to socket $SOCKET_PATH${RESET}" >&2
        return 1
    }

    echo "$response"
}

# send_rpc_timed — like send_rpc but prints elapsed time in milliseconds
# Arguments: same as send_rpc
# Outputs: JSON response on stdout, time in ms on fd 3
# Usage:
#   exec 3>&1
#   response=$(send_rpc_timed "desktop.ping" "{}" "t1" 3>&1 1>&4) 4>&1
# Simpler usage via benchmark_rpc below.
send_rpc_timed() {
    local method="$1"
    local params="${2:-"{}"}"
    local id="${3:-test-$(( RANDOM % 10000 ))}"

    local start_ns end_ns elapsed_ms response

    start_ns=$(python3 -c "import time; print(int(time.time_ns()))")

    response=$(send_rpc "$method" "$params" "$id") || return 1

    end_ns=$(python3 -c "import time; print(int(time.time_ns()))")

    elapsed_ms=$(echo "scale=2; ($end_ns - $start_ns) / 1000000" | bc)

    echo "$response"
    echo "$elapsed_ms" >&3 2>/dev/null || true
}

# benchmark_rpc — run an RPC call N times and report statistics
# Arguments:
#   $1 — label (human-readable)
#   $2 — method
#   $3 — params JSON
#   $4 — iterations
#   $5 — max acceptable p95 in ms (for pass/fail)
# Returns: 0 if p95 <= max, 1 otherwise
benchmark_rpc() {
    local label="$1"
    local method="$2"
    local params="${3:-{}}"
    local iterations="${4:-10}"
    local max_p95="${5:-5000}"

    local times=()
    local failures=0

    for (( i=1; i<=iterations; i++ )); do
        local start_ns end_ns elapsed_ms response

        start_ns=$(python3 -c "import time; print(int(time.time_ns()))")

        response=$(send_rpc "$method" "$params" "bench-$i") || {
            failures=$((failures + 1))
            continue
        }

        end_ns=$(python3 -c "import time; print(int(time.time_ns()))")

        elapsed_ms=$(echo "scale=2; ($end_ns - $start_ns) / 1000000" | bc)
        times+=("$elapsed_ms")
    done

    if [[ ${#times[@]} -eq 0 ]]; then
        echo "  ${RED}$label: all $iterations iterations failed${RESET}"
        return 1
    fi

    # Sort times numerically
    local sorted
    sorted=$(printf '%s\n' "${times[@]}" | sort -g)

    local count=${#times[@]}
    local min max avg p95 sum

    min=$(echo "$sorted" | head -1)
    max=$(echo "$sorted" | tail -1)

    sum=0
    for t in "${times[@]}"; do
        sum=$(echo "$sum + $t" | bc)
    done
    avg=$(echo "scale=2; $sum / $count" | bc)

    # p95 index (1-based, ceiling)
    local p95_idx
    p95_idx=$(echo "($count * 95 + 99) / 100" | bc)
    if [[ "$p95_idx" -gt "$count" ]]; then
        p95_idx=$count
    fi
    p95=$(echo "$sorted" | sed -n "${p95_idx}p")

    printf "  %s: min=%.1fms avg=%.1fms p95=%.1fms max=%.1fms (%d/%d ok)\n" \
        "$label" "$min" "$avg" "$p95" "$max" "$count" "$iterations"

    if [[ $failures -gt 0 ]]; then
        echo "  ${YELLOW}  ($failures failures)${RESET}"
    fi

    # Compare p95 against threshold
    local pass
    pass=$(echo "$p95 <= $max_p95" | bc -l)
    if [[ "$pass" -eq 1 ]]; then
        return 0
    else
        echo "  ${RED}  p95 ${p95}ms exceeds threshold ${max_p95}ms${RESET}"
        return 1
    fi
}

# ---------------------------------------------------------------------------
# JSON assertion helpers
# ---------------------------------------------------------------------------

# assert_json_has — assert a jq expression produces non-null, non-empty output
# Arguments:
#   $1 — JSON string
#   $2 — jq filter (e.g. ".result")
#   $3 — description (for error message)
assert_json_has() {
    local json="$1"
    local filter="$2"
    local desc="${3:-$filter}"

    local value
    value=$(echo "$json" | jq -r "$filter" 2>/dev/null) || {
        echo "${RED}FAIL: $desc — invalid JSON${RESET}" >&2
        return 1
    }

    if [[ -z "$value" || "$value" == "null" ]]; then
        echo "${RED}FAIL: $desc — expected non-null at $filter${RESET}" >&2
        echo "  Got: $json" >&2
        return 1
    fi
}

# assert_json_eq — assert a jq expression equals an expected value
# Arguments:
#   $1 — JSON string
#   $2 — jq filter
#   $3 — expected value (string)
#   $4 — description (optional)
assert_json_eq() {
    local json="$1"
    local filter="$2"
    local expected="$3"
    local desc="${4:-$filter == $expected}"

    local actual
    actual=$(echo "$json" | jq -r "$filter" 2>/dev/null) || {
        echo "${RED}FAIL: $desc — invalid JSON${RESET}" >&2
        return 1
    }

    if [[ "$actual" != "$expected" ]]; then
        echo "${RED}FAIL: $desc — expected '$expected', got '$actual'${RESET}" >&2
        echo "  Response: $json" >&2
        return 1
    fi
}

# assert_json_gt — assert a jq expression (numeric) is greater than a threshold
# Arguments:
#   $1 — JSON string
#   $2 — jq filter
#   $3 — threshold (number)
#   $4 — description (optional)
assert_json_gt() {
    local json="$1"
    local filter="$2"
    local threshold="$3"
    local desc="${4:-$filter > $threshold}"

    local actual
    actual=$(echo "$json" | jq -r "$filter" 2>/dev/null) || {
        echo "${RED}FAIL: $desc — invalid JSON${RESET}" >&2
        return 1
    }

    if [[ "$actual" == "null" || -z "$actual" ]]; then
        echo "${RED}FAIL: $desc — got null${RESET}" >&2
        return 1
    fi

    local pass
    pass=$(echo "$actual > $threshold" | bc -l 2>/dev/null) || {
        echo "${RED}FAIL: $desc — non-numeric value '$actual'${RESET}" >&2
        return 1
    }

    if [[ "$pass" -ne 1 ]]; then
        echo "${RED}FAIL: $desc — $actual is not > $threshold${RESET}" >&2
        return 1
    fi
}

# assert_no_error — assert the JSON-RPC response has no error field
# Arguments:
#   $1 — JSON response
#   $2 — description (optional)
assert_no_error() {
    local json="$1"
    local desc="${2:-no error}"

    local has_error
    has_error=$(echo "$json" | jq 'has("error")' 2>/dev/null) || {
        echo "${RED}FAIL: $desc — invalid JSON${RESET}" >&2
        return 1
    }

    if [[ "$has_error" == "true" ]]; then
        local code msg
        code=$(echo "$json" | jq -r '.error.code' 2>/dev/null)
        msg=$(echo "$json" | jq -r '.error.message' 2>/dev/null)
        echo "${RED}FAIL: $desc — got error code=$code msg=$msg${RESET}" >&2
        return 1
    fi
}

# assert_is_error — assert the JSON-RPC response IS an error with specific code
# Arguments:
#   $1 — JSON response
#   $2 — expected error code
#   $3 — description (optional)
assert_is_error() {
    local json="$1"
    local expected_code="$2"
    local desc="${3:-is error $expected_code}"

    local actual_code
    actual_code=$(echo "$json" | jq -r '.error.code' 2>/dev/null) || {
        echo "${RED}FAIL: $desc — invalid JSON${RESET}" >&2
        return 1
    }

    if [[ "$actual_code" == "null" || -z "$actual_code" ]]; then
        echo "${RED}FAIL: $desc — expected error but got success${RESET}" >&2
        echo "  Response: $json" >&2
        return 1
    fi

    if [[ "$actual_code" != "$expected_code" ]]; then
        echo "${RED}FAIL: $desc — expected error code $expected_code, got $actual_code${RESET}" >&2
        return 1
    fi
}

# ---------------------------------------------------------------------------
# Test runner helpers
# ---------------------------------------------------------------------------

# run_test — run a test function and track results
# Arguments:
#   $1 — test name
#   $2 — test function name
run_test() {
    local name="$1"
    local func="$2"

    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    printf "  %-60s " "$name"

    local output
    if output=$($func 2>&1); then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        echo "${GREEN}PASS${RESET}"
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        FAILED_TESTS="${FAILED_TESTS}    - ${name}\n"
        echo "${RED}FAIL${RESET}"
        if [[ -n "$output" ]]; then
            echo "$output" | sed 's/^/    /'
        fi
    fi
}

# skip_test — mark a test as skipped
# Arguments:
#   $1 — test name
#   $2 — reason
skip_test() {
    local name="$1"
    local reason="${2:-}"

    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    TESTS_SKIPPED=$((TESTS_SKIPPED + 1))
    printf "  %-60s ${YELLOW}SKIP${RESET}" "$name"
    if [[ -n "$reason" ]]; then
        echo " ($reason)"
    else
        echo ""
    fi
}

# print_summary — print test results summary
# Arguments: none (uses global counters)
# Returns: 0 if all passed, 1 if any failed
print_summary() {
    echo ""
    echo "${BOLD}========================================${RESET}"
    echo "${BOLD}Test Summary${RESET}"
    echo "${BOLD}========================================${RESET}"
    echo "  Total:   $TESTS_TOTAL"
    echo "  ${GREEN}Passed:  $TESTS_PASSED${RESET}"
    echo "  ${RED}Failed:  $TESTS_FAILED${RESET}"
    echo "  ${YELLOW}Skipped: $TESTS_SKIPPED${RESET}"

    if [[ $TESTS_FAILED -gt 0 ]]; then
        echo ""
        echo "${RED}Failed tests:${RESET}"
        printf "$FAILED_TESTS"
        echo ""
        return 1
    else
        echo ""
        echo "${GREEN}All tests passed.${RESET}"
        return 0
    fi
}

# print_header — print a section header
# Arguments:
#   $1 — section title
print_header() {
    local title="$1"
    echo ""
    echo "${BOLD}${CYAN}--- $title ---${RESET}"
    echo ""
}

# wait_for_socket — wait until the socket appears (up to timeout)
# Arguments:
#   $1 — timeout in seconds (default: 10)
# Returns: 0 if socket appeared, 1 if timeout
wait_for_socket() {
    local timeout="${1:-10}"
    local elapsed=0

    while [[ ! -S "$SOCKET_PATH" ]] && [[ $elapsed -lt $timeout ]]; do
        sleep 1
        elapsed=$((elapsed + 1))
    done

    [[ -S "$SOCKET_PATH" ]]
}

# get_process_memory_kb — get RSS of a process in KB
# Arguments:
#   $1 — PID
get_process_memory_kb() {
    local pid="$1"
    if is_macos; then
        ps -o rss= -p "$pid" 2>/dev/null | tr -d ' '
    else
        ps -o rss= -p "$pid" 2>/dev/null | tr -d ' '
    fi
}
