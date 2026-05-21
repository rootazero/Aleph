# Stale-Stream Killer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop a stalled streaming LLM response from hanging an agent turn indefinitely — by giving every provider protocol a per-chunk SSE idle timeout (today only Anthropic has one) and by configuring the provider HTTP client with connection-level timeouts (today it is a bare `reqwest::Client::new()`).

**Architecture:** Part A extracts the existing Anthropic `wrap_idle_timeout` into a shared `stream_idle` module and applies it to the OpenAI Chat, OpenAI Responses, and Gemini streaming paths, replicating the `AtomicU64` field pattern Anthropic already uses (store from config in `build_request`, load in `stream_deltas`). Part B adds a shared `build_provider_http_client()` with `connect_timeout` / `pool_idle_timeout` / `tcp_keepalive` and points the protocol registry and loader at it. No harness/loop code is touched; this is pure provider-transport hardening.

**Tech Stack:** Rust, reqwest, tokio-stream (`StreamExt::timeout`), futures, `std::sync::atomic::AtomicU64`.

**Spec:** [`docs/superpowers/specs/2026-05-21-stale-stream-killer-design.md`](../specs/2026-05-21-stale-stream-killer-design.md)

**Worktree:** Implementation runs in `worktree-feat-stale-stream-killer` (created via `superpowers:using-git-worktrees` at execution time). Spec and plan live on `main`.

**Cargo concurrency cap:** This machine OOM-kills past 3 concurrent cargo processes. Before EVERY `cargo` command, prefix the gate:
```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && <cargo command>
```
Use `run_in_background: true` for cargo runs (compiles take 5-20 min); read the output file when notified.

---

## File Structure

| File | New / Modified | Responsibility |
|------|----------------|----------------|
| `src/providers/protocols/stream_idle.rs` | **New** | `wrap_idle_timeout` (generalized, `provider_label` param), `effective_idle_secs(config)`, `DEFAULT_STREAM_IDLE_SECS` const, unit tests. |
| `src/providers/protocols/http_client.rs` | **New** | `build_provider_http_client()` — shared `reqwest::Client` builder with connection-level timeouts. |
| `src/providers/protocols/mod.rs` | Modified | Declare the two new modules. |
| `src/providers/protocols/anthropic/adapter.rs` | Modified | Delete the local `wrap_idle_timeout` + its 3 tests; call the shared one with label `"Anthropic"`. |
| `src/providers/protocols/openai_chat.rs` | Modified | Add `stream_idle_timeout_secs: Arc<AtomicU64>` to `OpenAiProtocol`. |
| `src/providers/protocols/openai_chat/proto_impl.rs` | Modified | Init the field in `new()`. |
| `src/providers/protocols/openai_chat/adapter.rs` | Modified | Store in `build_request`; wrap the byte stream in `stream_deltas`; add test. |
| `src/providers/protocols/gemini.rs` | Modified | Add the field to `GeminiProtocol`. |
| `src/providers/protocols/gemini/proto_impl.rs` | Modified | Init the field in `new()`. |
| `src/providers/protocols/gemini/adapter.rs` | Modified | Store + wrap + test. |
| `src/providers/protocols/openai_responses/mod.rs` | Modified | Add the field to `OpenAiResponsesProtocol`, init in `new()`. |
| `src/providers/protocols/openai_responses/adapter.rs` *(or `mod.rs`)* | Modified | Store + wrap + test (whichever file holds `build_request` / `stream_deltas`). |
| `src/providers/protocols/registry.rs` | Modified | Builtin-factory path uses `build_provider_http_client()`. |
| `src/providers/protocols/loader.rs` | Modified | `ConfigurableProtocol::new` gets `build_provider_http_client()`. |

---

## Task 1: Create the shared `stream_idle` module

**Files:**
- Create: `src/providers/protocols/stream_idle.rs`
- Modify: `src/providers/protocols/mod.rs`

- [ ] **Step 1: Create `src/providers/protocols/stream_idle.rs`**

