# Fix Bridge Known Issues — Design

## Goal

Fix the known issues discovered during bridge acceptance testing: screenshot/OCR silent failures, request timeouts, and test socat half-close behavior.

## Root Cause Analysis

### Issue 1: Empty responses for Screenshot/OCR

**Symptom**: `send_rpc "desktop.screenshot"` returns empty response instead of JSON-RPC error.

**Root cause**: socat's half-close behavior. When `echo "..." | socat` is used, echo completes → stdin EOF → socat shuts down the socket's write side. If the bridge handler is slow (screenshot, OCR), by the time it responds, the socket is half-closed. The bridge's `writer.write_all()` may fail silently, and the response is lost.

**Contributing factor**: No request timeout in bridge. `xcap::Monitor::capture_image()` blocks synchronously with no timeout. If Screen Recording permission is missing, the call may hang or return slowly.

### Issue 2: Subsequent requests failing after Screenshot/OCR

**Symptom**: AX tree, click, window list work individually but fail when run after screenshot tests.

**Root cause**: The bridge's async handler for a failed screenshot may still be holding resources when subsequent connections arrive. Without timeouts, stale handlers accumulate.

## Design

### Bridge Side (3 changes)

#### 1. Unified Request Timeout (mod.rs)

Wrap `dispatch()` in `tokio::time::timeout(Duration::from_secs(30))`. Since `dispatch()` is currently synchronous and calls blocking system APIs (xcap, enigo), use `tokio::task::spawn_blocking` to run it off the async runtime thread.

```rust
// handle_connection()
let result = tokio::time::timeout(
    Duration::from_secs(30),
    tokio::task::spawn_blocking(move || dispatch(&method, params))
).await;

let response = match result {
    Ok(Ok(Ok(value))) => protocol::success_response(&req.id, value),
    Ok(Ok(Err((code, msg)))) => protocol::error_response(&req.id, code, &msg),
    Ok(Err(join_err)) => protocol::error_response(&req.id, ERR_INTERNAL, &format!("Handler panicked: {join_err}")),
    Err(_) => protocol::error_response(&req.id, ERR_INTERNAL, "Request timed out after 30s"),
};
```

This also provides implicit panic safety: `spawn_blocking` catches panics and returns `JoinError`.

#### 2. Screen Recording Permission Pre-check (perception.rs)

Add `CGPreflightScreenCaptureAccess()` check before screenshot/OCR on macOS:

```rust
#[cfg(target_os = "macos")]
fn screen_recording_granted() -> bool {
    // core-graphics crate provides this
    unsafe { core_graphics::access::CGPreflightScreenCaptureAccess() }
}

pub fn handle_screenshot(params: Value) -> Result<Value, (i32, String)> {
    #[cfg(target_os = "macos")]
    if !screen_recording_granted() {
        return Err((ERR_INTERNAL,
            "Screen Recording permission not granted. Enable in System Settings > Privacy & Security > Screen Recording.".into()));
    }
    // ... existing code
}
```

Same check added to `handle_ocr()` when it captures screen (not when processing base64 input).

#### 3. Connection-level Read/Write Timeout (mod.rs)

Add timeout to `read_line()` in handle_connection to prevent idle connections from accumulating:

```rust
let line_result = tokio::time::timeout(
    Duration::from_secs(5),
    buf_reader.read_line(&mut line)
).await;
```

### Test Side (1 change)

#### socat Half-Close Fix (lib.sh)

Change socat parameters to keep the read direction open after stdin EOF:

```bash
# Old:
response=$(echo "$request" | socat -T10 - UNIX-CONNECT:"$SOCKET_PATH" 2>/dev/null)

# New:
response=$(echo "$request" | socat -t10 -T15 - UNIX-CONNECT:"$SOCKET_PATH" 2>/dev/null)
```

- `-t10`: After stdin EOF, keep reading from socket for 10 more seconds
- `-T15`: Total inactivity timeout of 15 seconds (safety net)

Apply to both `send_rpc()` and `send_rpc_timed()`.

## Files to Modify

| File | Change |
|------|--------|
| `apps/desktop/src-tauri/src/bridge/mod.rs` | Add timeout + spawn_blocking to dispatch, read timeout |
| `apps/desktop/src-tauri/src/bridge/perception.rs` | Add Screen Recording permission check |
| `apps/desktop/src-tauri/Cargo.toml` | Add `core-graphics` dependency (if not present) for permission API |
| `tests/bridge_acceptance/lib.sh` | Fix socat parameters |

## Success Criteria

1. `cargo build -p aleph-tauri` compiles without errors
2. Bridge returns JSON-RPC error (not empty response) for screenshot without Screen Recording permission
3. Bridge returns timeout error for handlers that take > 30s
4. Acceptance tests receive actual error responses instead of empty strings
5. Tests that previously failed due to socat half-close now receive proper responses
