#!/usr/bin/env bash
# Bridge Acceptance Tests — Functional Parity
#
# Verifies that the macOS Swift helper (AlephBridge) actually SERVES the JSON-RPC
# surface Aleph publishes: the methods exist, and they enforce their contracts.
#
# ---------------------------------------------------------------------------
# What this file used to do, and why it was worse than nothing
# ---------------------------------------------------------------------------
#
# It was fake green, in four separate ways:
#
#   1. It accepted a "not implemented" reply as a PASS. Every test was shaped
#      `if error.code == -32000; then return 0`. So in a build where click and
#      type_text did not exist AT ALL, the click and type_text acceptance tests
#      were green. A test that passes when the feature is absent is not a test.
#   2. -32000 is not even a code this protocol defines (see
#      shared/protocol/src/desktop_bridge/errors.rs: NOT_IMPLEMENTED is -32002).
#      The escape hatch was keyed on a phantom.
#   3. It called `desktop.click`, `desktop.type_text`, `desktop.ax_tree` … — a
#      namespace the Router does not register and never did. The real surface is
#      `bridge.*` / `ax.*` / `input.*` / `screen.*`.
#   4. It spoke to a Unix socket (`~/.aleph/bridge.sock`) belonging to a Tauri
#      bridge that no longer exists anywhere in this repo. The helper is
#      stdio-only (see Sources/AlephBridge/main.swift), and its `Request.id` is a
#      UInt64 — so the string ids this suite sent ("test-1234") could not even be
#      parsed. It could never have run against the real bridge, which is how it
#      stayed "green" while asserting nothing.
#
# So: real transport (stdio), real methods, and an error is an error.
#
#   -32601 method not found  → FAIL. The method is not registered.
#   -32002 not implemented   → FAIL. This is the hole that was papered over.
#   -32001 permission denied → SKIP, loudly, and only for the permission-gated
#                              families (or FAIL under PARITY_STRICT=1). A missing
#                              TCC grant is an environment fact, not a passing
#                              test — it is never counted as one.
#
# ---------------------------------------------------------------------------
# Scope
# ---------------------------------------------------------------------------
#
# This checks the SURFACE and the CONTRACTS, and it deliberately actuates nothing:
# a script that clicks and types on whatever the user happens to have in front of
# them is hostile, and a click into the void asserts nothing anyway. Proving that
# input really lands is the job of the closed-loop e2e suite, which drives this
# same bridge at a fixture app that reports back what happened to it:
#
#     just test-computer-use-e2e     (desktop/macos/tests/computer_use_e2e.rs)
#
# The strongest check here needs no permissions at all: `bridge.handshake` reports
# `supported_methods`, so the registry assertion below catches a method that has
# been dropped from the Router — precisely the bug that let this suite go on
# calling `desktop.click` for however long it did.
#
# Usage:
#   ./test_functional_parity.sh
#   ALEPH_BRIDGE_BIN=/path/to/AlephBridge ./test_functional_parity.sh
#   PARITY_STRICT=1 ./test_functional_parity.sh   # permission gaps become failures

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# lib.sh is sourced for its test runner and assertions only (run_test / skip_test /
# assert_json_* / print_summary). Its `send_rpc` speaks to the dead Unix socket and
# is shadowed below.
source "$SCRIPT_DIR/lib.sh"

PARITY_STRICT="${PARITY_STRICT:-0}"
ALEPH_BRIDGE_BIN="${ALEPH_BRIDGE_BIN:-$REPO_ROOT/desktop/macos/bridge/.build/release/AlephBridge}"

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

check_deps jq

if ! is_macos; then
    echo "${YELLOW}Skipping: the Swift bridge is macOS-only.${RESET}"
    exit 0
fi

if [[ ! -x "$ALEPH_BRIDGE_BIN" ]]; then
    echo "${RED}ERROR: bridge helper not found at $ALEPH_BRIDGE_BIN${RESET}" >&2
    echo "Build it with \`just swift-bridge\`, or set ALEPH_BRIDGE_BIN." >&2
    exit 1
fi