First read `src/providers/protocols/anthropic/adapter.rs` lines 679-699 (the current `wrap_idle_timeout`) and its 3 tests (`wrap_idle_timeout_fires_after_threshold`, `wrap_idle_timeout_resets_on_event`, `wrap_idle_timeout_zero_disables`, around lines 986-1043) to copy their exact bodies. Then create the new file:

```rust
//! Per-chunk idle timeout for streaming LLM responses.
//!
//! A streaming response can stall mid-flight — the upstream stops sending SSE
//! bytes after a network blip or proxy hang — and the agent turn would hang
//! indefinitely. `wrap_idle_timeout` aborts the stream when no chunk arrives
//! within `idle_secs`, surfacing `AlephError::Timeout` so the caller's
//! existing transient-error path handles it.

use axum::body::Bytes;
use futures::stream::BoxStream;

use crate::error::{AlephError, Result};

/// Built-in idle timeout (seconds) when `ProviderConfig.stream_idle_timeout_secs`
/// is unset. A stalled SSE stream that sends no byte for this long is aborted.
pub(crate) const DEFAULT_STREAM_IDLE_SECS: u64 = 60;

/// Resolve the effective idle timeout from provider config: the configured
/// value, or `DEFAULT_STREAM_IDLE_SECS` when unset.
pub(crate) fn effective_idle_secs(config: &crate::config::ProviderConfig) -> u64 {
    config
        .stream_idle_timeout_secs
        .unwrap_or(DEFAULT_STREAM_IDLE_SECS)
}

/// Wrap a byte stream so a gap longer than `idle_secs` between chunks yields
/// `AlephError::Timeout`. `idle_secs == 0` disables the wrap (returns the
/// stream unchanged). `provider_label` names the upstream in the error
/// message (e.g. `"OpenAI"`, `"Gemini"`, `"Anthropic"`).
pub(crate) fn wrap_idle_timeout(
    stream: BoxStream<'static, Result<Bytes>>,
    idle_secs: u64,
    provider_label: &'static str,
) -> BoxStream<'static, Result<Bytes>> {
    if idle_secs == 0 {
        return stream;
    }
    use tokio_stream::StreamExt as _;
    let timed = stream.timeout(std::time::Duration::from_secs(idle_secs));
    let mapped = futures::StreamExt::map(timed, move |res| match res {
        Ok(inner) => inner,
        Err(_elapsed) => Err(AlephError::Timeout {
            suggestion: Some(format!(
                "{provider_label} stream stalled: no SSE event received for \
                 {idle_secs}s. The upstream may be unresponsive; retry or \
                 increase ProviderConfig.stream_idle_timeout_secs."
            )),
        }),
    });
    Box::pin(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;

    #[tokio::test]
    async fn idle_timeout_fires_after_threshold() {
        // A stream that never yields — must trip the idle timeout.
        let pending: BoxStream<'static, Result<Bytes>> = futures::stream::pending().boxed();
        let mut wrapped = wrap_idle_timeout(pending, 1, "TestProvider");
        let first = wrapped.next().await;
        assert!(
            matches!(first, Some(Err(AlephError::Timeout { .. }))),
            "stalled stream must yield AlephError::Timeout, got {first:?}",
        );
    }

    #[tokio::test]
    async fn idle_timeout_resets_on_event() {
        // A stream that yields one chunk immediately then ends — the single
        // chunk arrives well within the 10s window, so no timeout.
        let s: BoxStream<'static, Result<Bytes>> =
            futures::stream::once(async { Ok(Bytes::from_static(b"data: x\n")) }).boxed();
        let mut wrapped = wrap_idle_timeout(s, 10, "TestProvider");
        let first = wrapped.next().await;
        assert!(
            matches!(first, Some(Ok(_))),
            "a prompt chunk must pass through, got {first:?}",
        );
    }

    #[tokio::test]
    async fn idle_timeout_zero_disables() {
        // idle_secs == 0 returns the stream unchanged — a pending stream stays
        // pending (we only assert the wrap did not inject a Timeout quickly).
        let pending: BoxStream<'static, Result<Bytes>> = futures::stream::pending().boxed();
        let mut wrapped = wrap_idle_timeout(pending, 0, "TestProvider");
        let raced = tokio::time::timeout(std::time::Duration::from_millis(50), wrapped.next()).await;
        assert!(raced.is_err(), "idle_secs==0 must not inject any timeout");
    }

    #[tokio::test]
    async fn timeout_message_carries_provider_label() {
        let pending: BoxStream<'static, Result<Bytes>> = futures::stream::pending().boxed();
        let mut wrapped = wrap_idle_timeout(pending, 1, "Gemini");
        match wrapped.next().await {
            Some(Err(AlephError::Timeout { suggestion: Some(msg) })) => {
                assert!(msg.contains("Gemini"), "label must appear in: {msg}");
            }
            other => panic!("expected labelled Timeout, got {other:?}"),
        }
    }

    #[test]
    fn effective_idle_secs_defaults_to_60_when_unset() {
        let config = crate::config::ProviderConfig::test_config("any-model");
        // test_config leaves stream_idle_timeout_secs = None
        assert_eq!(effective_idle_secs(&config), 60);
    }

    #[test]
    fn effective_idle_secs_uses_configured_value() {
        let mut config = crate::config::ProviderConfig::test_config("any-model");
        config.stream_idle_timeout_secs = Some(15);
        assert_eq!(effective_idle_secs(&config), 15);
    }
}
```

