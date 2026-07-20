# Stale-Stream Killer — Design Spec

**Cycle**: Cycle 4 — Long-task hardening follow-up
**Date**: 2026-05-21
**Scope**: Item 3 of 4 deferred from [Cycle 2](./2026-05-20-long-task-hardening-design.md)
**Net LOC estimate**: +250 / −40

## Problem Statement

A streaming LLM response can stall mid-flight — the server stops sending SSE
bytes after a network blip, a proxy hang, or an upstream incident — and the
agent turn hangs indefinitely. Two concrete gaps in Aleph today:

1. **Per-chunk idle timeout exists for only one protocol.** `wrap_idle_timeout`
   in `src/providers/protocols/anthropic/adapter.rs` wraps the Anthropic SSE
   byte stream: if no chunk arrives within `idle_secs`, it yields
   `AlephError::Timeout`. The other three streaming protocols — OpenAI Chat,
   OpenAI Responses, Gemini — each call `response.bytes_stream()` directly with
   **no idle timeout**. A stalled stream on those protocols hangs until the
   harness `turn_timeout` fires, and `turn_timeout` is opt-in (default `None`).

2. **Provider HTTP clients have no connection-level timeouts.** The protocol
   registry builds clients with a bare `reqwest::Client::new()`
   (`src/providers/protocols/registry.rs`, in the builtin-protocol factory
   path) — and so does the dynamic-protocol loader
   (`src/providers/protocols/loader.rs`). `Client::new()` sets no
   `connect_timeout`, no `pool_idle_timeout`, no `tcp_keepalive`. A stale
   pooled keep-alive connection can hang the TCP/TLS handshake of the next
   request with no upper bound. (Ollama-native and `oauth_refresh` already use
   `Client::builder().timeout(...)`, but the main protocol path does not.)

## Scope

### In Scope (Cycle 4)

- **A — Universal SSE idle timeout.** Extract `wrap_idle_timeout` into a shared
  module; apply it to the OpenAI Chat, OpenAI Responses, and Gemini streaming
  paths, mirroring the `AtomicU64` pattern Anthropic already uses. Built-in
  default of 60s when `ProviderConfig.stream_idle_timeout_secs` is unset.

- **B — Connection-level client timeouts.** Replace the bare
  `reqwest::Client::new()` in the registry and loader with a shared builder
  that sets `connect_timeout`, `pool_idle_timeout`, and `tcp_keepalive`.

### Out of Scope (explicitly excluded)

- **Hermes-style "rebuild the reqwest client after N consecutive failures"
  circuit breaker.** Rejected, not merely deferred. With `pool_idle_timeout`
  and `connect_timeout` set (part B), stale pooled connections are evicted or
  time out automatically and reqwest retries on a fresh connection — manual
  client rebuild is a sledgehammer for a client that previously had no
  timeouts. Part B addresses the root cause. If repeated failures still occur
  after B ships, that is a measure-first follow-up, not a blind port.
- An overall request `.timeout()` on the streaming client — would kill a
  legitimately long stream. Stream stalls are handled by part A's idle timeout.
- Markdown-skill / MCP HTTP clients — separate transport layers, not the
  LLM-provider path.

## Design A — Universal SSE Idle Timeout

### A1. Shared module `src/providers/protocols/stream_idle.rs` (new)

Move `wrap_idle_timeout` here from `anthropic/adapter.rs`, generalized so the
stall message is not Anthropic-specific:

```rust
//! Per-chunk idle timeout for streaming LLM responses.

use futures::stream::BoxStream;
use crate::error::{AlephError, Result};
use axum::body::Bytes;

/// Built-in idle timeout (seconds) when `ProviderConfig.stream_idle_timeout_secs`
/// is unset. A stalled SSE stream that sends no byte for this long is aborted.
pub(crate) const DEFAULT_STREAM_IDLE_SECS: u64 = 60;

/// Resolve the effective idle timeout from provider config.
pub(crate) fn effective_idle_secs(config: &crate::config::ProviderConfig) -> u64 {
    config
        .stream_idle_timeout_secs
        .unwrap_or(DEFAULT_STREAM_IDLE_SECS)
}

/// Wrap a byte stream so that a gap longer than `idle_secs` between chunks
/// yields `AlephError::Timeout`. `idle_secs == 0` disables the wrap (returns
/// the stream unchanged). `provider_label` names the upstream in the error
/// message (e.g. "OpenAI", "Gemini", "Anthropic").
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
```