echo "${BOLD}Bridge Acceptance Tests — Functional Parity${RESET}"
echo "Helper: $ALEPH_BRIDGE_BIN"

# ---------------------------------------------------------------------------
# Transport — JSON-RPC 2.0 over the helper's stdin/stdout
# ---------------------------------------------------------------------------

RPC_ID=0

# send_rpc — one request, one response. Shadows lib.sh's socket version.
#
# The helper reads newline-delimited requests from stdin and exits on EOF, so a
# process per call is the whole protocol. `id` must be a NUMBER: the Swift
# `Request` decodes it as UInt64, and a string id is a parse error.
send_rpc() {
    local method="$1"
    local params="${2:-"{}"}"
    RPC_ID=$((RPC_ID + 1))

    printf '{"jsonrpc":"2.0","id":%d,"method":"%s","params":%s}\n' \
        "$RPC_ID" "$method" "$params" \
        | "$ALEPH_BRIDGE_BIN" 2>/dev/null \
        | head -n 1
}

# rpc_error_code — the error code of a response, or empty when it succeeded.
rpc_error_code() {
    echo "$1" | jq -r '.error.code // empty' 2>/dev/null
}

# assert_rpc_ok — the call must have SUCCEEDED.
#
# Every error is a failure. -32601 and -32002 get their own message because they
# are the two that used to be silently swallowed.
assert_rpc_ok() {
    local resp="$1"
    local desc="${2:-rpc}"

    if [[ -z "$resp" ]]; then
        echo "FAIL: $desc — helper returned nothing (crashed?)" >&2
        return 1
    fi

    local code
    code=$(rpc_error_code "$resp")
    case "$code" in
        "")
            assert_json_has "$resp" ".result" "$desc result"
            ;;
        -32601)
            echo "FAIL: $desc — method not registered by the Router (-32601)" >&2
            return 1
            ;;
        -32002)
            echo "FAIL: $desc — method is a stub (-32002 not implemented)" >&2
            return 1
            ;;
        *)
            echo "FAIL: $desc — error $code: $(echo "$resp" | jq -r '.error.message')" >&2
            return 1
            ;;
    esac
}

# assert_rpc_rejects — the call must have been REJECTED with a specific code.
#
# Used for the contract tests: a targeted-rail call with no `pid` must be refused
# (-32602), not quietly redirected onto the global HID tap (which would drag the
# user's real cursor). "It refused correctly" is a real behaviour, and asserting it
# needs no side effects.
assert_rpc_rejects() {
    local resp="$1"
    local expected="$2"
    local desc="${3:-rpc}"

    local code
    code=$(rpc_error_code "$resp")

    if [[ -z "$code" ]]; then
        echo "FAIL: $desc — expected error $expected, but the call SUCCEEDED" >&2
        return 1
    fi
    if [[ "$code" == "$expected" ]]; then
        return 0
    fi
    # Not the expected rejection. -32601 gets its own message because it means the
    # method is not there at all — a contract test cannot "reject correctly" if
    # there is nothing behind the name to do the rejecting.
    if [[ "$code" == "-32601" ]]; then
        echo "FAIL: $desc — method not registered by the Router (-32601)" >&2
        return 1
    fi
    echo "FAIL: $desc — expected error $expected, got $code: $(echo "$resp" | jq -r '.error.message')" >&2
    return 1
}

# ---------------------------------------------------------------------------
# Permission probes
#
# A missing TCC grant makes the behavioural families unrunnable. It does NOT make
# them pass: the affected tests are skipped by name, the summary counts them as
# skipped, and PARITY_STRICT=1 turns them back into failures on a machine that is
# supposed to hold the grant.
# ---------------------------------------------------------------------------

# `|| true`: a helper that crashes must surface as a failing test below, not as a
# `set -e` abort with no output at all.
ax_probe=$(send_rpc "ax.query_focused" '{}' || true)
AX_TRUSTED=1
if [[ "$(rpc_error_code "$ax_probe")" == "-32001" ]]; then
    AX_TRUSTED=0
fi

