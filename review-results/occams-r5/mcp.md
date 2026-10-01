# src/mcp — occams-r5 Review

## Summary
- 6 findings: 0 crit / 0 high / 2 medium / 4 low
- Files reviewed (line counts)
- Batch-1 findings (2026-08, ~14 commits merged) excluded from re-report.

| File | Lines |
|------|-------|
| external/connection.rs | 2250 |
| manager/actor.rs | 1353 |
| transport/sse.rs | 1001 |
| auth/provider.rs | 1057 |
| transport/http.rs | 810 |
| transport/stdio.rs | 1078 |
| auth/storage.rs | 666 |
| tool_bridge.rs | 700 |
| modern/headers.rs | 747 |
| manager/types.rs | 916 |
| manager/handle.rs | 556 |
| auth/callback.rs | 454 |
| ... (20 more files) | ... |
| **Total** | **~19,349** |

## Findings

### [MEDIUM] src/mcp/modern/mod.rs:221 — unreachable `debug_assert!(false)` body carries live `tracing::error!`

- **Category:** Dead code / logic
- **Confidence:** High

`RequestMeta::attach` handles three cases. The `Some(Value::Object(map))` and `None|Null` paths cover every value that can legally reach this method. The `Some(other)` branch is unconditionally unreachable:

```rust
Some(other) => {
    debug_assert!(false, "MCP request params must be a JSON object");
    tracing::error!(
        "MCP request params were not a JSON object; \
         required _meta could not be attached"
    );
    return other;
}
```

Every call site in the codebase passes either `None` or `Some(Value::Object(...))`, so the `tracing::error!` never fires. The `return other` is also inert — `_meta` is silently not attached, but there is no caller that could observe this.

**Suggested fix:** Replace with `Some(other) => other,` and `#[cold]` it, or restructure as a `match` with two arms and a `unreachable!()` in the third. Add a `#[cfg(debug_assertions)]` guard on the `tracing::error!` if keeping the telemetry is desired.

**Risk:** Low — this code is never reached, but it misleads readers and any future caller that passes an unexpected type will silently degrade.

---

### [LOW] src/mcp/transport/sse.rs:271 — jitter can be exactly 0ms

- **Category:** Quality (resilience)
- **Location:** sse.rs:271
- **Confidence:** Low

```rust
let jitter_ms: u64 = (rand::random::<u8>() as u64) * 50;
```

`rand::random::<u8>()` returns `[0, 255]`. When it returns 0, `jitter_ms = 0`, and the reconnect sleep becomes `backoff_secs + 0ms`. The comment says *"a tiny jitter so concurrent listeners do not all land on the same instant"* — but 0 is not a tiny jitter, it is no jitter.

**Suggested fix:** Either clamp the random range to `[1, 255]` (`1 + rng.random_range(0..255)`), or add 1 to the result.

**Risk:** Low — probability is 1/256 per reconnect loop iteration, and only matters when multiple listeners are restarting simultaneously.

---

### [LOW] src/mcp/transport/sse.rs:319–320 — SSE overflow flag set but loop continues

- **Category:** Quality (correctness)
- **Location:** sse.rs:319–320 (inner `if !overflow` block)
- **Confidence:** Medium

When a single `data:` line exceeds `MAX_SSE_DATA_LINE_BYTES`, the overflow flag is set and `data` is cleared, but the loop does not `break` — it continues consuming remaining lines from the SSE stream:

```rust
if data.len() + piece.len() > MAX_SSE_DATA_LINE_BYTES {
    overflow = true;
    data.clear();
} else {
    // ... accumulate
}
```

The outer `for line in body.lines()` continues, and every subsequent `flush()` is a no-op because `overflow` is set. This is harmless (no incorrect parse result) but wasteful — a malicious server sending one enormous line followed by thousands of tiny ones would spin the loop without producing any result.

**Suggested fix:** Add `break;` after `overflow = true; data.clear();`. The result will be `None` (no matching response found), which correctly signals that the stream was unusable.

**Risk:** Low — the response is simply not found, which is the correct outcome for a hostile oversized event.

---

### [LOW] src/mcp/auth/callback.rs:265–283 — URL decode corrupts in-progress multi-byte sequences

- **Category:** Logic (correctness)
- **Location:** callback.rs:265–283 (`url_decode` function)
- **Confidence:** High

The comment says *"correctly handles multi-byte UTF-8 sequences by collecting consecutive percent-encoded bytes"*, but the flush logic in two places is wrong:

```rust
if !encoded_buf.is_empty() {
    result.push_str(&String::from_utf8_lossy(&encoded_buf));
    encoded_buf.clear();
}
```

`String::from_utf8_lossy` replaces each invalid byte with U+FFFD. For a valid multi-byte UTF-8 sequence split across two flushes, the first flush will emit the leading bytes (which are valid UTF-8 on their own) or U+FFFD for partial sequences. The second flush processes the remaining bytes as if they were at the start of a new sequence, again emitting U+FFFD for each incomplete tail.