Register the module in `src/providers/protocols/mod.rs` (`mod stream_idle;`).

### A2. Anthropic adapter — switch to the shared helper

In `src/providers/protocols/anthropic/adapter.rs`:
- Delete the local `wrap_idle_timeout` function and its three `#[cfg(test)]`
  tests (`wrap_idle_timeout_fires_after_threshold`, `..._resets_on_event`,
  `..._zero_disables`) — they move to `stream_idle.rs`.
- Import the shared `wrap_idle_timeout`; call it with `"Anthropic"` as the
  label. Anthropic already has an `AtomicU64 stream_idle_timeout_secs` (default
  60) stored from config; that mechanism is unchanged. Behavior is identical.

### A3. OpenAI Chat / OpenAI Responses / Gemini — add the pattern

For each of `OpenAiProtocol`, `OpenAiResponsesProtocol`, `GeminiProtocol`,
mirror Anthropic's mechanism exactly:

1. **Struct field** — add `stream_idle_timeout_secs: std::sync::Arc<std::sync::atomic::AtomicU64>`.
2. **`new()`** — initialize it to `Arc::new(AtomicU64::new(DEFAULT_STREAM_IDLE_SECS))`.
3. **Request-prep method** — each protocol has a method that receives
   `config: &ProviderConfig` while building the request (the same method that
   builds headers/body). Add:
   `self.stream_idle_timeout_secs.store(stream_idle::effective_idle_secs(config), Ordering::Relaxed);`
4. **`stream_deltas(&self, response)`** — immediately after the
   `response.bytes_stream()....boxed()` line, add:
   ```rust
   let idle_secs = self.stream_idle_timeout_secs.load(Ordering::Relaxed);
   let byte_stream = stream_idle::wrap_idle_timeout(byte_stream, idle_secs, "<Label>");
   ```
   where `<Label>` is `"OpenAI"`, `"OpenAI"`, `"Gemini"` respectively.

The `stream_deltas` signature stays `(&self, response)` — the `AtomicU64` is
how the config-derived value crosses into the `'static` stream closure without
threading a new parameter through the trait. This is the proven Anthropic
pattern; consistency across all four protocols is the goal.

### Why a built-in 60s default

`ProviderConfig.stream_idle_timeout_secs` is `Option<u64>` defaulting to
`None`. `effective_idle_secs` resolves `None → 60`. This means every protocol
gets stale-stream protection out of the box. A user can override per provider,
or set `0` to disable. Anthropic already defaults to 60 (its `AtomicU64` is
seeded with 60), so this is consistent — the change makes the other three
match.

## Design B — Connection-level Client Timeouts

### B1. Shared builder `src/providers/protocols/http_client.rs` (new)

```rust
//! Shared reqwest client construction for LLM provider protocols.

use std::time::Duration;

/// Build the HTTP client used by every provider protocol.
///
/// Sets connection-level timeouts so a stale pooled keep-alive connection
/// cannot hang a request's handshake without bound. Deliberately sets NO
/// overall request `.timeout()` — streaming responses are long-lived and an
/// overall cap would kill a legitimately long stream. Mid-stream stalls are
/// handled separately by `stream_idle::wrap_idle_timeout`.
pub(crate) fn build_provider_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        // Fail-soft: a builder error is implausible with these options, but
        // if it ever happens, a default client is better than a panic at
        // provider-construction time.
        .unwrap_or_else(|_| reqwest::Client::new())
}
```

Register in `src/providers/protocols/mod.rs` (`mod http_client;`).

| Setting | Value | Rationale |
|---------|-------|-----------|
| `connect_timeout` | 10s | Caps TCP+TLS handshake; a dead pooled endpoint fails fast instead of hanging. |
| `pool_idle_timeout` | 90s | Evicts idle keep-alive connections before NAT/proxy silently drops them half-open. |
| `tcp_keepalive` | 60s | OS-level probes detect a dead peer on a long-lived streaming connection. |
| overall `.timeout()` | *unset* | Streaming responses are long-lived; the A-layer idle timeout handles stalls. |