NOTE: confirm `crate::config::ProviderConfig::test_config(&str)` exists (it is used in `anthropic/adapter.rs` tests — e.g. `ProviderConfig::test_config("claude-3-5-sonnet")`). If the constructor name differs, match the real one. Confirm `AlephError::Timeout` is a struct variant with a `suggestion: Option<String>` field (it is — see the current `wrap_idle_timeout`).

- [ ] **Step 2: Register the module**

In `src/providers/protocols/mod.rs`, add alongside the other `mod` lines (the file has `pub mod anthropic;` … `pub mod template;` and a private `mod jsonpath;`). Add a private declaration — the module's items are `pub(crate)`:

```rust
mod stream_idle;
```

Place it in alphabetical position (after `pub mod registry;`, before `pub mod template;` — or wherever alphabetical order puts it relative to neighbors; match the file's existing ordering).

- [ ] **Step 3: Run the tests (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib providers::protocols::stream_idle 2>&1 | tail -20
```

Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/providers/protocols/stream_idle.rs src/providers/protocols/mod.rs
git commit -m "providers: add shared stream_idle module (generalized wrap_idle_timeout)"
```

---

## Task 2: Switch the Anthropic adapter to the shared helper

**Files:**
- Modify: `src/providers/protocols/anthropic/adapter.rs`

- [ ] **Step 1: Delete the local `wrap_idle_timeout` and its 3 tests**

In `src/providers/protocols/anthropic/adapter.rs`:
- Delete the `fn wrap_idle_timeout(...)` definition (currently lines ~679-699).
- Delete the three `#[cfg(test)]` tests `wrap_idle_timeout_fires_after_threshold`, `wrap_idle_timeout_resets_on_event`, `wrap_idle_timeout_zero_disables` (currently in the `mod tests` block, ~lines 986-1043). Read the file to find their exact current line ranges — do not delete neighbouring tests.

- [ ] **Step 2: Call the shared helper at the streaming site**

The streaming site currently reads (around line 545-548):

```rust
        let idle_secs = self
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed);
        let byte_stream = wrap_idle_timeout(byte_stream, idle_secs);
```

Change the last line to call the shared function with the `"Anthropic"` label:

```rust
        let byte_stream = crate::providers::protocols::stream_idle::wrap_idle_timeout(
            byte_stream,
            idle_secs,
            "Anthropic",
        );
```

Anthropic's `stream_idle_timeout_secs` `AtomicU64` field and its `build_request` store (`self.stream_idle_timeout_secs.store(config.stream_idle_timeout_secs.unwrap_or(60), ...)`) are unchanged — keep them.

- [ ] **Step 3: Run the Anthropic adapter tests (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib providers::protocols::anthropic 2>&1 | tail -25
```

Expected: all remaining Anthropic tests pass (the 3 deleted `wrap_idle_timeout_*` tests now live in `stream_idle.rs`). No compile error referencing the removed local function.

- [ ] **Step 4: Commit**

```bash
git add src/providers/protocols/anthropic/adapter.rs
git commit -m "providers/anthropic: use shared stream_idle::wrap_idle_timeout"
```

---

## Task 3: OpenAI Chat — per-chunk idle timeout

**Files:**
- Modify: `src/providers/protocols/openai_chat.rs` (struct)
- Modify: `src/providers/protocols/openai_chat/proto_impl.rs` (`new()`)
- Modify: `src/providers/protocols/openai_chat/adapter.rs` (`build_request` store + `stream_deltas` wrap + test)

- [ ] **Step 1: Add the field to `OpenAiProtocol`**

In `src/providers/protocols/openai_chat.rs`, the struct is currently:

```rust
pub struct OpenAiProtocol {
    client: Client,
}
```

Change it to:

```rust
pub struct OpenAiProtocol {
    client: Client,
    /// Idle timeout (seconds) for the SSE byte stream, resolved from
    /// `ProviderConfig.stream_idle_timeout_secs` in `build_request` and read
    /// in `stream_deltas`. An `AtomicU64` because `&self` is shared (`Arc`)
    /// and the value must cross into the `'static` stream closure.
    stream_idle_timeout_secs: std::sync::Arc<std::sync::atomic::AtomicU64>,
}
```

- [ ] **Step 2: Initialize the field in `new()`**

In `src/providers/protocols/openai_chat/proto_impl.rs`, `new()` is currently:

```rust
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
```

Change to:

```rust
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            stream_idle_timeout_secs: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
                crate::providers::protocols::stream_idle::DEFAULT_STREAM_IDLE_SECS,
            )),
        }
    }
