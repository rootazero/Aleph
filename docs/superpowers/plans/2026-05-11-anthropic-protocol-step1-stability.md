# Anthropic Protocol Step 1 — Stability Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate two Anthropic protocol stability gaps in Aleph — (1) malformed tool-arg JSON fallback that violates the dispatcher's "arguments is always Object" invariant, and (2) missing per-event idle timeout on streaming responses that lets stalls hang for up to 300s.

**Architecture:** Two surgical commits, both reusing existing infrastructure. Commit 1 is a 1-line return-value fix in `delta.rs`. Commit 2 adds a per-protocol `Arc<AtomicU64>` field that mirrors the existing `last_model` pattern on `AnthropicProtocol`, allowing `build_request` (which has `&ProviderConfig`) to publish a value that `stream_deltas` (which doesn't take config) reads at stream-construction time. No changes to the `ProtocolAdapter` trait, no changes to other protocols (OpenAI/Gemini/Ollama), no changes to `src/harness/`.

**Tech Stack:** Rust 2024, `tokio` runtime, `tokio-stream` (already in `Cargo.toml:129`), `futures::Stream`, `tracing::warn!`, `serde_json::Value`, `reqwest` SSE byte streaming.

**Spec:** `docs/superpowers/specs/2026-05-11-anthropic-protocol-step1-stability.md`

---

## ⚠️ Plan Revisions (2026-05-11, post-cleanup re-review)

After the dead-import cleanup commit `0d760f7bb` landed, a re-review found two issues. The following overrides apply to the original tasks below:

1. **Task 3 is redefined** — Instead of adding an `include_str!` regression test, modify the **existing** `test_collector_malformed_tool_args_fallback` at `src/providers/delta.rs:432-455`. Change its `Value::String("not json{".to_string())` assertion to `Value::Object(serde_json::Map::new())` and rename to `test_collector_malformed_tool_args_returns_empty_object`. Reason: Commit 1's behavior change will break that existing test if left alone; the `include_str!` approach was a brittle proxy for what we now address directly.

2. **Tasks 6 + 7 are REPLACEMENT, not ADDITION** — `last_model: Arc<RwLock<Option<String>>>` at `anthropic.rs:53` is dead code (compiler warns `field 'last_model' is never read`; no caller anywhere in the crate reads or writes it). Instead of adding `stream_idle_timeout_secs` beside it:
   - Task 6: the new struct keeps `client`, `name_map`, and **adds** `stream_idle_timeout_secs` while **removing** `last_model`.
   - Task 7: the `new()` body initializes only `client`, `name_map`, and `stream_idle_timeout_secs` — drop the `last_model:` initializer line.
   - Update the Architecture sentence: replace "mirrors the existing last_model pattern" with "replaces the unused last_model scratchpad field".

3. **Commit 2 message** gains a third paragraph: "Also removes the unused `last_model` field — a previous-iteration scratchpad never read at any call site (compiler warning)."

4. **Polish**: Plan Task 1.2's ambiguity callout about `handle` vs `push` can be ignored — the confirmed method is `push`.

The compiler's other dead-code warning, `get_model_cost` at `proto_impl.rs:308`, is **out of Step 1 scope** and tracked for a future cleanup.

---

## File Structure

Before listing tasks, here is the complete inventory of files this plan touches:

| File | Action | Why |
|---|---|---|
| `src/providers/delta.rs` | Modify (`DeltaCollector::finish` + doc comment + new tests) | Change `Value::String(raw)` fallback to `Value::Object({})` — maintains the dispatcher's type invariant |
| `src/config/types/provider.rs` | Modify (add field to `ProviderConfig`) | New `stream_idle_timeout_secs: Option<u64>` user-facing knob |
| `src/providers/protocols/anthropic.rs` | Modify (add field to `AnthropicProtocol`) | New `stream_idle_timeout_secs: Arc<AtomicU64>` shared between build_request and stream_deltas |
| `src/providers/protocols/anthropic/proto_impl.rs` | Modify (`new()` constructor + `build_request` body) | Initialize the atomic to 60; copy user config into atomic during request build |
| `src/providers/protocols/anthropic/adapter.rs` | Modify (new helper + `stream_deltas` body + new tests) | `wrap_idle_timeout` helper + wire into stream_deltas |
| `CHANGELOG.md` | Modify (add English entry) | User-visible new error string ("Anthropic stream stalled…") |

**Files NOT touched** (and must not be touched):

- `src/providers/protocols/openai_chat/` / `openai_responses/` / `gemini/` / `ollama.rs` — other protocols
- `src/providers/adapter.rs` — `ProtocolAdapter` trait signature stays unchanged
- `src/harness/` — error bubbling and orphan filter are reused as-is
- `src/providers/protocols/anthropic/sse.rs` — exploration confirmed `IndexIdTracker` is already per-call

---

# Commit 1 — Tool-Arg Parse Fallback Hardening

This commit's blast radius is ~5 lines of production code + ~50 lines of test code in `src/providers/delta.rs`.

## Task 1: Add the first failing test — `malformed_tool_args_becomes_empty_object`

**Files:**
- Modify: `src/providers/delta.rs` (append to the bottom; if no `#[cfg(test)] mod tests` exists, create it with the standard pattern shown below)

- [ ] **Step 1.1: Locate or create the test module**

Run: `grep -n '#\[cfg(test)\]' /Volumes/TBU4/Workspace/Aleph/src/providers/delta.rs`

If output shows an existing `#[cfg(test)] mod tests { ... }`, append inside it (above the closing `}`).
If output is empty, append this scaffold at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
}
```

- [ ] **Step 1.2: Write the failing test**

Add this test function to the `tests` module:

```rust
#[test]
fn malformed_tool_args_becomes_empty_object() {
    // Simulate a streaming tool_use whose partial_json was truncated mid-write.
    // The collector should not fail; it should fall back to an empty object {}
    // (NOT a Value::String) so that the dispatcher's schema validation runs
    // normally and emits a structured "missing field X" ToolError.
    let mut collector = DeltaCollector::new();
    collector.handle(ProviderDelta::ToolCallStart {
        id: "call_truncated".to_string(),
        name: "Read".to_string(),
    });
    collector.handle(ProviderDelta::ToolCallArgDelta {
        id: "call_truncated".to_string(),
        delta: "{\"file_path\":\"/foo".to_string(), // truncated, missing closing quote/brace
    });
    collector.handle(ProviderDelta::ToolCallEnd {
        id: "call_truncated".to_string(),
    });

    let response = collector.finish();
    assert_eq!(response.tool_calls.len(), 1, "tool call should be preserved");
    let call = &response.tool_calls[0];
    assert_eq!(call.id, "call_truncated");
    assert_eq!(call.name, "Read");
    assert!(
        matches!(call.arguments, Value::Object(_)),
        "arguments must be Value::Object (the dispatcher invariant), got: {:?}",
        call.arguments
    );
    assert_eq!(
        call.arguments.as_object().unwrap().len(),
        0,
        "expected empty object, got: {:?}",
        call.arguments
    );
}
```

> **Note on `DeltaCollector::handle`**: The exact method name may differ — check the `impl DeltaCollector` block above the `finish()` method. If the public entry point is `push`, `feed`, `accept`, or `apply_delta`, substitute accordingly. The test should call whatever method exists for feeding `ProviderDelta` events into the collector. If the collector has no such method and only accepts deltas through a constructor or builder, adapt the test to construct the collector with `tool_calls: vec![("call_truncated".into(), "Read".into(), "{\"file_path\":\"/foo".into())]` directly via `pub(crate)` field access if the field is module-visible.

- [ ] **Step 1.3: Run the test to confirm it fails**

Run: `cargo test -p alephcore --lib malformed_tool_args_becomes_empty_object -- --nocapture`

Expected: **FAIL** with an assertion message like `assertion failed: matches!(call.arguments, Value::Object(_))` — because the current fallback returns `Value::String(raw_args)`, not `Value::Object({})`.

If the test fails to **compile** (not to assert), check that `DeltaCollector`, `ProviderDelta`, and `Value` are all accessible. Add missing `use` statements at the top of the `tests` module.

## Task 2: Make Task 1's test pass — the 1-line fix

**Files:**
- Modify: `src/providers/delta.rs` (the `finish()` method, the malformed-args branch)

- [ ] **Step 2.1: Apply the production change**

Find this block inside `DeltaCollector::finish`:

```rust
match serde_json::from_str::<Value>(&raw_args) {
    Ok(v) => v,
    Err(e) => {
        warn!(
            tool_id = %id,
            tool_name = %name,
            error = %e,
            raw_args = %raw_args,
            "Malformed tool arguments — falling back to raw string value"
        );
        Value::String(raw_args)
    }
}
```

Replace **only** the `Err(e) => { ... }` body with:

```rust
Err(e) => {
    warn!(
        tool_id = %id,
        tool_name = %name,
        error = %e,
        raw_args = %raw_args,
        "Malformed tool arguments — defaulting to empty object (dispatcher will report missing fields)"
    );
    Value::Object(serde_json::Map::new())
}
```

The structured `warn!` fields (`tool_id`, `tool_name`, `error`, `raw_args`) are unchanged — only the log message text and the returned `Value` variant change.

- [ ] **Step 2.2: Update the docstring above `finish()`**

Find the doc comment above `pub fn finish` that says:

```rust
/// Malformed tool arguments are handled gracefully: if `serde_json::from_str` fails,
/// a warning is logged and the raw string is stored as `Value::String(raw)`.
```

Replace with:

```rust
/// Malformed tool arguments are handled gracefully: if `serde_json::from_str` fails,
/// a warning is logged (including the full raw payload for telemetry) and an
/// empty object `Value::Object({})` is returned. The dispatcher's schema
/// validation then reports the missing fields, producing a structured
/// ToolError that the model can react to on the next turn.
```

- [ ] **Step 2.3: Run the test and verify it passes**

Run: `cargo test -p alephcore --lib malformed_tool_args_becomes_empty_object -- --nocapture`

Expected: **PASS**.

## Task 3: Add the second test — `malformed_tool_args_logs_raw`

**Files:**
- Modify: `src/providers/delta.rs` (append to the `tests` module)

- [ ] **Step 3.1: Add the test**

Append to the `tests` module:

```rust
#[test]
fn malformed_tool_args_logs_raw() {
    // Verify that when JSON parse fails, the full raw_args string is preserved
    // in the structured log so operators can diagnose model truncation.
    // We rely on the existence of the `raw_args = %raw_args` field in the
    // warn! invocation, which is a literal grep target for telemetry pipelines.
    let source = include_str!("delta.rs");
    assert!(
        source.contains("raw_args = %raw_args"),
        "DeltaCollector::finish must emit raw_args as a structured tracing field"
    );
    assert!(
        source.contains("defaulting to empty object"),
        "DeltaCollector::finish must log the new fallback message"
    );
    assert!(
        !source.contains("falling back to raw string value"),
        "Old fallback log message must be removed"
    );
}
```

> **Why include_str! instead of capturing tracing output?** Capturing structured `tracing` events in unit tests requires the `tracing-test` crate or a custom subscriber, which would inflate this single-line fix into a test-infrastructure project. The `include_str!` approach is a low-cost regression guard that asserts the contract: "the raw_args field MUST be emitted" without requiring runtime log capture. If a future task adds proper tracing-test infrastructure, this test can be rewritten then.

- [ ] **Step 3.2: Run the test and verify it passes**

Run: `cargo test -p alephcore --lib malformed_tool_args_logs_raw -- --nocapture`

Expected: **PASS**.

## Task 4: Final verification + Commit 1

- [ ] **Step 4.1: Run full Anthropic-related tests for safety**

Run: `cargo test -p alephcore --lib delta -- --nocapture`

Expected: all `delta` tests pass (the two new ones plus any existing).

- [ ] **Step 4.2: Run clippy on the touched file**

Run: `cargo clippy -p alephcore --lib -- -D warnings 2>&1 | grep -A2 'delta.rs' || echo "clippy clean for delta.rs"`

Expected: `clippy clean for delta.rs` (no new warnings).

- [ ] **Step 4.3: Stage and commit**

Stage only the one touched file:

```bash
git add src/providers/delta.rs
git status --short
```

Expected output line: `M  src/providers/delta.rs`

Commit:

```bash
git commit -m "$(cat <<'EOF'
providers/delta: empty object instead of String on malformed tool args

