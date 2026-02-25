# Fix Bridge Known Issues — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix screenshot/OCR silent failures, add request timeouts, add permission pre-checks, and fix socat half-close in test suite.

**Architecture:** Three targeted changes to the bridge's `handle_connection` (timeout + spawn_blocking), `perception.rs` (permission pre-check), and test `lib.sh` (socat parameters). No dispatcher restructuring — the existing `dispatch()` function signature stays the same.

**Tech Stack:** Rust (tokio, core-graphics FFI), Bash (socat)

---

### Task 1: Fix socat half-close in test lib.sh

**Files:**
- Modify: `tests/bridge_acceptance/lib.sh:102` (send_rpc socat call)

**Context:** When `echo "..." | socat - UNIX-CONNECT:...` is used, socat detects stdin EOF after echo completes and may close the socket before the bridge responds to slow operations (screenshot, OCR). The `-t` flag controls the half-close timeout — how long socat keeps reading after the write side reaches EOF.

**Step 1: Edit send_rpc socat parameters**

In `tests/bridge_acceptance/lib.sh`, line 102, change:

```bash
# Old (line 102):
    response=$(echo "$request" | socat -T10 - UNIX-CONNECT:"$SOCKET_PATH" 2>/dev/null) || {

# New:
    response=$(echo "$request" | socat -t10 -T15 - UNIX-CONNECT:"$SOCKET_PATH" 2>/dev/null) || {
```

Explanation:
- `-t10` = after stdin EOF, keep reading from socket for 10 more seconds (half-close timeout)
- `-T15` = total inactivity timeout of 15 seconds (safety net)

**Step 2: Verify the fix works**

Run:
```bash
source tests/bridge_acceptance/lib.sh
result=$(send_rpc "desktop.ping" '{}' "socat-test")
echo "$result"
```
Expected: `{"jsonrpc":"2.0","id":"socat-test","result":"pong"}`

**Step 3: Commit**

```bash
git add tests/bridge_acceptance/lib.sh
git commit -m "test: fix socat half-close in bridge acceptance tests

Use -t10 (half-close timeout) and -T15 (total inactivity timeout)
so socat keeps reading the response after stdin EOF."
```

---

### Task 2: Add Screen Recording permission pre-check

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge/perception.rs:9-12` (add imports)
- Modify: `apps/desktop/src-tauri/src/bridge/perception.rs:25` (add check in handle_screenshot)
- Modify: `apps/desktop/src-tauri/src/bridge/perception.rs:77` (add check in handle_ocr)
- Modify: `apps/desktop/src-tauri/src/bridge/perception.rs:109` (add check in capture_screen_png)

**Context:** The archived Swift code used `CGPreflightScreenCaptureAccess()` to check Screen Recording permission before capture. The current Tauri bridge skips this, causing xcap to fail silently on some systems. The function is a C FFI from CoreGraphics.framework — it returns `true` if Screen Recording is granted, `false` otherwise. No user prompt is shown.

**Step 1: Add the permission check function**

At the top of `perception.rs`, after the existing `use` statements (line 12), add:

```rust
// -- Add after line 12 --

/// Check if Screen Recording permission is granted (macOS only).
/// Returns `true` on non-macOS platforms (no permission needed).
#[cfg(target_os = "macos")]
fn screen_recording_granted() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