screen_probe=$(send_rpc "screen.list_displays" '{}' || true)
SCREEN_TRUSTED=1
if [[ "$(rpc_error_code "$screen_probe")" == "-32001" ]]; then
    SCREEN_TRUSTED=0
fi

if [[ "$PARITY_STRICT" == "1" ]]; then
    AX_TRUSTED=1
    SCREEN_TRUSTED=1
fi

macos_major=$(sw_vers -productVersion | cut -d. -f1)

# ===================================================================
# F1: Handshake — the method registry
#
# The permission-free heart of the suite. `supported_methods` is what the Router
# actually registered, so this is the check that catches a method disappearing out
# from under a caller — which is exactly what had happened to `desktop.click`.
# ===================================================================

print_header "F1: Handshake + method registry"

# The computer-use surface, as declared in
# shared/protocol/src/desktop_bridge/methods/{bridge,ax,input,screen}.rs.
#
# input.clipboard_read / input.clipboard_write are declared in the protocol but
# deliberately NOT served by the helper (the clipboard is handled natively on the
# Rust side), so they are not required here.
REQUIRED_METHODS=(
    bridge.ping
    bridge.handshake
    ax.query_focused
    ax.query_tree
    ax.query_by_role
    ax.set_value
    ax.perform_action
    input.click
    input.double_click
    input.type_text
    input.key_combo
    input.scroll
    input.drag
    input.hover
    input.mouse_button
    input.cursor_position
    screen.capture
    screen.list_displays
)

HANDSHAKE=$(send_rpc "bridge.handshake" '{"rust_version":"acceptance","protocol_version":2}' || true)

test_f1_handshake() {
    assert_rpc_ok "$HANDSHAKE" "bridge.handshake"
    assert_json_eq "$HANDSHAKE" ".result.protocol_version" "2" "protocol version"
    assert_json_has "$HANDSHAKE" ".result.supported_methods" "supported_methods"
}

test_f1_registry() {
    local missing=()
    local method
    for method in "${REQUIRED_METHODS[@]}"; do
        if ! echo "$HANDSHAKE" | jq -e --arg m "$method" \
            '.result.supported_methods | index($m)' >/dev/null 2>&1; then
            missing+=("$method")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        echo "FAIL: the Router does not register: ${missing[*]}" >&2
        echo "  A caller invoking these gets -32601 at runtime." >&2
        return 1
    fi
}

test_f1_ping() {
    local resp
    resp=$(send_rpc "bridge.ping" '{}')
    assert_rpc_ok "$resp" "bridge.ping"
    assert_json_eq "$resp" ".result.pong" "true" "pong"
}

run_test "F1a: Handshake succeeds" test_f1_handshake
run_test "F1b: Router registers the full computer-use surface" test_f1_registry
run_test "F1c: Ping/pong" test_f1_ping

# ===================================================================
# F2: JSON-RPC error semantics
# ===================================================================

print_header "F2: JSON-RPC error semantics"

test_f2_unknown_method() {
    local resp
    resp=$(send_rpc "bridge.nonexistent_method" '{}')
    assert_rpc_rejects "$resp" "-32601" "unknown method"
}

test_f2_malformed_json() {
    local resp
    resp=$(printf 'this is not json\n' | "$ALEPH_BRIDGE_BIN" 2>/dev/null | head -n 1)
    local code
    code=$(rpc_error_code "$resp")
    if [[ "$code" != "-32700" ]]; then
        echo "FAIL: expected parse error -32700, got '${code:-none}' ($resp)" >&2
        return 1
    fi
}

run_test "F2a: Unknown method returns -32601" test_f2_unknown_method
run_test "F2b: Malformed JSON returns -32700" test_f2_malformed_json

# ===================================================================
# F3: Accessibility (ax.*)
# ===================================================================

print_header "F3: Accessibility (ax.*)"

test_f3_query_focused() {
    local resp
    resp=$(send_rpc "ax.query_focused" '{}')
    assert_rpc_ok "$resp" "ax.query_focused"
}

test_f3_query_tree() {
    local resp
    resp=$(send_rpc "ax.query_tree" '{"max_depth":3}')
    assert_rpc_ok "$resp" "ax.query_tree"
}