DeltaCollector::finish previously fell back to Value::String(raw_args) when
serde_json::from_str failed on a streaming tool_use's accumulated
partial_json. This violated the dispatcher's "arguments is always
Value::Object" invariant, causing schema validation to fail with
type-mismatch errors that the model couldn't act on cleanly.

Return Value::Object(Map::new()) instead. The dispatcher then emits the
standard "missing required field X" ToolError, giving the model an
actionable signal to retry. The raw_args payload remains in the
structured warn! log for telemetry.

Adds 2 unit tests.
EOF
)"
```

Verify commit succeeded:

```bash
git log -1 --oneline
```

Expected: One new commit at HEAD with subject `providers/delta: empty object instead of String on malformed tool args`.

---

# Commit 2 — Streaming Idle Timeout

This commit's blast radius is ~5 files, ~60 lines of production code + ~100 lines of tests.

## Task 5: Add `stream_idle_timeout_secs` to `ProviderConfig`

**Files:**
- Modify: `src/config/types/provider.rs`

- [ ] **Step 5.1: Add the field**

Open `src/config/types/provider.rs`. Find the `ProviderConfig` struct definition (around line 28). Locate the existing `timeout_seconds: u64` field with `#[serde(default = "default_timeout_seconds")]`. Immediately below `timeout_seconds`, add:

```rust
    /// Per-event idle timeout for streaming responses, in seconds.
    ///
    /// Wraps each SSE event read with a watchdog: if no chunk arrives within
    /// this duration, the stream aborts with `AlephError::Timeout`. This is
    /// distinct from `timeout_seconds` (which is the total request timeout).
    ///
    /// `None` or unset: 60 seconds (default).
    /// `Some(0)`: idle timeout disabled.
    ///
    /// Currently honored only by the Anthropic protocol adapter; other
    /// protocols ignore this field.
    #[serde(default)]
    pub stream_idle_timeout_secs: Option<u64>,
```

- [ ] **Step 5.2: Verify the project still compiles**

Run: `cargo check -p alephcore 2>&1 | tail -15`

Expected: `Finished ... dev [...] target(s)` (no errors). If there's an error like "missing field `stream_idle_timeout_secs` in initializer of ProviderConfig", search for `ProviderConfig {` struct-literal sites and add `stream_idle_timeout_secs: None,`:

```bash
rg -n 'ProviderConfig\s*\{' --type rust src/ tests/
```

For each match that constructs `ProviderConfig` literally (rather than via `..Default::default()`), add `stream_idle_timeout_secs: None,` to the initializer.

## Task 6: Add `stream_idle_timeout_secs` field to `AnthropicProtocol`