```

- [ ] **Step 3: Store the config value in `build_request`**

In `src/providers/protocols/openai_chat/adapter.rs`, find the `impl ProtocolAdapter for OpenAiProtocol` block's `fn build_request(&self, payload: ..., config: &ProviderConfig) -> ...`. At the very top of the method body (mirroring `anthropic/adapter.rs`'s `build_request`), add:

```rust
        self.stream_idle_timeout_secs.store(
            crate::providers::protocols::stream_idle::effective_idle_secs(config),
            std::sync::atomic::Ordering::Relaxed,
        );
```

- [ ] **Step 4: Wrap the byte stream in `stream_deltas`**

In the same file, `stream_deltas` currently builds the byte stream:

```rust
        // Wrap the bytes stream in an AlephError-typed stream
        let byte_stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
            .boxed();
```

Immediately after that statement, add:

```rust
        let idle_secs = self
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed);
        let byte_stream = crate::providers::protocols::stream_idle::wrap_idle_timeout(
            byte_stream,
            idle_secs,
            "OpenAI",
        );
```

The `byte_stream` binding type is `BoxStream<'static, Result<axum::body::Bytes>>` — the same type `wrap_idle_timeout` takes and returns, so the shadowing rebind type-checks. If the existing `.boxed()` produces a different `Bytes` type, adjust — the shared function is fixed to `axum::body::Bytes` (confirmed against the Anthropic adapter, which uses `axum::body::Bytes`).

- [ ] **Step 5: Write the store test**

Add to the `#[cfg(test)] mod tests` in `src/providers/protocols/openai_chat/adapter.rs` (read the file to find the test module; if `adapter.rs` has no test module, add one, or place the test in `src/providers/protocols/openai_chat/tests.rs` which already exists). Model the `build_request` call on the existing `build_request_*` tests in the Anthropic suite:

```rust
#[test]
fn build_request_stores_configured_stream_idle_timeout() {
    let proto = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = crate::config::ProviderConfig::test_config("gpt-4o");
    config.stream_idle_timeout_secs = Some(17);
    let payload = crate::providers::adapter::RequestPayload::new(&[]);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        17,
    );
}

#[test]
fn build_request_defaults_stream_idle_timeout_to_60() {
    let proto = OpenAiProtocol::new(reqwest::Client::new());
    let config = crate::config::ProviderConfig::test_config("gpt-4o");
    // stream_idle_timeout_secs is None in test_config
    let payload = crate::providers::adapter::RequestPayload::new(&[]);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        60,
    );
}
```

NOTE: confirm the exact `RequestPayload` constructor and the `build_request` signature/import path against the existing Anthropic `build_request_*` tests in `src/providers/protocols/anthropic.rs` — copy their setup idiom verbatim (they already construct a payload + config and call `build_request`). `build_request` returning `Err` on an empty payload is fine: the `.store()` runs at the top of the method before any fallible work, so the assertion holds regardless of the `Result`.

- [ ] **Step 6: Run the tests (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib providers::protocols::openai_chat 2>&1 | tail -25
```

Expected: the 2 new tests pass; all pre-existing openai_chat tests still pass.

- [ ] **Step 7: Commit**

```bash
git add src/providers/protocols/openai_chat.rs src/providers/protocols/openai_chat/proto_impl.rs src/providers/protocols/openai_chat/adapter.rs src/providers/protocols/openai_chat/tests.rs
git commit -m "providers/openai_chat: per-chunk SSE idle timeout"
```

(Drop `tests.rs` from the `git add` if you placed the test inside `adapter.rs` instead.)

---

## Task 4: OpenAI Responses — per-chunk idle timeout

**Files:**
- Modify: `src/providers/protocols/openai_responses/mod.rs` (struct + `new()`)
- Modify: `src/providers/protocols/openai_responses/adapter.rs` *(or wherever `build_request` / `stream_deltas` live — read the directory first)*

- [ ] **Step 1: Add the field to `OpenAiResponsesProtocol`**

In `src/providers/protocols/openai_responses/mod.rs`, the struct + `new()` are currently:

```rust
pub struct OpenAiResponsesProtocol {
    client: Client,
    variant: ResponsesVariant,
}

impl OpenAiResponsesProtocol {
    pub fn new(client: Client, variant: ResponsesVariant) -> Self {
        Self { client, variant }
    }
```

Change to:

```rust
pub struct OpenAiResponsesProtocol {
    client: Client,
    variant: ResponsesVariant,
    /// Idle timeout (seconds) for the SSE byte stream — see `stream_idle`.
    stream_idle_timeout_secs: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl OpenAiResponsesProtocol {
    pub fn new(client: Client, variant: ResponsesVariant) -> Self {
        Self {
            client,
            variant,
            stream_idle_timeout_secs: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
                crate::providers::protocols::stream_idle::DEFAULT_STREAM_IDLE_SECS,
            )),
        }
    }
```

- [ ] **Step 2: Store the config value in `build_request`**

Locate the `impl ProtocolAdapter for OpenAiResponsesProtocol` block and its `build_request(&self, payload, config: &ProviderConfig)`. At the top of the method body add:

```rust
        self.stream_idle_timeout_secs.store(
            crate::providers::protocols::stream_idle::effective_idle_secs(config),
            std::sync::atomic::Ordering::Relaxed,
        );
```

- [ ] **Step 3: Wrap the byte stream in `stream_deltas`**

Find `stream_deltas` (the `response.bytes_stream()` site is around `openai_responses/mod.rs:311` per the spec — but it may be in an `adapter.rs`; grep `bytes_stream` under `src/providers/protocols/openai_responses/`). After the `let byte_stream = response.bytes_stream()....boxed();` statement, add:

```rust
        let idle_secs = self
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed);
        let byte_stream = crate::providers::protocols::stream_idle::wrap_idle_timeout(
            byte_stream,
            idle_secs,
            "OpenAI",
        );
```

- [ ] **Step 4: Write the store test**

Add to the test module that covers this protocol (`src/providers/protocols/openai_responses/tests.rs` exists — use it):

```rust
#[test]
fn build_request_stores_configured_stream_idle_timeout() {
    let proto = OpenAiResponsesProtocol::new(
        reqwest::Client::new(),
        ResponsesVariant::default(),
    );
    let mut config = crate::config::ProviderConfig::test_config("gpt-4o");
    config.stream_idle_timeout_secs = Some(23);
    let payload = crate::providers::adapter::RequestPayload::new(&[]);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        23,
    );
}