#[cfg(not(target_os = "macos"))]
fn screen_recording_granted() -> bool {
    true
}
```

**Step 2: Add check to handle_screenshot**

At the start of `handle_screenshot()` (line 26, after the opening `{`), add:

```rust
    if !screen_recording_granted() {
        return Err((ERR_INTERNAL,
            "Screen Recording permission not granted. \
             Enable in System Settings > Privacy & Security > Screen Recording.".into()));
    }
```

**Step 3: Add check to capture_screen_png**

At the start of `capture_screen_png()` (line 110, after the opening `{`), add the same check:

```rust
    if !screen_recording_granted() {
        return Err((ERR_INTERNAL,
            "Screen Recording permission not granted. \
             Enable in System Settings > Privacy & Security > Screen Recording.".into()));
    }
```

This covers OCR when it captures from screen (not when processing a provided base64 image).

**Step 4: Verify it compiles**

Run:
```bash
cargo build -p aleph-tauri 2>&1 | tail -5
```
Expected: `Finished` with no errors (warnings OK).

**Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/bridge/perception.rs
git commit -m "bridge: add Screen Recording permission pre-check

Check CGPreflightScreenCaptureAccess() before screenshot/OCR
capture on macOS. Returns clear JSON-RPC error instead of
silent failure when permission is not granted."
```

---

### Task 3: Add request timeout and spawn_blocking to dispatch

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs:12-21` (add imports)
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs:86-120` (rewrite handle_connection)

**Context:** The `dispatch()` function is synchronous and calls blocking system APIs (xcap screen capture, enigo input, objc FFI for OCR/AX tree). Running these on the tokio async runtime thread can starve other tasks. Wrapping in `spawn_blocking` moves them to a dedicated thread pool AND gives us implicit panic safety (panics in `spawn_blocking` become `JoinError` instead of crashing the server). Adding `tokio::time::timeout` ensures no handler blocks forever.

**Step 1: Add imports**

In `mod.rs`, add `use std::time::Duration;` to the imports section. The exact edit — after line 21 (`use tracing::{error, info, warn};`), add:

```rust
use std::time::Duration;
```

**Step 2: Rewrite handle_connection**

Replace the entire `handle_connection` function (lines 86-120) with:

```rust
async fn handle_connection(stream: tokio::net::UnixStream) {
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    // Read request with 5s timeout (prevents idle connections from accumulating)
    let read_result = tokio::time::timeout(
        Duration::from_secs(5),
        buf_reader.read_line(&mut line),
    )
    .await;

    match read_result {
        Ok(Ok(0)) | Err(_) => return,   // EOF or read timeout
        Ok(Err(e)) => {
            tracing::debug!("Failed to read from client: {}", e);
            return;
        }
        Ok(Ok(_)) => {}
    }

    let line = line.trim_end();
    if line.is_empty() {
        return;
    }

    let response = match protocol::parse_request(line) {
        Ok(req) => {
            let method = req.method.clone();
            let params = req.params.unwrap_or(json!({}));
            let id = req.id.clone();

            // Run dispatch on blocking thread pool with 30s timeout.
            // spawn_blocking also catches panics (returns JoinError).
            let result = tokio::time::timeout(
                Duration::from_secs(30),
                tokio::task::spawn_blocking(move || dispatch(&method, params)),
            )
            .await;

            match result {
                Ok(Ok(Ok(value))) => protocol::success_response(&id, value),
                Ok(Ok(Err((code, msg)))) => protocol::error_response(&id, code, &msg),
                Ok(Err(join_err)) => protocol::error_response(
                    &id,
                    ERR_INTERNAL,
                    &format!("Handler panicked: {join_err}"),
                ),
                Err(_) => protocol::error_response(
                    &id,
                    ERR_INTERNAL,
                    "Request timed out after 30s",
                ),
            }
        }
        Err(err_resp) => serde_json::to_string(&err_resp).unwrap_or_default(),
    };

    let response_line = format!("{}\n", response);
    if let Err(e) = writer.write_all(response_line.as_bytes()).await {
        tracing::debug!("Failed to write response to client: {}", e);
    }
}
```

Key changes:
1. `read_line` wrapped in `tokio::time::timeout(5s)` — prevents idle connections
2. `dispatch` runs inside `spawn_blocking` — moves blocking I/O off async runtime
3. `spawn_blocking` result wrapped in `tokio::time::timeout(30s)` — no handler blocks forever
4. `spawn_blocking` catches panics — `JoinError` becomes a clean JSON-RPC error
5. Clone `req.method`, `req.params`, `req.id` before the `move` closure (required for ownership)

**Step 3: Verify it compiles**

Run:
```bash
cargo build -p aleph-tauri 2>&1 | tail -5
```
Expected: `Finished` with no errors.

**Step 4: Run existing Rust tests**

Run:
```bash
cargo test -p aleph-tauri 2>&1 | tail -10
```
Expected: All tests pass (or `0 tests` if there are no unit tests in this crate).

**Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/bridge/mod.rs
git commit -m "bridge: add 30s request timeout and spawn_blocking for dispatch

- Wrap dispatch() in spawn_blocking to move blocking I/O (xcap,
  enigo, objc FFI) off the async runtime thread
- Add 30s timeout so no handler blocks forever
- Add 5s read timeout to prevent idle connection accumulation
- spawn_blocking catches panics as JoinError → clean JSON-RPC error"
```

---

### Task 4: Run acceptance tests to verify fixes

**Files:**
- Read: `tests/bridge_acceptance/run_all.sh`
- Modify: `docs/plans/2026-02-25-macos-app-removal-record.md` (update results)

**Context:** The bridge needs to be rebuilt and restarted before testing. The acceptance tests verify that the fixes actually work — screenshot/OCR should now return a proper JSON-RPC error instead of an empty response.

**Step 1: Rebuild the bridge**

```bash
cargo build -p aleph-tauri 2>&1 | tail -5
```
Expected: `Finished`

**Step 2: Kill any running bridge and restart**

```bash
pkill -f aleph-tauri || true
rm -f "$HOME/.aleph/bridge.sock"
sleep 1
ALEPH_SOCKET_PATH="$HOME/.aleph/bridge.sock" /path/to/target/debug/aleph-tauri &
sleep 2
# Verify it's running
echo '{"jsonrpc":"2.0","id":"1","method":"desktop.ping","params":{}}' | \
    socat -t5 -T10 - UNIX-CONNECT:"$HOME/.aleph/bridge.sock"
```
Expected: `{"jsonrpc":"2.0","id":"1","result":"pong"}`

**Step 3: Test that screenshot returns a proper error (not empty)**

```bash
echo '{"jsonrpc":"2.0","id":"2","method":"desktop.screenshot","params":{}}' | \
    socat -t10 -T15 - UNIX-CONNECT:"$HOME/.aleph/bridge.sock"
```
Expected: A JSON-RPC error response like:
```json
{"jsonrpc":"2.0","id":"2","error":{"code":-32603,"message":"Screen Recording permission not granted. Enable in System Settings > Privacy & Security > Screen Recording."}}
```
**NOT** an empty response.

**Step 4: Run the full functional parity test suite**

```bash
ALEPH_SOCKET_PATH="$HOME/.aleph/bridge.sock" bash tests/bridge_acceptance/test_functional_parity.sh
```

Expected results:
- F1a-d (screenshot): FAIL with proper error (not empty response) — OR PASS if Screen Recording is granted
- F2a-b (OCR): Same — FAIL with proper error or PASS
- F3, F4, F7: Should now PASS (no longer affected by screenshot silent failures)
- F5, F6, F8-F12: PASS (unchanged)

**Step 5: Run performance and stability tests**

```bash
ALEPH_SOCKET_PATH="$HOME/.aleph/bridge.sock" bash tests/bridge_acceptance/test_performance.sh
ALEPH_SOCKET_PATH="$HOME/.aleph/bridge.sock" \
    STABILITY_LONG_RUN_SECS=30 STABILITY_HOTKEY_CYCLES=20 \
    STABILITY_UDS_CYCLES=10 STABILITY_STRESS_DURATION_SECS=10 \
    STABILITY_STRESS_RPS=5 \
    bash tests/bridge_acceptance/test_stability.sh
```
Expected: All pass.

**Step 6: Update migration record with new results**

Update `docs/plans/2026-02-25-macos-app-removal-record.md` with the new test results.

**Step 7: Commit**

```bash
git add docs/plans/2026-02-25-macos-app-removal-record.md
git commit -m "docs: update acceptance results after bridge fixes"
```