**Files:**
- Modify: `src/providers/protocols/anthropic.rs`

- [ ] **Step 6.1: Add the struct field**

Open `src/providers/protocols/anthropic.rs`. Find the `AnthropicProtocol` struct (around line 47):

```rust
pub struct AnthropicProtocol {
    client: Client,
    /// Sanitized → original tool-name map. Populated when building requests
    /// (so Anthropic accepts the names) and consulted while parsing the
    /// streamed response (so the dispatcher receives the original names).
    name_map: ToolNameMap,
    last_model: std::sync::Arc<std::sync::RwLock<Option<String>>>,
}
```

Replace with:

```rust
pub struct AnthropicProtocol {
    client: Client,
    /// Sanitized → original tool-name map. Populated when building requests
    /// (so Anthropic accepts the names) and consulted while parsing the
    /// streamed response (so the dispatcher receives the original names).
    name_map: ToolNameMap,
    last_model: std::sync::Arc<std::sync::RwLock<Option<String>>>,
    /// Per-event idle timeout (seconds) for streaming responses.
    /// Written by `build_request` from `ProviderConfig.stream_idle_timeout_secs`
    /// (default 60); read by `stream_deltas` at stream-construction time.
    /// A value of 0 disables the idle watchdog.
    ///
    /// Uses `AtomicU64` rather than `RwLock<u64>` because the value is a
    /// single primitive: lock-free load/store is appropriate and avoids
    /// any contention between concurrent `build_request` and `stream_deltas`
    /// calls within the same protocol instance.
    stream_idle_timeout_secs: std::sync::Arc<std::sync::atomic::AtomicU64>,
}
```

## Task 7: Initialize the new field in `AnthropicProtocol::new`

**Files:**
- Modify: `src/providers/protocols/anthropic/proto_impl.rs`

- [ ] **Step 7.1: Update the constructor**

Open `src/providers/protocols/anthropic/proto_impl.rs`. Find the `new()` method (around line 17):

```rust
pub fn new(client: Client) -> Self {
    Self {
        client,
        name_map: Arc::new(RwLock::new(HashMap::new())),
        last_model: std::sync::Arc::new(std::sync::RwLock::new(None)),
    }
}
```

Replace with:

```rust
pub fn new(client: Client) -> Self {
    Self {
        client,
        name_map: Arc::new(RwLock::new(HashMap::new())),
        last_model: std::sync::Arc::new(std::sync::RwLock::new(None)),
        stream_idle_timeout_secs: std::sync::Arc::new(
            std::sync::atomic::AtomicU64::new(60),
        ),
    }
}
```

The default `60` matches the documented behavior in `ProviderConfig.stream_idle_timeout_secs` — when the user doesn't set it, `build_request` will write `60` (via `unwrap_or(60)`) into the atomic anyway, but initializing to `60` here means even if `stream_deltas` somehow runs before `build_request` ever did (e.g. in a test that bypasses `build_request`), the value is sensible.

- [ ] **Step 7.2: Verify compile**

Run: `cargo check -p alephcore 2>&1 | tail -10`

Expected: `Finished ... dev [...]` (no errors).

## Task 8: Wire `build_request` to publish the user-config value into the atomic

**Files:**
- Modify: `src/providers/protocols/anthropic/adapter.rs`

- [ ] **Step 8.1: Add the atomic store at the top of `build_request`**

Open `src/providers/protocols/anthropic/adapter.rs`. Find the `fn build_request` method body (the `impl ProtocolAdapter for AnthropicProtocol` block, around line 23 onward). The first line of the body currently looks like:

```rust
let actual_model = payload
    .model
    .as_deref()
    .unwrap_or_else(|| config.default_model());
```

Immediately **before** that line, add:

```rust
// Publish the user's idle-timeout config to the shared atomic so
// stream_deltas (which doesn't receive &ProviderConfig in its trait
// signature) can read the right value when the response arrives.
self.stream_idle_timeout_secs.store(
    config.stream_idle_timeout_secs.unwrap_or(60),
    std::sync::atomic::Ordering::Relaxed,
);
```

- [ ] **Step 8.2: Verify compile**

Run: `cargo check -p alephcore 2>&1 | tail -10`

Expected: `Finished ... dev [...]` (no errors).

## Task 9: Add the `wrap_idle_timeout` helper function

**Files:**
- Modify: `src/providers/protocols/anthropic/adapter.rs`

- [ ] **Step 9.1: Add the helper at the top of the file**

Open `src/providers/protocols/anthropic/adapter.rs`. Locate the existing imports at the top of the file (around line 1-20). After the existing `use` block, add:

```rust
use axum::body::Bytes;
use std::time::Duration;
```

(Check first: `axum::body::Bytes` may already be imported via `super::sse` or elsewhere. If `cargo check` later flags unused imports, remove redundant ones.)

After the imports but **before** the `#[async_trait] impl ProtocolAdapter for AnthropicProtocol` block, add:

```rust
/// Wrap a byte stream with a per-event idle-timeout watchdog.
///
/// Each `.next()` call on the returned stream completes within `idle_secs`
/// seconds or yields `Err(AlephError::Timeout { ... })`. The error carries a
/// suggestion string referencing the actual configured threshold so the user
/// can adjust `[providers.<name>].stream_idle_timeout_secs` if the default
/// is too aggressive for their network.
///
/// `idle_secs == 0` disables the watchdog (returns the stream untouched).
fn wrap_idle_timeout(
    stream: futures::stream::BoxStream<'static, Result<Bytes>>,
    idle_secs: u64,
) -> futures::stream::BoxStream<'static, Result<Bytes>> {
    if idle_secs == 0 {
        return stream;
    }
    use tokio_stream::StreamExt as _;
    let timed = stream.timeout(Duration::from_secs(idle_secs));
    let mapped = futures::StreamExt::map(timed, move |res| match res {
        Ok(inner) => inner,
        Err(_elapsed) => Err(AlephError::Timeout {
            suggestion: Some(format!(
                "Anthropic stream stalled: no SSE event received for {idle_secs}s. \
                 The upstream connection appears dead. Raise \
                 [providers.<name>].stream_idle_timeout_secs in config.toml \
                 if your provider routinely takes longer between events."
            )),
        }),
    });
    Box::pin(mapped)
}
```

> **Why `futures::StreamExt::map` (UFCS) instead of `.map`?** Both `tokio_stream::StreamExt` and `futures::StreamExt` define a `.map` method, which causes ambiguity if both are in scope as default-resolution traits. Using the universal function-call syntax `futures::StreamExt::map(timed, ...)` resolves the trait unambiguously.

- [ ] **Step 9.2: Verify compile**

Run: `cargo check -p alephcore 2>&1 | tail -15`

Expected: `Finished ... dev [...]`. If `tokio_stream::StreamExt::timeout` is reported missing, check that `tokio-stream` in `Cargo.toml` is enabled with the relevant feature. Currently `Cargo.toml:129` shows `tokio-stream = { version = "0.1", features = ["sync"] }` — the `timeout` method is in the default feature set (`time` is enabled via `tokio`'s `time` feature, which Aleph already uses). If absent, append the `time` feature to `tokio-stream`'s feature list.

## Task 10: Write the first idle-timeout test — `wrap_idle_timeout_fires_after_threshold`

**Files:**
- Modify: `src/providers/protocols/anthropic/adapter.rs` (append to or create `#[cfg(test)] mod tests`)

- [ ] **Step 10.1: Locate or create the tests module**

Run: `grep -n '#\[cfg(test)\]' /Volumes/TBU4/Workspace/Aleph/src/providers/protocols/anthropic/adapter.rs`

If the file has no `#[cfg(test)] mod tests`, append this scaffold at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use futures::stream::StreamExt;
}
```

If the file already has tests, ensure `axum::body::Bytes` and `futures::stream::StreamExt` are imported inside the `tests` module (add them if missing).

- [ ] **Step 10.2: Write the failing test**

Append this test to the `tests` module:

```rust
#[tokio::test(start_paused = true)]
async fn wrap_idle_timeout_fires_after_threshold() {
    // A stream that never yields any bytes. With idle_secs=5, the wrapped
    // stream's first .next() should resolve to Err(AlephError::Timeout)
    // after we advance the simulated clock past 5 seconds.
    let inner: futures::stream::BoxStream<'static, crate::error::Result<Bytes>> =
        futures::stream::pending().boxed();
    let mut wrapped = wrap_idle_timeout(inner, 5);

    // Advance the paused tokio clock past the idle threshold.
    tokio::time::advance(std::time::Duration::from_secs(6)).await;

    let next = wrapped.next().await.expect("stream should yield Err, not None");
    match next {
        Err(crate::error::AlephError::Timeout { suggestion }) => {
            let msg = suggestion.expect("Timeout must carry a suggestion message");
            assert!(
                msg.contains("Anthropic stream stalled"),
                "suggestion should mention stall, got: {msg}"
            );
            assert!(
                msg.contains("5s"),
                "suggestion should mention the configured threshold (5s), got: {msg}"
            );
        }
        other => panic!("expected AlephError::Timeout, got: {other:?}"),
    }
}
```

> **`#[tokio::test(start_paused = true)]`**: Pauses the tokio time driver so `tokio::time::advance` can fast-forward the simulated clock without real sleeping. This makes the test run in milliseconds instead of seconds.

- [ ] **Step 10.3: Run the test and verify it passes**

Run: `cargo test -p alephcore --lib wrap_idle_timeout_fires_after_threshold -- --nocapture`

Expected: **PASS**. If it fails with a compile error about `start_paused`, check that the project's `tokio` dependency has the `test-util` feature. Search:

```bash
grep -E 'tokio\s*=' /Volumes/TBU4/Workspace/Aleph/Cargo.toml
```