test_f3_query_by_role() {
    local resp
    resp=$(send_rpc "ax.query_by_role" '{"role":"AXButton"}')
    assert_rpc_ok "$resp" "ax.query_by_role"
    assert_json_has "$resp" ".result.elements" "elements array"
}

test_f3_locator_miss_is_an_error() {
    # A locator matching nothing must be refused, not answered with a cheerful
    # `performed: true` about an element that does not exist.
    local resp
    resp=$(send_rpc "ax.perform_action" \
        '{"locator":{"role":"AXButton","title":"aleph-no-such-element-3F9C"},"action":"AXPress"}')
    assert_rpc_rejects "$resp" "-32602" "perform_action on a locator that matches nothing"
}

if [[ "$AX_TRUSTED" == "1" ]]; then
    run_test "F3a: ax.query_focused" test_f3_query_focused
    run_test "F3b: ax.query_tree" test_f3_query_tree
    run_test "F3c: ax.query_by_role" test_f3_query_by_role
    run_test "F3d: perform_action rejects a locator that matches nothing" test_f3_locator_miss_is_an_error
else
    skip_test "F3a: ax.query_focused" "no Accessibility grant"
    skip_test "F3b: ax.query_tree" "no Accessibility grant"
    skip_test "F3c: ax.query_by_role" "no Accessibility grant"
    skip_test "F3d: perform_action rejects a locator that matches nothing" "no Accessibility grant"
fi

# ===================================================================
# F4: Input rail (input.*) — contracts, not actuation
#
# Nothing here moves the mouse or types. Each test asserts that the targeted rail
# REFUSES an under-specified request instead of quietly falling back to the global
# HID tap. Refusing IS the behaviour, and asserting it costs no side effects.
# ===================================================================

print_header "F4: Input rail (input.*)"

test_f4_cursor_position() {
    # The one input method that is read-only.
    local resp
    resp=$(send_rpc "input.cursor_position" '{}')
    assert_rpc_ok "$resp" "input.cursor_position"
    assert_json_has "$resp" ".result.x" "cursor x"
    assert_json_has "$resp" ".result.y" "cursor y"
}

test_f4_click_requires_pid() {
    local resp
    resp=$(send_rpc "input.click" '{"x":100,"y":100,"button":"left"}')
    assert_rpc_rejects "$resp" "-32602" "input.click without pid"
}

test_f4_type_text_requires_pid() {
    local resp
    resp=$(send_rpc "input.type_text" '{"text":"hello"}')
    assert_rpc_rejects "$resp" "-32602" "input.type_text without pid"
}

test_f4_scroll_requires_a_location() {
    # The targeted rail never moves the cursor, so the event's location is the only
    # thing the app can route a scroll by. A scroll with a pid but no point is
    # under-specified and must be refused.
    local resp
    resp=$(send_rpc "input.scroll" '{"direction":"down","amount":3,"pid":1}')
    assert_rpc_rejects "$resp" "-32602" "input.scroll without x/y"
}

test_f4_key_combo_rejects_unknown_modifier() {
    local resp
    resp=$(send_rpc "input.key_combo" '{"modifiers":["hyper"],"key":"c","pid":1}')
    assert_rpc_rejects "$resp" "-32602" "input.key_combo with an unknown modifier"
}

test_f4_scroll_rejects_unknown_direction() {
    local resp
    resp=$(send_rpc "input.scroll" '{"direction":"sideways","amount":3,"pid":1,"x":10,"y":10}')
    assert_rpc_rejects "$resp" "-32602" "input.scroll with an unknown direction"
}

if [[ "$AX_TRUSTED" == "1" ]]; then
    run_test "F4a: input.cursor_position reads the cursor" test_f4_cursor_position
    run_test "F4b: input.click refuses to run without a pid" test_f4_click_requires_pid
    run_test "F4c: input.type_text refuses to run without a pid" test_f4_type_text_requires_pid
    run_test "F4d: input.scroll refuses to run without a location" test_f4_scroll_requires_a_location
    run_test "F4e: input.key_combo rejects an unknown modifier" test_f4_key_combo_rejects_unknown_modifier
    run_test "F4f: input.scroll rejects an unknown direction" test_f4_scroll_rejects_unknown_direction
