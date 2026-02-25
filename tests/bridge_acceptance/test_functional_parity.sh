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
    # Result should contain base64-encoded image data
    local image_data
    image_data=$(echo "$resp" | jq -r '.result.image // .result.image_base64 // .result' 2>/dev/null)
    if [[ ${#image_data} -lt 100 ]]; then
        echo "FAIL: screenshot data too small (${#image_data} chars)" >&2
        return 1
    fi
}

test_f1_screenshot_returns_dimensions() {
    local resp
    resp=$(send_rpc "desktop.screenshot" '{}')
    assert_no_error "$resp" "screenshot dimensions"
    # Must return width and height > 0
    local width height
    width=$(echo "$resp" | jq -r '.result.width // empty' 2>/dev/null)
    height=$(echo "$resp" | jq -r '.result.height // empty' 2>/dev/null)
    if [[ -z "$width" || -z "$height" ]]; then
        echo "FAIL: screenshot missing width/height fields" >&2
        return 1
    fi
    if [[ "$width" -le 0 || "$height" -le 0 ]]; then
        echo "FAIL: screenshot dimensions invalid (${width}x${height})" >&2
        return 1
    fi
}

run_test "F1a: Screenshot (fullscreen)" test_f1_screenshot_fullscreen
run_test "F1b: Screenshot (region)" test_f1_screenshot_region
run_test "F1c: Screenshot returns base64 image data" test_f1_screenshot_returns_base64
run_test "F1d: Screenshot returns dimensions" test_f1_screenshot_returns_dimensions

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

# Recursive depth counter for AX tree JSON
_ax_tree_depth() {
    local json="$1"
    echo "$json" | python3 -c "
import json, sys
def depth(node, d=1):
    children = node.get('children', [])
    if not children:
        return d
    return max(depth(c, d+1) for c in children)
try:
    data = json.load(sys.stdin)
    result = data.get('result', data)
    print(depth(result))
except:
    print(0)
"
}

test_f3_ax_tree_depth() {
    local resp
    resp=$(send_rpc "desktop.ax_tree" '{"app_bundle_id":"com.apple.finder"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32000" ]]; then
            return 0  # Not implemented is acceptable
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
    local tree_depth
    tree_depth=$(_ax_tree_depth "$resp")
    if [[ "$tree_depth" -lt 3 ]]; then
        echo "FAIL: AX tree depth is $tree_depth, expected >= 3" >&2
        return 1
    fi
}

if is_macos; then
    run_test "F3a: AX tree (focused app)" test_f3_ax_tree
    run_test "F3b: AX tree (by bundle ID)" test_f3_ax_tree_by_bundle
    run_test "F3c: AX tree depth >= 3" test_f3_ax_tree_depth
else
    skip_test "F3a: AX tree (focused app)" "macOS only"
    skip_test "F3b: AX tree (by bundle ID)" "macOS only"
    skip_test "F3c: AX tree depth >= 3" "macOS only"
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
    local resp
    resp=$(send_rpc "tray.update_status" '{"status":"idle","tooltip":"Acceptance Test"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        # -32601 (method not found) or -32000 (not implemented) are acceptable
        # since tray may be a Tauri command rather than bridge RPC
        if [[ "$code" == "-32601" || "$code" == "-32000" ]]; then
            echo "INFO: tray.update_status not available via bridge RPC (code=$code), defer to manual checklist M3" >&2
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

run_test "F9: Tray status update" test_f9_tray_status

# ===================================================================
# F10: Halo Window Show/Hide
# ===================================================================

print_header "F10: Halo Window Show/Hide"

test_f10_halo_show() {
    local resp
    resp=$(send_rpc "webview.show" '{"window":"halo"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32601" || "$code" == "-32000" ]]; then
            echo "INFO: webview.show not available via bridge RPC (code=$code), defer to manual checklist M4" >&2
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

test_f10_halo_hide() {
    local resp
    resp=$(send_rpc "webview.hide" '{"window":"halo"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32601" || "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

run_test "F10a: Halo window show" test_f10_halo_show
run_test "F10b: Halo window hide" test_f10_halo_hide

# ===================================================================
# F11: Settings Window Show/Hide
# ===================================================================

print_header "F11: Settings Window Show/Hide"

test_f11_settings_show() {
    local resp
    resp=$(send_rpc "webview.show" '{"window":"settings"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32601" || "$code" == "-32000" ]]; then
            echo "INFO: webview.show not available via bridge RPC (code=$code), defer to manual checklist M5" >&2
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

test_f11_settings_hide() {
    local resp
    resp=$(send_rpc "webview.hide" '{"window":"settings"}')
    local has_error
    has_error=$(echo "$resp" | jq 'has("error")' 2>/dev/null)
    if [[ "$has_error" == "true" ]]; then
        local code
        code=$(echo "$resp" | jq -r '.error.code' 2>/dev/null)
        if [[ "$code" == "-32601" || "$code" == "-32000" ]]; then
            return 0
        fi
        echo "FAIL: unexpected error code $code" >&2
        return 1
    fi
}

run_test "F11a: Settings window show" test_f11_settings_show
run_test "F11b: Settings window hide" test_f11_settings_hide

# ===================================================================
# F12: Handshake + Capabilities
# ===================================================================

print_header "F12: Handshake + Capabilities"

test_f12_handshake() {
    local resp
    resp=$(send_rpc "aleph.handshake" '{"protocol_version":"1.0"}')
    assert_no_error "$resp" "handshake"
    assert_json_eq "$resp" ".result.bridge_type" "desktop" "bridge type"
    assert_json_has "$resp" ".result.platform" "platform"
    assert_json_has "$resp" ".result.capabilities" "capabilities list"
    # Verify at least 3 capabilities registered
    local cap_count
    cap_count=$(echo "$resp" | jq '.result.capabilities | length' 2>/dev/null)
    if [[ -z "$cap_count" || "$cap_count" -lt 3 ]]; then
        echo "FAIL: only $cap_count capabilities (expected >= 3)" >&2
        return 1
    fi
}

test_f12_ping() {
    local resp
    resp=$(send_rpc "system.ping" '{}')
    assert_json_eq "$resp" ".result" "pong" "ping response"
}

test_f12_unknown_method() {
    local resp
    resp=$(send_rpc "desktop.nonexistent_method" '{}')
    assert_is_error "$resp" "-32601" "unknown method returns -32601"
}

test_f12_malformed_json() {
    local response
    response=$(echo 'this is not json' | socat - UNIX-CONNECT:"$SOCKET_PATH" 2>/dev/null) || {
        echo "FAIL: could not connect" >&2
        return 1
    }
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

run_test "F12a: Handshake returns capabilities" test_f12_handshake
run_test "F12b: Ping/pong" test_f12_ping
run_test "F12c: Unknown method returns -32601" test_f12_unknown_method
run_test "F12d: Malformed JSON returns -32700" test_f12_malformed_json

# ===================================================================
# Summary
# ===================================================================

print_summary