If `test-util` is missing, the test setup needs the feature in `[dev-dependencies]`. Append a `dev-dependencies` entry:

```toml
[dev-dependencies]
tokio = { version = "1", features = ["test-util"] }
```

(Skip this if already present; check `Cargo.toml` first.)

## Task 11: Write the second idle-timeout test — `wrap_idle_timeout_resets_on_event`

**Files:**
- Modify: `src/providers/protocols/anthropic/adapter.rs` (append to `tests` module)

- [ ] **Step 11.1: Write the test**

Append:

```rust
#[tokio::test(start_paused = true)]
async fn wrap_idle_timeout_resets_on_event() {
    use futures::stream::StreamExt;
    // Build a stream that yields a ping every 3 seconds for 5 ticks.
    // With idle_secs=5, no individual gap exceeds 5s, so the wrapper must
    // NEVER yield a Timeout — all events should pass through cleanly.
    let stream = async_stream::stream! {
        for i in 0u8..5 {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            yield Ok::<_, crate::error::AlephError>(Bytes::from(format!("ping{i}\n")));
        }
    };
    let inner: futures::stream::BoxStream<'static, crate::error::Result<Bytes>> =
        stream.boxed();
    let mut wrapped = wrap_idle_timeout(inner, 5);

    for expected in 0u8..5 {
        let chunk = wrapped
            .next()
            .await
            .expect("stream should still be active")
            .expect("no Timeout should fire when events arrive within idle window");
        assert_eq!(chunk, Bytes::from(format!("ping{expected}\n")));
    }
    assert!(wrapped.next().await.is_none(), "stream should be exhausted");
}
```

> **`async_stream::stream!`**: A crate already in Aleph's dep tree (verify with `grep async_stream /Volumes/TBU4/Workspace/Aleph/Cargo.toml`). If not present, add to `[dev-dependencies]`: `async-stream = "0.3"`.

- [ ] **Step 11.2: Run the test and verify it passes**

Run: `cargo test -p alephcore --lib wrap_idle_timeout_resets_on_event -- --nocapture`

Expected: **PASS**. If `async_stream` is missing, the failure will be a compile error — add the dev-dep as above.

## Task 12: Write the third idle-timeout test — `wrap_idle_timeout_zero_disables`

**Files:**
- Modify: `src/providers/protocols/anthropic/adapter.rs` (append to `tests` module)

- [ ] **Step 12.1: Write the test**

Append:

```rust
#[tokio::test(start_paused = true)]
async fn wrap_idle_timeout_zero_disables() {
    use futures::stream::StreamExt;
    // With idle_secs=0, the wrapper returns the inner stream verbatim — even
    // a never-yielding stream should never produce a Timeout error.
    let inner: futures::stream::BoxStream<'static, crate::error::Result<Bytes>> =
        futures::stream::pending().boxed();
    let mut wrapped = wrap_idle_timeout(inner, 0);

    // Advance time well past any reasonable timeout threshold.
    tokio::time::advance(std::time::Duration::from_secs(86_400)).await;

    // Poll once with a tiny real-time deadline; if the wrapper IS firing a
    // timeout, this would return Some(Err(Timeout)). If correctly disabled,
    // the inner pending() never yields, so we get a poll-pending which
    // converts to Err(Elapsed) at the outer timeout we apply here.
    let outcome = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        wrapped.next(),
    )
    .await;
    assert!(
        outcome.is_err(),
        "wrapped stream with idle_secs=0 must never yield; got: {outcome:?}"
    );
}
```

- [ ] **Step 12.2: Run the test and verify it passes**

Run: `cargo test -p alephcore --lib wrap_idle_timeout_zero_disables -- --nocapture`

Expected: **PASS**.

## Task 13: Wire `wrap_idle_timeout` into `stream_deltas`

**Files:**
- Modify: `src/providers/protocols/anthropic/adapter.rs` (the `stream_deltas` method body)

- [ ] **Step 13.1: Apply the wiring**

Open `src/providers/protocols/anthropic/adapter.rs`. Locate this block inside `stream_deltas` (around line 200):

```rust
let byte_stream = response
    .bytes_stream()
    .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
    .boxed();
```

Immediately **after** that block, **before** the `/// Per-iteration mutable state carried through unfold` doc comment, insert:

```rust
// Apply the per-event idle watchdog using the value published by the
// most recent build_request call (default 60s; 0 = disabled).
let idle_secs = self
    .stream_idle_timeout_secs
    .load(std::sync::atomic::Ordering::Relaxed);
let byte_stream = wrap_idle_timeout(byte_stream, idle_secs);
```

The rest of `stream_deltas` (the `struct State`, the `let state = State { bytes: byte_stream, ... }`, and the `unfold` loop) is unchanged because `wrap_idle_timeout` preserves the `BoxStream<'static, Result<Bytes>>` type signature.

- [ ] **Step 13.2: Verify compile**

Run: `cargo check -p alephcore 2>&1 | tail -10`

Expected: `Finished ... dev [...]` (no errors).

## Task 14: Run the full Anthropic test suite + clippy

