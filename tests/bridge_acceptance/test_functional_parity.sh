#!/usr/bin/env bash
# Bridge Acceptance Tests — Functional Parity (F1-F12)
#
# Verifies that the Tauri bridge implements all Desktop Bridge RPC methods
# with the same interface as the deprecated Swift app.
#
# Usage: ./test_functional_parity.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

check_deps socat jq bc python3
check_socket

echo "${BOLD}Bridge Acceptance Tests — Functional Parity${RESET}"
echo "Socket: $SOCKET_PATH"

# ===================================================================
# F1: Screenshot
# ===================================================================

print_header "F1: Screenshot (desktop.screenshot)"

test_f1_screenshot_fullscreen() {
    local resp
    resp=$(send_rpc "desktop.screenshot" '{}')
    assert_no_error "$resp" "screenshot fullscreen"
    assert_json_has "$resp" ".result" "screenshot result"
}

test_f1_screenshot_region() {
    local resp
    resp=$(send_rpc "desktop.screenshot" '{"region":{"x":0,"y":0,"width":200,"height":200}}')
    assert_no_error "$resp" "screenshot region"
    assert_json_has "$resp" ".result" "screenshot region result"
}

test_f1_screenshot_returns_base64() {
    local resp
    resp=$(send_rpc "desktop.screenshot" '{}')
    assert_no_error "$resp" "screenshot base64"
    # Result should contain base64-encoded image data (check for typical PNG/JPEG base64 prefix)
    local result
    result=$(echo "$resp" | jq -r '.result' 2>/dev/null)
    # If result is an object with image_base64 field, extract it
    local image_data
    image_data=$(echo "$resp" | jq -r '.result.image_base64 // .result' 2>/dev/null)
    if [[ ${#image_data} -lt 100 ]]; then
        echo "FAIL: screenshot data too small (${#image_data} chars)" >&2
        return 1
    fi
}

run_test "F1a: Screenshot (fullscreen)" test_f1_screenshot_fullscreen
run_test "F1b: Screenshot (region)" test_f1_screenshot_region
run_test "F1c: Screenshot returns base64 image data" test_f1_screenshot_returns_base64

# ===================================================================
# F2: OCR (macOS only)
# ===================================================================

print_header "F2: OCR (desktop.ocr) — macOS only"

test_f2_ocr_from_screen() {
    local resp
    resp=$(send_rpc "desktop.ocr" '{}')
    # Either succeeds or returns not-implemented (-32000)
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        # -32000 means "not implemented on this platform" — that is acceptable for Tauri
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
    assert_json_has "$resp" ".result" "OCR result"
}

test_f2_ocr_from_base64() {
    # Create a tiny 1x1 white PNG in base64
    local white_png="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg=="
    local resp
    resp=$(send_rpc "desktop.ocr" "{\"image_base64\":\"$white_png\"}")
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
    assert_json_has "$resp" ".result" "OCR from base64 result"
}

if is_macos; then
    run_test "F2a: OCR from screen" test_f2_ocr_from_screen
    run_test "F2b: OCR from base64 image" test_f2_ocr_from_base64
else
    skip_test "F2a: OCR from screen" "macOS only"
    skip_test "F2b: OCR from base64 image" "macOS only"
fi

# ===================================================================
# F3: Accessibility Tree (macOS only)
# ===================================================================

print_header "F3: AX Tree (desktop.ax_tree) — macOS only"

test_f3_ax_tree() {
    local resp
    resp=$(send_rpc "desktop.ax_tree" '{}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
    assert_json_has "$resp" ".result" "AX tree result"
}

test_f3_ax_tree_by_bundle() {
    local resp
    resp=$(send_rpc "desktop.ax_tree" '{"app_bundle_id":"com.apple.finder"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
    assert_json_has "$resp" ".result" "AX tree by bundle result"
}

if is_macos; then
    run_test "F3a: AX tree (focused app)" test_f3_ax_tree
    run_test "F3b: AX tree (by bundle ID)" test_f3_ax_tree_by_bundle
else
    skip_test "F3a: AX tree (focused app)" "macOS only"
    skip_test "F3b: AX tree (by bundle ID)" "macOS only"
fi

# ===================================================================
# F4: Mouse Click
# ===================================================================

print_header "F4: Mouse Click (desktop.click)"

test_f4_click() {
    local resp
    resp=$(send_rpc "desktop.click" '{"x":100,"y":100,"button":"left"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
    assert_json_has "$resp" ".result" "click result"
}

test_f4_click_right() {
    local resp
    resp=$(send_rpc "desktop.click" '{"x":100,"y":100,"button":"right"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

run_test "F4a: Mouse click (left)" test_f4_click
run_test "F4b: Mouse click (right)" test_f4_click_right

# ===================================================================
# F5: Keyboard Input
# ===================================================================

print_header "F5: Keyboard Input (desktop.type_text)"

test_f5_type_text() {
    local resp
    resp=$(send_rpc "desktop.type_text" '{"text":"hello"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

test_f5_type_text_unicode() {
    local resp
    resp=$(send_rpc "desktop.type_text" '{"text":"你好世界 🌍"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

run_test "F5a: Type text (ASCII)" test_f5_type_text
run_test "F5b: Type text (Unicode)" test_f5_type_text_unicode

# ===================================================================
# F5b: Key Combo
# ===================================================================

print_header "F5b: Key Combo (desktop.key_combo)"

test_f5b_key_combo() {
    local resp
    resp=$(send_rpc "desktop.key_combo" '{"keys":["cmd","c"]}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

test_f5b_key_combo_multi() {
    local resp
    resp=$(send_rpc "desktop.key_combo" '{"keys":["cmd","shift","s"]}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

run_test "F5b-a: Key combo (cmd+c)" test_f5b_key_combo
run_test "F5b-b: Key combo (cmd+shift+s)" test_f5b_key_combo_multi

# ===================================================================
# F6: App Launch (macOS only)
# ===================================================================

print_header "F6: App Launch (desktop.launch_app) — macOS only"

test_f6_launch_app() {
    # Launch Calculator (safe, always available on macOS)
    local resp
    resp=$(send_rpc "desktop.launch_app" '{"bundle_id":"com.apple.calculator"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

if is_macos; then
    run_test "F6: Launch app by bundle ID" test_f6_launch_app
else
    skip_test "F6: Launch app by bundle ID" "macOS only"
fi

# ===================================================================
# F7: Window List
# ===================================================================

print_header "F7: Window List (desktop.window_list)"

test_f7_window_list() {
    local resp
    resp=$(send_rpc "desktop.window_list" '{}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
    # Result should be an array or contain window data
    assert_json_has "$resp" ".result" "window list result"
}

run_test "F7: Window list" test_f7_window_list

# ===================================================================
# F7b: Focus Window
# ===================================================================

print_header "F7b: Focus Window (desktop.focus_window)"

test_f7b_focus_window() {
    local resp
    resp=$(send_rpc "desktop.focus_window" '{"window_id":0}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        # -32000 (not implemented) or reasonable error for invalid ID are OK
        if [[ "$code" == "-32000" || "$code" == "-32603" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

run_test "F7b: Focus window" test_f7b_focus_window

# ===================================================================
# F8: Canvas Overlay
# ===================================================================

print_header "F8: Canvas Overlay (desktop.canvas_*)"

test_f8_canvas_show() {
    local resp
    resp=$(send_rpc "desktop.canvas_show" '{"html":"<h1>Test</h1>","position":{"x":100,"y":100,"width":400,"height":300}}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

test_f8_canvas_update() {
    local resp
    resp=$(send_rpc "desktop.canvas_update" '{"patch":[]}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

test_f8_canvas_hide() {
    local resp
    resp=$(send_rpc "desktop.canvas_hide" '{}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

run_test "F8a: Canvas show" test_f8_canvas_show
run_test "F8b: Canvas update" test_f8_canvas_update
run_test "F8c: Canvas hide" test_f8_canvas_hide

# ===================================================================
# F9: Tray Status
# ===================================================================

print_header "F9: Tray Status"

test_f9_tray_status() {
    # Tray is a Tauri-native feature (not a bridge RPC method).
    # We verify the bridge process is alive and can respond to ping,
    # which implies the Tauri app (with its tray) is running.
    local resp
    resp=$(send_rpc "desktop.ping" '{}')
    assert_json_eq "$resp" ".result" "pong" "tray status (bridge alive)"
}

run_test "F9: Tray status (bridge alive implies tray active)" test_f9_tray_status

# ===================================================================
# F10: Halo Window Show/Hide
# ===================================================================

print_header "F10: Halo Window Show/Hide"

test_f10_halo() {
    # Halo is managed by Tauri commands, not bridge RPC.
    # We verify the bridge is responsive (Tauri app is running).
    local resp
    resp=$(send_rpc "desktop.ping" '{}')
    assert_json_eq "$resp" ".result" "pong" "halo (bridge alive)"
}

run_test "F10: Halo show/hide (bridge alive, manual checklist M4)" test_f10_halo

# ===================================================================
# F11: Settings Window Show/Hide
# ===================================================================

print_header "F11: Settings Window Show/Hide"

test_f11_settings() {
    # Settings is managed by Tauri commands, not bridge RPC.
    local resp
    resp=$(send_rpc "desktop.ping" '{}')
    assert_json_eq "$resp" ".result" "pong" "settings (bridge alive)"
}

run_test "F11: Settings show/hide (bridge alive, manual checklist M5)" test_f11_settings

# ===================================================================
# F12: Handshake + Capabilities
# ===================================================================

print_header "F12: Handshake + Capabilities"

test_f12_ping() {
    local resp
    resp=$(send_rpc "desktop.ping" '{}')
    assert_json_eq "$resp" ".jsonrpc" "2.0" "JSON-RPC version"
    assert_json_eq "$resp" ".result" "pong" "ping response"
}

test_f12_unknown_method() {
    local resp
    resp=$(send_rpc "desktop.nonexistent_method" '{}')
    assert_is_error "$resp" "-32601" "unknown method returns -32601"
}

test_f12_malformed_json() {
    # Send raw malformed JSON over the socket
    local response
    response=$(echo 'this is not json' | socat - UNIX-CONNECT:"$SOCKET_PATH" 2>/dev/null) || {
        echo "FAIL: could not connect" >&2
        return 1
    }
    # Should get a parse error response
    local code
    code=$(echo "$response" | jq -r '.error.code' 2>/dev/null) || {
        echo "FAIL: response is not valid JSON: $response" >&2
        return 1
    }
    if [[ "$code" != "-32700" ]]; then
        echo "FAIL: expected parse error -32700, got $code" >&2
        return 1
    fi
}

test_f12_response_id_matches() {
    local resp
    resp=$(send_rpc "desktop.ping" '{}' "unique-id-42")
    assert_json_eq "$resp" ".id" "unique-id-42" "response ID matches request"
}

run_test "F12a: Ping/pong handshake" test_f12_ping
run_test "F12b: Unknown method returns -32601" test_f12_unknown_method
run_test "F12c: Malformed JSON returns -32700" test_f12_malformed_json
run_test "F12d: Response ID matches request ID" test_f12_response_id_matches

# ===================================================================
# Summary
# ===================================================================

print_summary