Example: `%E4%B8%AD%E5%BD%A9` (UTF-8 for two Chinese characters) — if the `E4` byte is flushed on its own, `from_utf8_lossy` treats it as an incomplete sequence and emits U+FFFD; the remaining `%B8%AD%E5%BD%A9` is then decoded as a separate sequence.

**Suggested fix:** Accumulate raw bytes into `Vec<u8>` (not `String`) throughout the loop. Flush with `String::from_utf8_lossy` only at the end or when hitting a non-% character. Alternatively, use `percent_encoding::percent_decode_str` from the `percent-encoding` crate.

**Risk:** Low — callback parameters in practice contain ASCII-only state tokens, so this bug is latent rather than triggered.

---

### [LOW] src/mcp/** — `rust-doctor-disable` over-suppression (~50 annotations)

- **Category:** Code quality (lint hygiene)
- **Confidence:** Medium

Approximately 50 `// rust-doctor-disable-next-line excessive-clone` comments appear throughout the mcp subtree. The most common targets are:

- `Arc::clone(&x)` calls (cheap refcount increments — clippy's threshold may be too tight)
- `config.*.clone()` in `manager/actor.rs` on `McpManagerConfig` fields
- `.to_string()` / `.to_string_lossy()` chains

The annotations are not wrong, but 50 inline suppressions for one lint suggests either:
1. The clippy `excessive-clone` threshold is calibrated too low for this codebase
2. Or the codebase genuinely over-clones and the root cause should be fixed rather than suppressed

**Suggested fix:** Audit the top offenders (especially `manager/actor.rs` where `config.id.clone()`, `config.name.clone()`, etc. appear ~15 times). Where a `&str` or cheap reference suffices, use it. If the threshold is the root cause, raise it in `clippy.toml` rather than annotating each call site.

**Risk:** N/A (cosmetic — the suppressed lints are low-severity).

---

### [LOW] src/mcp/auth/callback.rs:102 — `CallbackServer` aborts server task without graceful drain

- **Category:** Quality (correctness)
- **Location:** callback.rs:102
- **Confidence:** High

```rust
let server_task = tokio::spawn(async move { loop { ... } });
// ...
server_task.abort();  // immediately kills the listener loop
```

The spawned task runs a loop calling `listener.accept()`. On a successful callback it sends the result and `break`s cleanly; on timeout it exits the loop. But `abort()` is called unconditionally *before* checking whether the task already exited — meaning the task is always abandoned rather than awaited.

With `timeout()` wrapping `result_rx.recv()`, the task is guaranteed to exit before the timeout fires. The `abort()` therefore always fires while the task is still running, forcibly terminating the `listener.accept()` call.

**Suggested fix:** Replace `server_task.abort()` with a graceful signal: either a `CancellationToken` passed into the task, or `server_task.await` (the task will exit when the result is sent, since the loop breaks).

**Risk:** Low — `abort()` is fire-and-forget, and the process exits shortly after anyway. However, on a non-exit path this could leak the listener socket.

---

## Cross-cutting

**Duplicated `parse_sse_response`:** Both `transport/http.rs` (~65 lines) and `transport/sse.rs` (~65 lines) define `parse_sse_response`. They are byte-for-byte identical (checked via `rg -c`). This is a maintenance hazard — any change to the SSE parsing logic must be applied in both places or the transports will diverge. Consider extracting to a shared function in `transport/traits.rs` or a private `transport/sse_utils.rs`.

**No lock-held-across-`.await` found:** The `StdMutex` in `transport/sse.rs` (`notification_handler`, `request_handler`) is held only for the synchronous callback invocation. All `RwLock` acquisitions use `std::sync::RwLock` for sync setters and `tokio::sync::RwLock` elsewhere, with locks released before any network I/O. No deadlock or priority-inversion risk found.

**`#[allow]` absent:** No `#[allow]` attributes exist in `src/mcp/`. The `excessive-clone` and `high-cyclomatic-complexity` lints appear to be disabled project-wide or via `clippy.toml` rather than per-item. This is fine — the suppression mechanism differs, but the intent is documented.

**`std::sync::RwLock` in `transport/http.rs`:** Correctly used for the sync `set_dialect` setter (the value is cloned out). No async operations under the lock. No concern.

---

## Out of scope

- Batch-1 findings (preflight URL redaction, error_class broken-pipe misclassification, jsonrpc re-exports) — confirmed fixed by git log (`109a9bd43`, `4d73e3148`).
- Logic of the SSE backoff algorithm (exponential, capped at 60s, with jitter) — reviewed and sound.
- OAuth state validation (`is_valid_state`) — correct constant-time concern documented.
- `rust-doctor-disable` over-suppression quality finding — flagged above, not actionable without broader project decision.