- [ ] **Step 14.1: Run all Anthropic protocol tests**

Run: `cargo test -p alephcore --lib anthropic 2>&1 | tail -20`

Expected: all existing tests still pass (regression check) + the 3 new `wrap_idle_timeout_*` tests pass. Look for `test result: ok. <N> passed; 0 failed`.

- [ ] **Step 14.2: Run all delta tests (should still be green from Commit 1)**

Run: `cargo test -p alephcore --lib delta 2>&1 | tail -10`

Expected: 2 new `malformed_tool_args_*` tests + any existing delta tests, all `0 failed`.

- [ ] **Step 14.3: Run clippy**

Run: `cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -20`

Expected: zero new warnings. If clippy complains about unused imports in `adapter.rs`, remove redundant `use axum::body::Bytes;` or `use std::time::Duration;` if they were already imported at the file level.

## Task 15: Update CHANGELOG.md

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 15.1: Find the unreleased / next-version section**

Run: `head -30 /Volumes/TBU4/Workspace/Aleph/CHANGELOG.md`

Identify the topmost section. If it has an `Unreleased` heading or a date-stamped section for the next release, append entries there. Otherwise, follow the existing pattern (most projects: `## [Unreleased] — Added / Fixed / Changed` subsections).

- [ ] **Step 15.2: Add two English entries**

Under the appropriate section, add:

```markdown
### Added
- Anthropic provider: new `stream_idle_timeout_secs` config (default 60s) wraps each SSE event read with a watchdog. If no chunk arrives within the threshold, the request aborts with `AlephError::Timeout` and the existing orphan tool_use filter cleans up any unfinished assistant tool calls on the next turn. Set to `0` to disable.

### Fixed
- Anthropic streaming: malformed tool-argument JSON (e.g. truncated `partial_json`) now falls back to an empty object `{}` instead of a `Value::String(raw)`. This preserves the dispatcher's "arguments is always Object" invariant so schema validation produces actionable "missing field X" ToolErrors that the model can react to.
```

If the CHANGELOG uses a different layout (e.g., bullet list without `Added/Fixed` headers), adapt the entries to match the existing style — what matters is that two English entries appear, one per fix, with the new error string and the disable-with-zero detail mentioned for Step 14's user-facing change.

## Task 16: Final verification + Commit 2

- [ ] **Step 16.1: Run the full lib test suite for regression safety**

Run: `cargo test -p alephcore --lib 2>&1 | tail -10`

Expected: any baseline failures match the 8 known failures listed in `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_baseline_test_failures.md`. NO new failures.

If you see new failures, do NOT proceed to commit. Investigate which test is broken; if a test in another protocol (`openai_chat`, `gemini`, `ollama`) failed, you likely accidentally touched its file or a shared type — revert and re-apply the minimal change to the Anthropic-only path.

- [ ] **Step 16.2: Manually verify the project builds in release mode**

Run: `cargo build -p alephcore --release 2>&1 | tail -5`

Expected: `Finished ... release [...]`.

- [ ] **Step 16.3: Stage and commit**

Stage all touched files:

```bash
git add \
  src/config/types/provider.rs \
  src/providers/protocols/anthropic.rs \
  src/providers/protocols/anthropic/proto_impl.rs \
  src/providers/protocols/anthropic/adapter.rs \
  CHANGELOG.md
git status --short
```

Expected output (5 modified files, nothing extra):

```
M  CHANGELOG.md
M  src/config/types/provider.rs
M  src/providers/protocols/anthropic.rs
M  src/providers/protocols/anthropic/adapter.rs
M  src/providers/protocols/anthropic/proto_impl.rs
```

If any other files appear staged, unstage them with `git restore --staged <path>` before committing.

Commit:

```bash
git commit -m "$(cat <<'EOF'
providers/anthropic: per-event streaming idle timeout

Adds a per-event idle watchdog around the SSE byte stream consumed by
AnthropicProtocol::stream_deltas. If no chunk arrives within
ProviderConfig.stream_idle_timeout_secs (default 60s), the stream aborts
with AlephError::Timeout — already classified as Transient and routed
through the existing retry policy table. The existing orphan tool_use
filter in DefaultPromptBuilder then cleans up any unfinished assistant
tool calls on the next turn, with no harness-level changes required.

The configuration value flows from ProviderConfig → AnthropicProtocol's
new stream_idle_timeout_secs: Arc<AtomicU64> field (mirroring the
existing last_model pattern). build_request stores into the atomic;
stream_deltas reads from it at stream-construction time. This keeps the
ProtocolAdapter trait signature unchanged — no other protocol adapters
(OpenAI/Gemini/Ollama) are touched.

Set stream_idle_timeout_secs = 0 in config.toml to disable.

Adds 3 unit tests covering: timeout fires on stalled stream, timer
resets on incoming event, zero disables the watchdog. Updates CHANGELOG.
EOF
)"
```

Verify commit succeeded:

```bash
git log -2 --oneline
```

Expected: two new commits at HEAD, one for `providers/delta` and one for `providers/anthropic`.

