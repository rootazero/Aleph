All 218 MCP tests pass. Here's the report:

---

# Module: mcp

## Summary
- Files reviewed: 30
- Issues found: 10
- Issues fixed: 10

## Fixes

1. **[client.rs:594] RwLock held across `.await` in `check_server_health`** → Clone Arc refs under lock, release lock, then await health checks (mirrors `list_tools` pattern)

2. **[transport/sse.rs:474] Race condition in `set_request_handler`** → Replaced async `tokio::spawn` with synchronous lock installation, mirroring the existing `set_notification_handler` pattern. Prevents server-initiated requests from being silently dropped.

3. **[transport/sse.rs:390] Missing SSRF validation in `send_notification`** → Added `validate_url()` check matching `send_request()`. Without this, a malicious config URL is blocked on requests but allowed through on notifications.

4. **[transport/http.rs:167] Missing SSRF validation in `send_notification`** → Same fix as SSE transport — added SSRF check for consistency.

5. **[auth/storage.rs:37] Integer overflow in `is_expired`** → `expires_at - 300` → `expires_at.saturating_sub(300)`. Prevents panic/wrong result on adversarial values deserialized from untrusted JSON.

6. **[auth/refresh.rs:170] Integer overflow in `should_refresh`** → `expires_at - refresh_threshold` → `expires_at.saturating_sub(refresh_threshold)`. Same pattern as above.

7. **[manager/config.rs:139] Orphaned temp file on rename failure** → Added `remove_file(&temp_path)` cleanup in the error path. Previously a failed rename left `.json.tmp` on disk forever.

8. **[external/runtime.rs:152] Windows `where` multi-line path** → Changed `.trim().to_string()` to `.lines().next()...trim()` to extract only the first path when `where` returns multiple results.

9. **[auth/callback.rs:259] `url_decode` null byte corruption on truncated sequences** → Rewrote the percent-decode fallback to track `h1`/`h2` separately, emitting the original characters (not null bytes) for malformed or truncated sequences.

10. **[approval.rs:124] Silent channel-drop rejection** → Added `tracing::warn!` when the approval oneshot channel closes unexpectedly, making the failure auditable instead of silent.

## Notes

- **`client.rs:484-509` — `sampling/createMessage` response dropped**: The TODO comment (`// TODO: Send response back to server via transport`) marks a known incomplete feature, not a regression. The `SseTransport::send_response()` method exists but isn't wired up. This requires design work (the transport isn't available inside the callback) and should be tracked as a feature task.

- **`expand_env_var` double-expansion**: The multi-pass `String::replace` in `config.rs:240-255` could theoretically double-expand if a variable's value contains `${...}` syntax. Risk is low (env vars rarely contain this pattern) but a single-pass `re.replace_all` would be cleaner. Deferred as low priority.

- **`check_runtime` is blocking sync**: `runtime.rs` uses `std::process::Command` (blocking I/O). Currently only called during startup filtering, so impact is minimal. If ever called from a hot async path, it should be wrapped in `spawn_blocking`.