#[test]
fn build_request_defaults_stream_idle_timeout_to_60() {
    let proto = OpenAiResponsesProtocol::new(
        reqwest::Client::new(),
        ResponsesVariant::default(),
    );
    let config = crate::config::ProviderConfig::test_config("gpt-4o");
    let payload = crate::providers::adapter::RequestPayload::new(&[]);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        60,
    );
}
```

Confirm `ResponsesVariant::default()` and the `RequestPayload` / `build_request` import paths against the existing tests in `openai_responses/tests.rs`; adapt the constructor call to match what that file already does.

- [ ] **Step 5: Run the tests (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib providers::protocols::openai_responses 2>&1 | tail -25
```

Expected: the 2 new tests pass; all pre-existing openai_responses tests still pass.

- [ ] **Step 6: Commit**

```bash
git add src/providers/protocols/openai_responses/
git commit -m "providers/openai_responses: per-chunk SSE idle timeout"
```

---

## Task 5: Gemini — per-chunk idle timeout

**Files:**
- Modify: `src/providers/protocols/gemini.rs` (struct)
- Modify: `src/providers/protocols/gemini/proto_impl.rs` (`new()`)
- Modify: `src/providers/protocols/gemini/adapter.rs` (`build_request` store + `stream_deltas` wrap + test)

- [ ] **Step 1: Add the field to `GeminiProtocol`**

In `src/providers/protocols/gemini.rs`, the struct is currently:

```rust
pub struct GeminiProtocol {
    client: Client,
}
```

Change it to:

```rust
pub struct GeminiProtocol {
    client: Client,
    /// Idle timeout (seconds) for the SSE byte stream — see `stream_idle`.
    stream_idle_timeout_secs: std::sync::Arc<std::sync::atomic::AtomicU64>,
}
```

- [ ] **Step 2: Initialize the field in `new()`**

In `src/providers/protocols/gemini/proto_impl.rs`, `new()` is currently:

```rust
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
```

Change to:

```rust
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            stream_idle_timeout_secs: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
                crate::providers::protocols::stream_idle::DEFAULT_STREAM_IDLE_SECS,
            )),
        }
    }
```

- [ ] **Step 3: Store the config value in `build_request`**

In `src/providers/protocols/gemini/adapter.rs`, find `impl ProtocolAdapter for GeminiProtocol`'s `fn build_request(&self, payload, config: &ProviderConfig)`. At the top of the method body add:

```rust
        self.stream_idle_timeout_secs.store(
            crate::providers::protocols::stream_idle::effective_idle_secs(config),
            std::sync::atomic::Ordering::Relaxed,
        );
```

- [ ] **Step 4: Wrap the byte stream in `stream_deltas`**

In the same file, `stream_deltas`'s byte-stream site is around `gemini/adapter.rs:175` (`.bytes_stream()`). Read the surrounding lines to find the full `let byte_stream = response.bytes_stream()....boxed();` statement, then immediately after it add:

```rust
        let idle_secs = self
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed);
        let byte_stream = crate::providers::protocols::stream_idle::wrap_idle_timeout(
            byte_stream,
            idle_secs,
            "Gemini",
        );
```

If the existing byte-stream binding has a different name than `byte_stream`, use that name in the `wrap_idle_timeout` call and the rebind.

- [ ] **Step 5: Write the store test**