## Task 17: Manual integration verification

This task is **manual** (not scripted) because writing automated coverage for a real Anthropic round-trip would require a wiremock setup that's out of proportion to the change.

- [ ] **Step 17.1: Start the server**

Run in one terminal: `cargo run --bin aleph-server`

Wait for it to log `listening on 127.0.0.1:18790` (or similar).

- [ ] **Step 17.2: Send a normal webchat message**

Open the webchat UI in a browser, send a simple message like "what is 2+2?". Confirm a normal response streams back without errors.

- [ ] **Step 17.3: Switch to kimi-for-coding and repeat**

Switch the agent's provider to `kimi-for-coding` (either through the webchat UI or by toggling `default_provider` in config.toml). Send the same simple message. Confirm response streams back normally.

- [ ] **Step 17.4: Trigger an artificial stall (one of two options)**

**Option A — socat-based stall** (more realistic, requires socat):

```bash
# In one terminal, set up a proxy that delays SSE events:
socat TCP-LISTEN:18791,fork,reuseaddr EXEC:'sh -c "sleep 90 && nc api.anthropic.com 443"'

# Reconfigure aleph to use base_url=https://127.0.0.1:18791 for anthropic provider
# (only for this test — revert after)
# Then send a message and confirm:
# - 60s after no SSE event, the server logs "Anthropic stream stalled..." error
# - The webchat turn fails cleanly (does not hang)
# - The next user message in the same session produces a normal response
#   (this exercises the orphan tool_use filter on a stalled tool_use, if any)
```

**Option B — config-driven aggressive timeout** (simpler, no socat):

Temporarily set in `config.toml` under the anthropic provider section:

```toml
stream_idle_timeout_secs = 1
```

Restart the server. Send a normal message. Observe:
- The server logs "Anthropic stream stalled (no SSE event for 1s)" within ~1 second
- The webchat turn fails (because 1s is too aggressive for normal Anthropic stream cadence)
- Revert `stream_idle_timeout_secs` to `60` (or remove the line) and restart — normal behavior returns

- [ ] **Step 17.5: Verify the orphan filter handled the failed turn cleanly**

After triggering a stall via Option A or B, send a follow-up message in the same session. Confirm:
- The follow-up message does NOT produce a `400 Bad Request` from Anthropic with `tool_call_ids did not have response messages` (this would indicate the orphan filter failed)
- The model responds normally (it has no memory of the stalled tool_use because the filter scrubbed it)

If you observe a 400 error, the orphan filter regression is a blocker — investigate `src/harness/prompt.rs:62-106` for any unintended interaction with the new error path.

- [ ] **Step 17.6: Restore normal config**

Ensure `config.toml` has the default `stream_idle_timeout_secs = 60` (or no entry, which uses the default). Restart the server. Confirm normal operation.

---

## Self-Review

After writing the full plan, I checked it against the spec:

**1. Spec coverage:**

- ✅ Change 1 (delta.rs tool-arg parse) → Tasks 1-4
- ✅ Change 2 (idle timeout) → Tasks 5-16
- ✅ ProviderConfig new field → Task 5
- ✅ AnthropicProtocol new field → Task 6-7
- ✅ build_request wiring → Task 8
- ✅ stream_deltas wiring → Task 13
- ✅ 5 new unit tests → Tasks 1, 3, 10, 11, 12
- ✅ CHANGELOG English entry → Task 15
- ✅ Manual stall verification → Task 17
- ✅ Two-commit Rollout → Tasks 1-4 (commit 1), 5-16 (commit 2)
- ✅ AlephError::Timeout reuse → Task 9
- ✅ "last_model pattern" reuse → Tasks 6-7 (struct/init mirrors)
- ✅ No trait signature changes → confirmed throughout (Task 8 stores into atomic instead of adding param)
- ✅ Non-goals (IndexIdTracker, OAuth, loop detection, kimi tagged-text) not addressed in any task — correct

**2. Placeholder scan:** No TBD/TODO/"figure out later" in any step. All test code blocks are complete. All shell commands have expected output. All file paths are absolute or fully-qualified.

**3. Type consistency:**

- `stream_idle_timeout_secs` is consistently `Option<u64>` in `ProviderConfig` and `Arc<AtomicU64>` in `AnthropicProtocol` across Tasks 5, 6, 7, 8, 9, 13
- `wrap_idle_timeout` signature is `(BoxStream<'static, Result<Bytes>>, u64) -> BoxStream<'static, Result<Bytes>>` in Task 9 and consumed with the same signature in Task 13
- `AlephError::Timeout { suggestion: Option<String> }` is used identically in Task 9 (production) and Task 10 (test assertion)

**4. Ambiguity check:** One mild ambiguity at Step 1.2 (the exact name of `DeltaCollector::handle`/`push`/etc.) is called out explicitly with fallback guidance. All other test code uses concrete, grep-able identifiers.

No issues found requiring follow-up edits.

---

**Plan complete and saved to** `docs/superpowers/plans/2026-05-11-anthropic-protocol-step1-stability.md`.

Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