### B2. Registry — use the shared builder

In `src/providers/protocols/registry.rs`, the builtin-protocol factory path
constructs `Client::new()` before invoking the `ProtocolFactory`. Replace that
with `http_client::build_provider_http_client()`.

### B3. Loader — use the shared builder

In `src/providers/protocols/loader.rs`, `ConfigurableProtocol::new(def, reqwest::Client::new())`
becomes `ConfigurableProtocol::new(def, http_client::build_provider_http_client())`.

Test-only `reqwest::Client::new()` call sites (inside `#[cfg(test)]` modules in
`configurable.rs`, `anthropic.rs`, etc.) are left unchanged — they are not the
production path.

## R-rule Compliance

| Rule | Check |
|------|-------|
| R1 (Brain-Limb) | Provider/transport layer only; no platform-system API in core. |
| R3 (Core Minimalism) | ~250 LOC, no new dependencies (`reqwest`, `tokio-stream`, `futures` already used). Two small new files, each single-responsibility. |
| R7 / R10 | Not harness/loop code, no LLM-reasoning replacement. Pure provider I/O hardening. |

## Testing

| Layer | File | Coverage |
|-------|------|----------|
| Unit | `stream_idle.rs::tests` | The 3 migrated tests — idle fires after threshold, resets on a received event, `idle_secs == 0` disables. Plus: the `provider_label` string appears in the `AlephError::Timeout` suggestion. Plus: `effective_idle_secs` returns 60 for `None`, the set value for `Some(n)`. |
| Unit | each protocol's test module | After the request-prep method runs with a config carrying `stream_idle_timeout_secs: Some(N)`, the protocol's `AtomicU64` loads `N`; with `None` it loads 60. |
| Unit | `http_client.rs::tests` | `build_provider_http_client()` returns a client without panicking (the builder succeeds with these options). |

reqwest does not expose configured timeout values for assertion, so B's test
verifies construction succeeds rather than introspecting the values.

## Risks

| ID | Risk | Mitigation |
|----|------|------------|
| R1 | A 60s idle default trips on a slow-but-legitimate model (long thinking pause with no SSE keep-alive). | 60s is generous for inter-chunk gaps; providers send periodic SSE pings/keep-alives. User can raise `stream_idle_timeout_secs` per provider or set `0` to disable. Anthropic has run with a 60s default already. |
| R2 | `connect_timeout` 10s too tight for a slow network. | 10s is handshake-only, not request duration; well above normal TLS handshake latency. Tunable in a follow-up if measured to be a problem. |
| R3 | A protocol's request-prep method runs on a path that does not reach `stream_deltas` (non-streaming request). | The `.store()` is idempotent and cheap; a stored value with no subsequent stream is harmless. |
| R4 | Migrating Anthropic's `wrap_idle_timeout` could change its behavior. | The shared function is byte-identical logic plus a `provider_label` parameter; the 3 migrated tests prove equivalence. |

## Implementation Order

1. **A1** Create `stream_idle.rs` with `wrap_idle_timeout` + `effective_idle_secs` + `DEFAULT_STREAM_IDLE_SECS` + migrated unit tests; register module.
2. **A2** Switch `anthropic/adapter.rs` to the shared helper; delete its local copy + tests; verify Anthropic streaming tests still pass.
3. **A3a** OpenAI Chat — add field, init, store, wrap; unit test.
4. **A3b** OpenAI Responses — same.
5. **A3c** Gemini — same.
6. **B1** Create `http_client.rs` with `build_provider_http_client` + unit test; register module.
7. **B2 / B3** Point registry and loader at the shared builder.

Each step is its own commit. Worktree branch: `worktree-feat-stale-stream-killer`.

## Reference

- Cycle 3 (precedent — per-tool budget): `docs/superpowers/specs/2026-05-21-tool-budget-cost-breaker-design.md`
- Existing `wrap_idle_timeout`: `src/providers/protocols/anthropic/adapter.rs`
- Bare client construction: `src/providers/protocols/registry.rs`, `src/providers/protocols/loader.rs`
- Config field: `ProviderConfig.stream_idle_timeout_secs` in `src/config/types/provider.rs`