else
    skip_test "F4a: input.cursor_position reads the cursor" "no Accessibility grant"
    skip_test "F4b: input.click refuses to run without a pid" "no Accessibility grant"
    skip_test "F4c: input.type_text refuses to run without a pid" "no Accessibility grant"
    skip_test "F4d: input.scroll refuses to run without a location" "no Accessibility grant"
    skip_test "F4e: input.key_combo rejects an unknown modifier" "no Accessibility grant"
    skip_test "F4f: input.scroll rejects an unknown direction" "no Accessibility grant"
fi

# ===================================================================
# F5: Screen capture (screen.*)
# ===================================================================

print_header "F5: Screen capture (screen.*)"

test_f5_list_displays() {
    local resp
    resp=$(send_rpc "screen.list_displays" '{}')
    assert_rpc_ok "$resp" "screen.list_displays"
    local count
    count=$(echo "$resp" | jq '.result.displays | length' 2>/dev/null)
    if [[ -z "$count" || "$count" -lt 1 ]]; then
        echo "FAIL: no displays reported" >&2
        return 1
    fi
}

test_f5_capture_returns_a_real_image() {
    local resp
    resp=$(send_rpc "screen.capture" '{}')
    assert_rpc_ok "$resp" "screen.capture"
    assert_json_has "$resp" ".result.png_base64" "png payload"

    local width height png_len
    width=$(echo "$resp" | jq -r '.result.width // 0')
    height=$(echo "$resp" | jq -r '.result.height // 0')
    png_len=$(echo "$resp" | jq -r '.result.png_base64 | length')

    if [[ "$width" -le 0 || "$height" -le 0 ]]; then
        echo "FAIL: degenerate capture dimensions (${width}x${height})" >&2
        return 1
    fi
    # A base64 PNG of a real screen is tens of KB. Anything tiny is an empty or 1x1
    # frame dressed up as a success.
    if [[ "$png_len" -lt 1000 ]]; then
        echo "FAIL: png_base64 is only $png_len chars — not a real frame" >&2
        return 1
    fi
}

test_f5_capture_rejects_window_id_with_region() {
    # A region is a rectangle in DISPLAY coordinates; it is meaningless against a
    # window-scoped filter. The two must not be silently combined.
    local resp
    resp=$(send_rpc "screen.capture" '{"window_id":1,"region":{"x":0,"y":0,"width":10,"height":10}}')
    assert_rpc_rejects "$resp" "-32602" "screen.capture with both window_id and region"
}

if [[ "$SCREEN_TRUSTED" != "1" ]]; then
    skip_test "F5a: screen.list_displays" "no Screen Recording grant"
    skip_test "F5b: screen.capture returns a real frame" "no Screen Recording grant"
    skip_test "F5c: screen.capture rejects window_id + region" "no Screen Recording grant"
elif [[ "$macos_major" -lt 14 ]]; then
    # SCScreenshotManager is macOS 14+; the helper says so with -32601 and the Rust
    # caller falls back to xcap. Not a parity failure on this OS.
    skip_test "F5a: screen.list_displays" "macOS 14+ required"
    skip_test "F5b: screen.capture returns a real frame" "macOS 14+ required"
    skip_test "F5c: screen.capture rejects window_id + region" "macOS 14+ required"
else
    run_test "F5a: screen.list_displays" test_f5_list_displays
    run_test "F5b: screen.capture returns a real frame" test_f5_capture_returns_a_real_image
    run_test "F5c: screen.capture rejects window_id + region" test_f5_capture_rejects_window_id_with_region
fi

# ===================================================================
# Summary
# ===================================================================

if [[ $TESTS_SKIPPED -gt 0 ]]; then
    echo ""
    echo "${YELLOW}${TESTS_SKIPPED} test(s) were SKIPPED, not passed.${RESET}"
    echo "Grant the helper Accessibility / Screen Recording and re-run, or set"
    echo "PARITY_STRICT=1 to make a missing grant a failure."
fi

print_summary