Add to the Gemini test module (`src/providers/protocols/gemini/tests.rs` exists):

```rust
#[test]
fn build_request_stores_configured_stream_idle_timeout() {
    let proto = GeminiProtocol::new(reqwest::Client::new());
    let mut config = crate::config::ProviderConfig::test_config("gemini-1.5-pro");
    config.stream_idle_timeout_secs = Some(31);
    let payload = crate::providers::adapter::RequestPayload::new(&[]);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        31,
    );
}

#[test]
fn build_request_defaults_stream_idle_timeout_to_60() {
    let proto = GeminiProtocol::new(reqwest::Client::new());
    let config = crate::config::ProviderConfig::test_config("gemini-1.5-pro");
    let payload = crate::providers::adapter::RequestPayload::new(&[]);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        60,
    );
}
```

Confirm the `RequestPayload` / `build_request` import paths against the existing tests in `gemini/tests.rs`.

- [ ] **Step 6: Run the tests (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib providers::protocols::gemini 2>&1 | tail -25
```

Expected: the 2 new tests pass; all pre-existing gemini tests still pass.

- [ ] **Step 7: Commit**

```bash
git add src/providers/protocols/gemini.rs src/providers/protocols/gemini/proto_impl.rs src/providers/protocols/gemini/adapter.rs src/providers/protocols/gemini/tests.rs
git commit -m "providers/gemini: per-chunk SSE idle timeout"
```

---

## Task 6: Create the shared HTTP client builder

**Files:**
- Create: `src/providers/protocols/http_client.rs`
- Modify: `src/providers/protocols/mod.rs`

- [ ] **Step 1: Create `src/providers/protocols/http_client.rs`**

```rust
//! Shared reqwest client construction for LLM provider protocols.

use std::time::Duration;

/// Build the HTTP client used by every provider protocol.
///
/// Sets connection-level timeouts so a stale pooled keep-alive connection
/// cannot hang a request's handshake without bound:
/// - `connect_timeout` caps the TCP+TLS handshake;
/// - `pool_idle_timeout` evicts idle keep-alive connections before a NAT or
///   proxy silently drops them half-open;
/// - `tcp_keepalive` lets the OS detect a dead peer on a long-lived stream.
///
/// Deliberately sets NO overall request `.timeout()` — streaming responses are
/// long-lived and an overall cap would kill a legitimately long stream.
/// Mid-stream stalls are handled separately by `stream_idle::wrap_idle_timeout`.
pub(crate) fn build_provider_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        // Fail-soft: a builder error is implausible with these options, but a
        // default client beats a panic at provider-construction time.
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_without_panicking() {
        // reqwest does not expose configured timeout values for assertion;
        // verify the builder succeeds with these options.
        let _client = build_provider_http_client();
    }
}
```

- [ ] **Step 2: Register the module**

In `src/providers/protocols/mod.rs`, add (alphabetical position — after `mod stream_idle;` from Task 1 / near `http_*`):

```rust
mod http_client;
```

- [ ] **Step 3: Run the test (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib providers::protocols::http_client 2>&1 | tail -15
```

Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add src/providers/protocols/http_client.rs src/providers/protocols/mod.rs
git commit -m "providers: add build_provider_http_client with connection-level timeouts"
```

---

## Task 7: Point the registry and loader at the shared builder

**Files:**
- Modify: `src/providers/protocols/registry.rs`
- Modify: `src/providers/protocols/loader.rs`

- [ ] **Step 1: Registry — use the builder**

In `src/providers/protocols/registry.rs`, the builtin-protocol factory path currently reads (around line 125-128):

```rust
            .map(|factory| {
                let client = Client::new();
                factory(client)
            })
```

Change the client construction:

```rust
            .map(|factory| {
                let client = crate::providers::protocols::http_client::build_provider_http_client();
                factory(client)
            })
```

(The `use reqwest::Client;` import at the top of `registry.rs` may become unused after this change — if the compiler warns, remove it. If `Client` is still referenced elsewhere in the file, keep it.)

- [ ] **Step 2: Loader — use the builder**

In `src/providers/protocols/loader.rs`, line 65 currently:

```rust
        let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new())?;
```

Change to:

```rust
        let protocol = ConfigurableProtocol::new(
            def.clone(),
            crate::providers::protocols::http_client::build_provider_http_client(),
        )?;
```

- [ ] **Step 3: Build + run provider tests (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib providers::protocols 2>&1 | tail -30
```

Expected: clean compile; all `providers::protocols` tests pass (registry, loader, the 4 protocols, stream_idle, http_client).

- [ ] **Step 4: Commit**

```bash
git add src/providers/protocols/registry.rs src/providers/protocols/loader.rs
git commit -m "providers: registry + loader build clients with connection timeouts"
```

---

## Task 8: Final review + merge

**Files:** (none — git + memory)

- [ ] **Step 1: Whole-crate check (gated, background)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo check -p alephcore --lib --tests 2>&1 | tail -20
```

Expected: clean.

- [ ] **Step 2: Full provider test sweep (gated, background)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib providers:: 2>&1 | tail -30
```

Expected: all `providers::` tests green (modulo any pre-existing baseline failures unrelated to this work).

- [ ] **Step 3: Merge latest main into the worktree branch**

```bash
git fetch origin
git merge main --no-edit
```

If conflicts: resolve, re-run Steps 1-2 before continuing.

- [ ] **Step 4: Fast-forward main**

After ExitWorktree (keep), from the main repo:

```bash
git merge-base --is-ancestor main worktree-feat-stale-stream-killer \
  && git merge worktree-feat-stale-stream-killer --ff-only \
  || echo "branch not a superset of main — re-sync main into the branch first"
```

If it is not a fast-forward, re-enter the worktree, `git merge main`, re-test, exit, retry the fast-forward.

- [ ] **Step 5: Update memory**

Create `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_stale_stream_killer_cycle4.md` (type=project) summarizing A + B, commit SHAs, the explicit exclusion of the client-rebuild breaker, and tests-green snapshot. Add a one-line entry to the top of `MEMORY.md`.

- [ ] **Step 6: Worktree cleanup**

Per project convention (CLAUDE.md), do NOT `git worktree remove` inside the EnterWorktree session — clean up in a new session, or via `ExitWorktree action: remove` once the branch is fully merged.

---

## Self-Review Notes

**Spec coverage:**
- §A1 shared module → Task 1.
- §A2 Anthropic switch → Task 2.
- §A3 OpenAI Chat / Responses / Gemini → Tasks 3, 4, 5.
- §B1 `http_client.rs` → Task 6.
- §B2 registry, §B3 loader → Task 7.
- §Testing: `stream_idle` unit tests (Task 1), per-protocol store tests (Tasks 3-5), `http_client` test (Task 6).
- §"Out of scope": no task implements a client-rebuild breaker or an overall `.timeout()` — correctly absent.

**Type consistency:**
- `stream_idle_timeout_secs: std::sync::Arc<std::sync::atomic::AtomicU64>` — identical across Tasks 3, 4, 5 and matches Anthropic's existing field.
- `wrap_idle_timeout(stream, idle_secs, provider_label)` — 3-arg signature defined in Task 1, called identically in Tasks 2, 3, 4, 5.
- `effective_idle_secs(config)` / `DEFAULT_STREAM_IDLE_SECS` — defined Task 1, used Tasks 3-5 (init + store).
- `build_provider_http_client()` — defined Task 6, called Task 7.
- Byte type fixed at `axum::body::Bytes` throughout (confirmed against the Anthropic adapter).

**Placeholder scan:** No TBD/TODO. Three execution-time verification notes are explicit (not vague): confirm `ProviderConfig::test_config` constructor name; confirm `RequestPayload::new` / `build_request` import idiom against existing protocol tests; locate `openai_responses` `build_request`/`stream_deltas` file (`mod.rs` vs `adapter.rs`). Each names exactly what to check and where the answer lives.
