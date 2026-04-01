# Provider Protocol Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor provider protocol layer to stream-first architecture with ProviderDelta, upgrade OpenAI Responses API and Anthropic protocol, consolidate Codex, and clean up dead code.

**Architecture:** All protocol adapters emit `Stream<ProviderDelta>` via a single `stream_deltas()` method, replacing both `parse_response()` and `parse_stream()`. A `DeltaCollector` bridges back to `ProviderResponse` for backward compatibility. AgentLoop consumes delta streams with a pre-wired `DeltaSink` for future end-to-end streaming.

**Tech Stack:** Rust, async-trait, futures (BoxStream, StreamExt, TryStreamExt), serde_json, reqwest, tokio

**Spec:** `docs/superpowers/specs/2026-03-25-provider-protocol-refactor-design.md`

---

## File Structure

### New Files
| File | Purpose |
|------|---------|
| `src/providers/protocols/openai_common/mod.rs` | Re-exports for shared OpenAI utilities |
| `src/providers/protocols/openai_common/tools.rs` | Tool sanitize/desanitize, tool definition formatting |
| `src/providers/protocols/openai_common/sse.rs` | SSE line buffering, `sse_line_stream()` helper |
| `src/providers/protocols/openai_chat.rs` | Renamed from `openai.rs`, refactored to use `stream_deltas()` |
| `src/providers/delta.rs` | `ProviderDelta` enum, `DeltaCollector`, `IndexIdTracker`, `DeltaSink` trait, `response_to_delta_stream()` |

### Modified Files
| File | Changes |
|------|---------|
| `src/providers/adapter.rs` | Remove `parse_response()`, `parse_stream()`, capability flags; add `stream_deltas()`; remove `is_streaming` from `build_request()` |
| `src/providers/http_provider.rs` | Use `stream_deltas()` + `DeltaCollector`, add `stream_raw()` |
| `src/providers/mod.rs` | Re-export `delta` module |
| `src/providers/protocols/mod.rs` | Export `openai_common`, `openai_chat` |
| `src/providers/protocols/registry.rs` | Update factory registrations |
| `src/providers/protocols/openai_responses.rs` | Add `ResponsesVariant`, merge Codex logic, implement `stream_deltas()`, add new API features |
| `src/providers/protocols/anthropic.rs` | Beta headers, prompt caching, service tier, full SSE via `stream_deltas()` |
| `src/providers/protocols/gemini.rs` | Implement `stream_deltas()` (mechanical, map existing SSE) |
| `src/providers/protocols/configurable.rs` | Implement `stream_deltas()` (delegate or fake-stream) |
| `src/providers/responses/types.rs` | Merge codex types; add `previous_response_id`, `context_management`, `TextConfig`, `TextFormat` |
| `src/providers/responses/shared.rs` | Remove SSE parsing functions, keep message/tool conversion |
| `src/providers/anthropic/types.rs` | Add `CacheControl`, `service_tier` to `MessagesRequest` |
| `src/agent_loop/loop_core.rs` | `LoopProvider::stream()`, Think step delta consumption, `DeltaSink` |
| `src/agent_loop/provider_bridge.rs` | Implement `stream()` for `AiProviderBridge` |
| `src/agent_loop/factory.rs` | Pass `DeltaSink` to `AgentLoop` |
| `src/agent_loop/integration_probe.rs` | Update `ProbeProvider` for stream interface |

### Deleted Files
| File | Reason |
|------|--------|
| `src/providers/protocols/codex.rs` | Merged into `openai_responses.rs` |
| `src/providers/codex/types.rs` | Merged into `responses/types.rs` |
| `src/providers/protocols/openai.rs` | Renamed to `openai_chat.rs` |

---

## Task 1: ProviderDelta + DeltaCollector Foundation

**Files:**
- Create: `src/providers/delta.rs`
- Modify: `src/providers/mod.rs` — add `pub mod delta;` and re-exports

This task adds all new types without breaking any existing code.

- [ ] **Step 1: Write tests for DeltaCollector**

In `src/providers/delta.rs`, write tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::{StopReason, TokenUsage};

    #[test]
    fn test_collector_text_only() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::TextDelta("Hello ".into()));
        c.push(ProviderDelta::TextDelta("world".into()));
        c.push(ProviderDelta::Done(StopReason::EndTurn));
        let r = c.finish();
        assert_eq!(r.text.as_deref(), Some("Hello world"));
        assert!(r.tool_calls.is_empty());
        assert_eq!(r.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn test_collector_thinking() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::ThinkingDelta("Let me think...".into()));
        c.push(ProviderDelta::TextDelta("Answer".into()));
        c.push(ProviderDelta::Done(StopReason::EndTurn));
        let r = c.finish();
        assert_eq!(r.thinking.as_deref(), Some("Let me think..."));
        assert_eq!(r.text.as_deref(), Some("Answer"));
    }

    #[test]
    fn test_collector_tool_calls() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::ToolCallStart { id: "call_1".into(), name: "search".into() });
        c.push(ProviderDelta::ToolCallArgDelta { id: "call_1".into(), delta: r#"{"q":"#.into() });
        c.push(ProviderDelta::ToolCallArgDelta { id: "call_1".into(), delta: r#""rust"}"#.into() });
        c.push(ProviderDelta::ToolCallEnd { id: "call_1".into() });
        c.push(ProviderDelta::Done(StopReason::ToolUse));
        let r = c.finish();
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "search");
        assert_eq!(r.tool_calls[0].arguments, serde_json::json!({"q": "rust"}));
        assert_eq!(r.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn test_collector_malformed_tool_args_fallback() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::ToolCallStart { id: "call_1".into(), name: "test".into() });
        c.push(ProviderDelta::ToolCallArgDelta { id: "call_1".into(), delta: "not json{".into() });
        c.push(ProviderDelta::ToolCallEnd { id: "call_1".into() });
        c.push(ProviderDelta::Done(StopReason::ToolUse));
        let r = c.finish();
        assert_eq!(r.tool_calls.len(), 1);
        // Fallback to Value::String for unparseable args
        assert!(r.tool_calls[0].arguments.is_string());
    }

    #[test]
    fn test_collector_usage() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::TextDelta("Hi".into()));
        c.push(ProviderDelta::Usage(TokenUsage { input_tokens: 10, output_tokens: 5, cache_read_tokens: Some(3) }));
        c.push(ProviderDelta::Done(StopReason::EndTurn));
        let r = c.finish();
        let u = r.usage.unwrap();
        assert_eq!(u.input_tokens, 10);
        assert_eq!(u.output_tokens, 5);
        assert_eq!(u.cache_read_tokens, Some(3));
    }

    #[test]
    fn test_collector_multiple_tool_calls() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::TextDelta("Running tools.".into()));
        c.push(ProviderDelta::ToolCallStart { id: "c1".into(), name: "search".into() });
        c.push(ProviderDelta::ToolCallArgDelta { id: "c1".into(), delta: r#"{"q":"a"}"#.into() });
        c.push(ProviderDelta::ToolCallEnd { id: "c1".into() });
        c.push(ProviderDelta::ToolCallStart { id: "c2".into(), name: "fetch".into() });
        c.push(ProviderDelta::ToolCallArgDelta { id: "c2".into(), delta: r#"{"url":"http://x"}"#.into() });
        c.push(ProviderDelta::ToolCallEnd { id: "c2".into() });
        c.push(ProviderDelta::Done(StopReason::ToolUse));
        let r = c.finish();
        assert_eq!(r.text.as_deref(), Some("Running tools."));
        assert_eq!(r.tool_calls.len(), 2);
        assert_eq!(r.tool_calls[0].name, "search");
        assert_eq!(r.tool_calls[1].name, "fetch");
    }

    #[test]
    fn test_index_id_tracker() {
        let mut t = IndexIdTracker::new();
        t.track(0, "call_abc".into());
        t.track(1, "call_def".into());
        assert_eq!(t.get(0), Some("call_abc"));
        assert_eq!(t.get(1), Some("call_def"));
        assert_eq!(t.get(2), None);
    }

    #[test]
    fn test_response_to_delta_stream() {
        use futures::StreamExt;
        let response = ProviderResponse {
            text: Some("Hello".into()),
            tool_calls: vec![],
            thinking: None,
            stop_reason: StopReason::EndTurn,
            usage: None,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let deltas: Vec<_> = rt.block_on(async {
            response_to_delta_stream(response).collect::<Vec<_>>().await
        });
        assert!(deltas.len() >= 2); // TextDelta + Done
        assert!(matches!(&deltas[0], Ok(ProviderDelta::TextDelta(t)) if t == "Hello"));
        assert!(matches!(&deltas.last().unwrap(), Ok(ProviderDelta::Done(StopReason::EndTurn))));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib delta::tests -- --nocapture 2>&1 | head -20`
Expected: Compilation error — module `delta` doesn't exist yet.

- [ ] **Step 3: Implement ProviderDelta, DeltaCollector, IndexIdTracker, DeltaSink, response_to_delta_stream**

Create `src/providers/delta.rs` with the full implementation per spec Section 1 + Section 5 test helper + Section 8 I4/I5 fixes. Key implementation details:

```rust
//! Streaming delta types for provider protocol output.

use std::collections::HashMap;
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde_json::Value;
use tracing::warn;

use crate::providers::adapter::{NativeToolCall, ProviderResponse, StopReason, TokenUsage};

/// Fine-grained streaming event from any AI provider.
///
/// Each variant maps to specific SSE event types across protocols.
///
/// Error semantics:
/// - `ProviderDelta::Error(msg)` = provider-level semantic error (Anthropic `error` SSE event,
///   OpenAI `response.failed`). Stream may continue; consumer decides whether to abort.
/// - `Result::Err` wrapping a delta = infrastructure failure (HTTP disconnect, invalid SSE
///   framing, UTF-8 error). Stream is broken; unrecoverable.
#[derive(Debug, Clone)]
pub enum ProviderDelta { /* ... per spec ... */ }

/// Collects ProviderDelta events into a complete ProviderResponse.
pub struct DeltaCollector {
    text: String,
    thinking: String,
    tool_calls: Vec<(String, String, String)>, // (id, name, accumulated_args) — preserves order
    usage: Option<TokenUsage>,
    stop_reason: StopReason,
}

/// Tracks index → id mapping for streaming tool calls.
///
/// Used by OpenAI Chat (tool_calls[index]) and Anthropic (content_block index).
pub struct IndexIdTracker {
    map: HashMap<u64, String>,
}

/// Receives ProviderDelta events for real-time forwarding.
/// Phase 1: NoopSink. Phase 2: connects to ReplyEmitter.
#[async_trait]
pub trait DeltaSink: Send + Sync {
    async fn on_delta(&self, delta: &ProviderDelta);
}

pub struct NoopSink;

/// Convert a ProviderResponse into a one-shot delta stream.
/// Used by MockProvider in tests and fallback bridge path.
pub fn response_to_delta_stream(response: ProviderResponse) -> BoxStream<'static, anyhow::Result<ProviderDelta>> { /* ... per spec ... */ }
```

Use `Vec<(id, name, args)>` for tool_calls in DeltaCollector to preserve insertion order (HashMap doesn't).

- [ ] **Step 4: Add module to providers/mod.rs**

Add `pub mod delta;` and re-export key types:
```rust
pub mod delta;
pub use delta::{ProviderDelta, DeltaCollector, DeltaSink, NoopSink, IndexIdTracker, response_to_delta_stream};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib delta::tests -- --nocapture`
Expected: All 7 tests PASS.

- [ ] **Step 6: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors (new module, no existing code changed).

- [ ] **Step 7: Commit**

```bash
git add src/providers/delta.rs src/providers/mod.rs
git commit -m "provider: add ProviderDelta, DeltaCollector, IndexIdTracker, DeltaSink foundation"
```

---

## Task 2: Extract openai_common Module

**Files:**
- Create: `src/providers/protocols/openai_common/mod.rs`
- Create: `src/providers/protocols/openai_common/tools.rs`
- Create: `src/providers/protocols/openai_common/sse.rs`
- Modify: `src/providers/protocols/mod.rs` — add `pub mod openai_common;`
- Modify: `src/providers/protocols/openai.rs` — change tool functions to import from `openai_common`

Extract shared logic WITHOUT changing any behavior. After this task, existing tests must still pass.

- [ ] **Step 1: Create `openai_common/tools.rs`**

Move these from `openai.rs` (lines 30-51):
- `sanitize_tool_name_pub()`
- `desanitize_tool_name_pub()`

And from `codex_utils.rs` (lines ~1-50):
- `ensure_properties_recursive()`
- `extract_codex_account_id()` — still needed by Codex variant's `build_request()` in `openai_responses.rs`

After moving, `codex_utils.rs` will be empty — delete it and remove `mod codex_utils;` from `protocols/mod.rs`.

- [ ] **Step 2: Create `openai_common/sse.rs`**

Create the SSE line buffering infrastructure:

```rust
//! SSE line buffering and stream utilities.

use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use crate::error::{AlephError, Result};

/// Build a stream of SSE data lines from an HTTP response.
///
/// Buffers incomplete lines across chunks. Strips "data: " prefix.
/// Filters out empty lines, comments, and [DONE] sentinel.
pub fn sse_line_stream(
    response: reqwest::Response,
) -> BoxStream<'static, Result<String>> {
    let buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

    let stream = response
        .bytes_stream()
        .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
        .try_filter_map(move |chunk| {
            let buf = buf.clone();
            async move {
                let text = std::str::from_utf8(&chunk)
                    .map_err(|e| AlephError::provider(format!("UTF-8 error: {}", e)))?;

                let mut buf_guard = buf.lock().unwrap_or_else(|e| e.into_inner());
                buf_guard.push_str(text);

                let mut lines = Vec::new();
                while let Some(pos) = buf_guard.find('\n') {
                    let line = buf_guard[..pos].trim_end().to_string();
                    buf_guard.drain(..=pos);
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data != "[DONE]" {
                            lines.push(data.to_string());
                        }
                    }
                }
                if lines.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(lines))
                }
            }
        })
        .map_ok(|lines| futures::stream::iter(lines.into_iter().map(Ok)))
        .try_flatten();

    Box::pin(stream)
}
```

- [ ] **Step 3: Create `openai_common/mod.rs`**

```rust
pub mod tools;
pub mod sse;
```

- [ ] **Step 4: Update `protocols/mod.rs` to export `openai_common`**

Add `pub mod openai_common;`

- [ ] **Step 5: Update `openai.rs` to import from `openai_common::tools`**

Replace the local `sanitize_tool_name_pub()` and `desanitize_tool_name_pub()` definitions with imports. Keep the private wrappers `sanitize_tool_name()` / `desanitize_tool_name()` as thin redirects if other code in the file uses them.

Update `responses/shared.rs` to import from `openai_common::tools` instead of `protocols::openai`.

- [ ] **Step 6: Run all existing tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -5`
Expected: All existing tests pass (no behavior change).

- [ ] **Step 7: Commit**

```bash
git add src/providers/protocols/openai_common/
git add src/providers/protocols/mod.rs
git add src/providers/protocols/openai.rs
git add src/providers/responses/shared.rs
git commit -m "provider: extract openai_common module (tools, sse)"
```

---

## Task 3: Upgrade Responses API Types

**Files:**
- Modify: `src/providers/responses/types.rs` — add new fields & types, merge codex types
- Modify: `src/providers/anthropic/types.rs` — add `CacheControl`, `service_tier`

No adapter logic changes — just type definitions.

- [ ] **Step 1: Add new Responses API types to `responses/types.rs`**

Add after existing types:

```rust
/// Server-side context compaction (OpenAI official endpoints only)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextManagement {
    #[serde(rename = "type")]
    pub mgmt_type: String,
}

/// Structured output config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<TextFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TextFormat {
    #[serde(rename = "json_schema")]
    JsonSchema { name: String, schema: serde_json::Value },
    #[serde(rename = "json_object")]
    JsonObject,
}
```

Add new fields to `ResponsesRequest`:

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub previous_response_id: Option<String>,

#[serde(skip_serializing_if = "Option::is_none")]
pub context_management: Option<ContextManagement>,
```

Change the `text` field from `Option<serde_json::Value>` to `Option<TextConfig>`.

Merge `TextConfig` from `codex/types.rs` (the `verbosity` field is now part of `TextConfig`).

- [ ] **Step 2: Add new Anthropic types to `anthropic/types.rs`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub control_type: String,
}
```

Add `cache_control: Option<CacheControl>` to `SystemBlock`.
Add `service_tier: Option<String>` to `MessagesRequest`.

- [ ] **Step 3: Fix compilation — update all sites that construct ResponsesRequest**

The `text` field type change from `Value` to `TextConfig` will break `openai_responses.rs` and `codex.rs`. Update both:
- `openai_responses.rs`: `text: None` (unchanged)
- `codex.rs`: `text: Some(TextConfig { format: None, verbosity: Some("medium".into()) })`

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -5`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/providers/responses/types.rs src/providers/anthropic/types.rs
git add src/providers/protocols/openai_responses.rs src/providers/protocols/codex.rs
git commit -m "provider: add Responses API and Anthropic type upgrades"
```

---

## Task 4: ProtocolAdapter Trait Migration

**Files:**
- Modify: `src/providers/adapter.rs` — add `stream_deltas()` with default impl, deprecate old methods
- Modify: All protocol adapters — add `stream_deltas()` implementations

This is the critical step. Strategy: add `stream_deltas()` as a new method with a **default implementation** that calls `parse_response()` (bridging). Then each adapter implements the real version. Once all done, remove old methods.

- [ ] **Step 1: Add `stream_deltas()` to ProtocolAdapter with default bridge impl**

In `adapter.rs`, add to the trait:

```rust
/// Stream-first output path. Default bridges via parse_response().
/// Override in each adapter for true streaming.
async fn stream_deltas(
    &self,
    response: reqwest::Response,
) -> Result<BoxStream<'static, Result<ProviderDelta>>> {
    // Default: fall back to parse_response + wrap as one-shot stream
    let provider_response = self.parse_response(response).await?;
    Ok(crate::providers::delta::response_to_delta_stream_result(provider_response))
}
```

- [ ] **Step 1b: Add `response_to_delta_stream_result()` variant in `delta.rs`**

This variant returns `BoxStream<'static, crate::error::Result<ProviderDelta>>` (using `crate::error::Result`, not `anyhow::Result`). It is used by the default `stream_deltas()` bridge and by `ConfigurableProtocol` custom mode. The existing `response_to_delta_stream()` returns `anyhow::Result` for test/bridge use.

```rust
/// Convert ProviderResponse to a one-shot stream using crate::error::Result.
/// Used by ProtocolAdapter default bridge and ConfigurableProtocol custom mode.
pub fn response_to_delta_stream_result(
    response: ProviderResponse,
) -> BoxStream<'static, crate::error::Result<ProviderDelta>> {
    // Same logic as response_to_delta_stream() but with crate::error::Result
}
```

Remove `is_streaming` from `build_request()` signature. Update the trait:

```rust
fn build_request(
    &self,
    payload: &RequestPayload,
    config: &ProviderConfig,
) -> Result<reqwest::RequestBuilder>;
```

- [ ] **Step 2: Fix all `build_request()` call sites**

Remove the `is_streaming` parameter from:
- `http_provider.rs` (~line 102) — remove the `true`/`false` arg
- `configurable.rs` — multiple call sites
- All 5 protocol adapter `impl ProtocolAdapter` blocks

Search: `grep -n "is_streaming" src/providers/` to find all sites.

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: Compiles. All adapters still have `parse_response()` and `parse_stream()` (not yet removed). The default `stream_deltas()` bridges to `parse_response()`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -5`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/providers/adapter.rs src/providers/delta.rs
git add src/providers/http_provider.rs
git add src/providers/protocols/
git commit -m "provider: add stream_deltas() to ProtocolAdapter with default bridge"
```

---

## Task 5: OpenAI Responses stream_deltas() + Codex Merge

**Files:**
- Modify: `src/providers/protocols/openai_responses.rs` — add `ResponsesVariant`, implement `stream_deltas()`, absorb Codex
- Modify: `src/providers/protocols/registry.rs` — register Codex as Responses variant
- Delete: `src/providers/protocols/codex.rs` (after merge)

- [ ] **Step 1: Write test for Responses stream_deltas()**

Add to `openai_responses.rs` tests:

```rust
#[tokio::test]
async fn test_stream_deltas_text() {
    // Simulate SSE body with text deltas
    let sse_body = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello \",\"output_index\":0,\"content_index\":0}\n\
        data: {\"type\":\"response.output_text.delta\",\"delta\":\"world\",\"output_index\":0,\"content_index\":0}\n\
        data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"status\":\"completed\",\"model\":\"gpt-4o\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}\n\
        data: [DONE]\n";
    // Use mock response with sse_body...
    // Collect stream, verify TextDelta("Hello "), TextDelta("world"), Usage(...), Done(EndTurn)
}

#[tokio::test]
async fn test_stream_deltas_tool_call() {
    // Simulate SSE body with function call
    // Verify ToolCallStart, ToolCallArgDelta, ToolCallEnd, Done(ToolUse)
}
```

- [ ] **Step 2: Implement `stream_deltas()` for OpenAiResponsesProtocol**

Use `openai_common::sse::sse_line_stream()` and map Responses API SSE events to `ProviderDelta`:

- `response.output_text.delta` → `TextDelta`
- `response.output_item.added` (function_call) → `ToolCallStart`
- `response.function_call_arguments.delta` → `ToolCallArgDelta`
- `response.function_call_arguments.done` / `response.output_item.done` (function_call) → `ToolCallEnd`
- `response.completed` → `Usage` + `Done`
- `response.failed` → `Error`

- [ ] **Step 3: Add `ResponsesVariant` struct and `codex()` constructor**

Per spec Section 3. Add to `openai_responses.rs`.

- [ ] **Step 4: Update `OpenAiResponsesProtocol::new()` to accept `ResponsesVariant`**

```rust
pub fn new(client: Client, variant: ResponsesVariant) -> Self {
    Self { client, variant }
}
```

Update `build_request()` to apply variant: endpoint path, extra headers, store, text, include.

- [ ] **Step 5: Add `previous_response_id` and `context_management` support**

In `build_responses_request()`:
- Accept `enable_server_context` flag from config
- If official OpenAI endpoint and enabled, set `store: true` + `context_management`

- [ ] **Step 6: Update protocol registry**

In `registry.rs`, change Codex registration:

```rust
// Before:
"codex" | "chatgpt" => Box::new(CodexProtocol::new(client))
// After:
"codex" | "chatgpt" => Box::new(OpenAiResponsesProtocol::new(client, ResponsesVariant::codex()))
```

- [ ] **Step 7: Delete `protocols/codex.rs`**

Remove the file. Remove `mod codex;` from `protocols/mod.rs`. Update any remaining imports.

- [ ] **Step 8: Run tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -5`
Expected: All tests pass (Codex tests moved or adapted).

- [ ] **Step 9: Commit**

```bash
git add -A src/providers/protocols/
git add src/providers/responses/
git commit -m "provider: Responses API stream_deltas() + merge Codex via ResponsesVariant"
```

---

## Task 6: Rename openai.rs → openai_chat.rs + stream_deltas()

**Files:**
- Rename: `src/providers/protocols/openai.rs` → `src/providers/protocols/openai_chat.rs`
- Modify: `src/providers/protocols/mod.rs` — update module declaration
- Modify: All files importing from `protocols::openai` — update paths

- [ ] **Step 1: Git rename**

```bash
git mv src/providers/protocols/openai.rs src/providers/protocols/openai_chat.rs
```

- [ ] **Step 2: Update module declaration in `protocols/mod.rs`**

Change `pub mod openai;` to `pub mod openai_chat;`. Update any `use` paths.

- [ ] **Step 3: Update all imports**

Search: `grep -rn "protocols::openai" src/` and update to `protocols::openai_chat`.
Also update `responses/shared.rs` which imports `openai::sanitize_tool_name_pub` (now from `openai_common::tools`).

- [ ] **Step 4: Implement `stream_deltas()` for `OpenAiChatProtocol`**

Map Chat Completions SSE to ProviderDelta using `IndexIdTracker`:
- `choices[0].delta.content` → `TextDelta`
- `choices[0].delta.tool_calls[i]` with `id` → `ToolCallStart`
- `choices[0].delta.tool_calls[i].function.arguments` → `ToolCallArgDelta`
- `finish_reason` → `Done(...)` + `ToolCallEnd` for any open tool calls
- `usage` → `Usage`

- [ ] **Step 5: Compile and test**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -5`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A src/providers/
git commit -m "provider: rename openai.rs to openai_chat.rs, implement stream_deltas()"
```

---

## Task 7: Anthropic Protocol Upgrade + stream_deltas()

**Files:**
- Modify: `src/providers/protocols/anthropic.rs` — beta headers, prompt caching, full SSE parsing

- [ ] **Step 1: Write tests for Anthropic SSE → ProviderDelta mapping**

```rust
#[tokio::test]
async fn test_anthropic_stream_deltas_text() {
    // SSE: content_block_start(text) + content_block_delta(text_delta) + content_block_stop + message_delta + message_stop
    // Expect: TextDelta + Done
}

#[tokio::test]
async fn test_anthropic_stream_deltas_tool_use() {
    // SSE: content_block_start(tool_use) + content_block_delta(input_json_delta) + content_block_stop
    // Expect: ToolCallStart + ToolCallArgDelta + ToolCallEnd + Done
}

#[tokio::test]
async fn test_anthropic_stream_deltas_thinking() {
    // SSE: content_block_start(thinking) + content_block_delta(thinking_delta) + content_block_stop
    // Expect: ThinkingDelta + Done
}
```

- [ ] **Step 2: Implement `stream_deltas()` for AnthropicProtocol**

Use `openai_common::sse::sse_line_stream()` + `AnthropicStreamState` for index→id tracking.

Map per spec Section 4.4 table. Key: `content_block_stop` only emits `ToolCallEnd` when `block_ids.contains_key(index)`.

- [ ] **Step 3: Add beta headers to `build_request()`**

```rust
.header("anthropic-beta", Self::build_beta_headers(config.default_model()))
```

Implement `build_beta_headers()` and `is_large_context_model()` per spec Section 4.1.

- [ ] **Step 4: Add prompt caching to `build_request()`**

Add `cache_control: Some(CacheControl { control_type: "ephemeral".into() })` to the last `SystemBlock`.

- [ ] **Step 5: Add service_tier to request**

Read from `config.extra_params` or a new config field, add to `MessagesRequest` if present.

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib anthropic -- --nocapture`
Expected: All Anthropic tests pass (old + new).

- [ ] **Step 7: Commit**

```bash
git add src/providers/protocols/anthropic.rs src/providers/anthropic/types.rs
git commit -m "provider: Anthropic upgrade — beta headers, prompt caching, full SSE stream_deltas()"
```

---

## Task 8: Gemini + Configurable Protocol Migration

**Files:**
- Modify: `src/providers/protocols/gemini.rs` — implement `stream_deltas()`
- Modify: `src/providers/protocols/configurable.rs` — implement `stream_deltas()`

Mechanical migration — map existing SSE parsing to ProviderDelta.

- [ ] **Step 1: Implement `stream_deltas()` for GeminiProtocol**

Gemini's streaming uses `generateContent` with SSE chunks. Map:
- `candidates[0].content.parts[i].text` → `TextDelta`
- `candidates[0].content.parts[i].functionCall` → `ToolCallStart` + `ToolCallArgDelta` + `ToolCallEnd` (Gemini returns complete function calls per chunk)
- `candidates[0].finishReason` → `Done(...)`
- `usageMetadata` → `Usage`

Gemini doesn't return tool call IDs — generate synthetic IDs (existing behavior, preserve it).

- [ ] **Step 2: Implement `stream_deltas()` for ConfigurableProtocol**

Two modes per spec Section 8 C1:
- **Minimal mode**: `self.base.stream_deltas(response).await`
- **Custom mode**: Use existing `parse_custom_response()` logic → `response_to_delta_stream_result()`

- [ ] **Step 3: Compile and test**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -5`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/providers/protocols/gemini.rs src/providers/protocols/configurable.rs
git commit -m "provider: Gemini and Configurable stream_deltas() migration"
```

---

## Task 9: HttpProvider Stream Adaptation

**Files:**
- Modify: `src/providers/http_provider.rs` — use `stream_deltas()` + `DeltaCollector`, add `stream_raw()`
- Modify: `src/providers/mod.rs` — add `as_http_provider()` to AiProvider

**IMPORTANT**: This task MUST run before Task 10 (removing old methods). HttpProvider currently calls `parse_response()` — if we remove `parse_response()` from the trait first, HttpProvider won't compile.

- [ ] **Step 1: Update `HttpProvider::execute()` (or `process()`)**

Replace the `parse_response()` call with:

```rust
let stream = self.adapter.stream_deltas(response).await?;
let mut collector = DeltaCollector::new();
futures::pin_mut!(stream);
while let Some(delta) = stream.next().await {
    collector.push(delta?);
}
let provider_response = collector.finish();
```

**Preserve ALL existing safety logic** per spec Section 8 C2:
1. PII filtering on outbound messages (BEFORE request)
2. Secret leak detection on outbound content (BEFORE request)
3. Timeout/network error mapping on `request.send()`
4. Secret leak detection on inbound response (AFTER collect)
5. **`provider_response.validate(self.adapter.name())`** (AFTER collect — this logs warnings for missing usage or unknown stop reason, currently at line ~118)

- [ ] **Step 2: Add `stream_raw()` method**

```rust
/// Expose raw delta stream with outbound safety checks applied.
/// Used by AiProviderBridge for real streaming to AgentLoop.
pub async fn stream_raw<'a>(
    &'a self,
    payload: adapter::RequestPayload<'a>,
) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
    // 1. PII filtering + leak detection (outbound)
    // 2. Build request + send
    // 3. Return adapter.stream_deltas() — inbound leak check deferred to DeltaCollector consumer
    let filtered = self.filter_pii_messages(&payload)?;
    self.check_outbound_leaks(&filtered)?;
    let request = self.adapter.build_request(&filtered, &self.config)?;
    let response = self.send_with_error_handling(request).await?;
    let stream = self.adapter.stream_deltas(response).await?;
    Ok(stream.map_err(|e| anyhow::anyhow!("{}", e)).boxed())
}
```

- [ ] **Step 3: Write test for `stream_raw()`**

Test that outbound safety checks are applied before streaming. Use a mock adapter:

```rust
#[tokio::test]
async fn test_stream_raw_applies_outbound_checks() {
    // Create HttpProvider with a mock adapter that returns a simple delta stream
    // Inject a known secret into the payload messages
    // Verify stream_raw() returns an error (blocked by leak detection)
}

#[tokio::test]
async fn test_stream_raw_returns_delta_stream() {
    // Create HttpProvider with a mock adapter
    // Verify stream_raw() returns a stream that yields expected deltas
}
```

- [ ] **Step 4: Add `as_http_provider()` downcast helper to `AiProvider`**

In `mod.rs`, add a method to `AiProvider` trait:

```rust
fn as_http_provider(&self) -> Option<&HttpProvider> { None }
```

`HttpProvider` overrides: `fn as_http_provider(&self) -> Option<&HttpProvider> { Some(self) }`

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -5`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/providers/http_provider.rs src/providers/mod.rs
git commit -m "provider: HttpProvider uses stream_deltas() + DeltaCollector, adds stream_raw()"
```

---

## Task 10: Remove Old ProtocolAdapter Methods

**Files:**
- Modify: `src/providers/adapter.rs` — remove `parse_response()`, `parse_stream()`, old capability flags
- Modify: All protocol adapters — remove old method implementations

Now that all adapters have `stream_deltas()` AND HttpProvider no longer calls `parse_response()`, safely remove the deprecated methods.

- [ ] **Step 1: Remove from trait**

In `adapter.rs`, remove:
- `parse_response()`
- `parse_stream()`
- `supports_parallel_tools()`
- `returns_tool_call_ids()`
- `supports_tool_choice()`

Also remove the default `stream_deltas()` bridge (no longer needed since all adapters implement it).

- [ ] **Step 2: Remove implementations from all adapters**

Remove `parse_response()` and `parse_stream()` from:
- `openai_chat.rs`
- `openai_responses.rs`
- `anthropic.rs`
- `gemini.rs`
- `configurable.rs`

- [ ] **Step 3: Clean up `responses/shared.rs`**

Remove:
- `parse_sse_body()`
- `build_sse_stream()`
- `extract_text()`
- `extract_tool_calls()`
- `parse_sse_data()` (if no longer used by `stream_deltas()`)

Keep:
- `convert_messages()`
- `build_tools()`
- `build_reasoning()`
- `map_tool_choice()`

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: Compiles. Any dangling references will surface as errors — fix them.

- [ ] **Step 5: Run all tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -5`
Expected: All tests pass. Some old tests in `shared.rs` that tested `parse_sse_body()` will need to be removed or converted to test `stream_deltas()` instead.

- [ ] **Step 6: Commit**

```bash
git add -A src/providers/
git commit -m "provider: remove parse_response(), parse_stream(), old capability flags"
```

---

## Task 11: LoopProvider + AgentLoop + Bridge Adaptation

**Files:**
- Modify: `src/agent_loop/loop_core.rs` — `LoopProvider::stream()`, Think step, `DeltaSink`
- Modify: `src/agent_loop/provider_bridge.rs` — implement `stream()`
- Modify: `src/agent_loop/factory.rs` — pass `DeltaSink`

**NOTE on constructor blast radius**: `AgentLoop::new()` is called in 12+ test sites plus `factory.rs` and `subagent_tool.rs`. The `delta_sink` field MUST NOT be a required constructor parameter. Instead, add it as a private field defaulting to `Box::new(NoopSink)` inside the existing `new()` body. Only `with_delta_sink()` builder exposes it. This way zero existing call sites need updating.

- [ ] **Step 1: Change `LoopProvider::call()` to `stream()`**

In `loop_core.rs` (line ~124):

```rust
#[async_trait]
pub trait LoopProvider: Send + Sync {
    async fn stream(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>>;
}
```

- [ ] **Step 2: Add `delta_sink` field to `AgentLoop`**

```rust
pub struct AgentLoop<P: LoopProvider> {
    // ... existing fields ...
    delta_sink: Box<dyn DeltaSink>,
}
```

Default to `NoopSink` in constructor. Add `with_delta_sink()` builder.

- [ ] **Step 3: Update Think step in `run_with_history_messages()`**

Replace (line ~358):
```rust
let response = self.provider.call(&messages, &system_prompt, &tool_defs).await?;
```

With:
```rust
use futures::StreamExt;
let delta_stream = self.provider.stream(&messages, &system_prompt, &tool_defs).await?;
let mut collector = crate::providers::DeltaCollector::new();
futures::pin_mut!(delta_stream);
while let Some(delta) = delta_stream.next().await {
    let delta = delta?;
    self.delta_sink.on_delta(&delta).await;
    collector.push(delta);
}
let response = collector.finish();
```

Everything after this line remains unchanged.

- [ ] **Step 4: Update `AiProviderBridge` to implement `stream()`**

In `provider_bridge.rs`:

```rust
#[async_trait]
impl LoopProvider for AiProviderBridge {
    async fn stream(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tools: &[LoopToolDefinition],
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
        let repaired = transform_messages(messages);
        let dispatcher_tools: Vec<_> = tools.iter().map(convert_tool_def).collect();
        let payload = RequestPayload::new(&repaired)
            .with_system(Some(system_prompt))
            .with_tools(Some(&dispatcher_tools));

        // Try real streaming via HttpProvider
        if let Some(http) = self.provider.as_http_provider() {
            return http.stream_raw(payload).await;
        }

        // Fallback: call process() and wrap
        let response = self.provider.process(payload).await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(response_to_delta_stream(response))
    }
}
```

- [ ] **Step 5: Update MockProvider in tests**

All mock providers in `loop_core.rs` tests need to implement `stream()` instead of `call()`. Use `response_to_delta_stream()`:

```rust
#[async_trait]
impl LoopProvider for MockProvider {
    async fn stream(&self, _messages: &[UnifiedMessage], _system_prompt: &str, _tools: &[ToolDefinition])
        -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>>
    {
        let response = self.next_response();
        Ok(response_to_delta_stream(response))
    }
}
```

Similarly for `CapturingMockProvider` and any other mock types.

- [ ] **Step 6: Update `integration_probe.rs`**

The `ProbeProvider` implements `AiProvider` (not `LoopProvider`), so it should be unaffected. But verify.

- [ ] **Step 7: Update `factory.rs`**

If `LoopFactory` constructs `AgentLoop`, ensure `NoopSink` is passed by default.

- [ ] **Step 8: Compile and run all tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -10`
Expected: All tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/agent_loop/
git commit -m "agent_loop: LoopProvider::stream(), Think step delta consumption, DeltaSink pre-wire"
```

---

## Task 12: Final Cleanup + Delete Dead Code

**Files:**
- Delete: `src/providers/codex/types.rs` (if not already deleted in Task 5)
- Delete: `src/providers/codex/mod.rs`
- Modify: `src/providers/mod.rs` — remove `pub mod codex` if types were only used there
- Clean up any remaining dead imports, unused functions

- [ ] **Step 1: Search for dead code**

Run: `cargo check -p alephcore 2>&1 | grep "warning.*unused\|warning.*dead_code" | head -20`
Address each warning.

- [ ] **Step 2: Remove codex submodule if still present**

Check if `src/providers/codex/` still has files used elsewhere (e.g., `codex/auth.rs` for OAuth, `codex/security.rs` for PoW). These should **NOT** be deleted — they're still needed for the Codex variant's auth flow. Only delete `codex/types.rs` (merged into `responses/types.rs`).

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -W clippy::all 2>&1 | head -30`
Fix any new warnings.

- [ ] **Step 4: Run full test suite**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -10`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add -A src/providers/
git commit -m "provider: final cleanup — remove dead code, fix warnings"
```

---

## Task 13: Integration Smoke Test

**Files:** No new files — run existing integration tests + manual verification.

- [ ] **Step 1: Run full test suite**

```bash
cargo test -p alephcore --lib
cargo test -p alephcore --lib -- loom  # if loom feature tests exist
```

- [ ] **Step 2: Compile release build**

```bash
cargo build --release --bin aleph-server 2>&1 | tail -5
```

- [ ] **Step 3: Quick check with `cargo check` on all packages**

```bash
cargo check --workspace 2>&1 | tail -10
```

- [ ] **Step 4: Commit final state if any fixes were needed**

```bash
git add -A && git commit -m "provider: integration fixes after protocol refactor"
```

---

## Summary

| Task | Description | Est. Complexity |
|------|-------------|----------------|
| 1 | ProviderDelta + DeltaCollector foundation | Medium |
| 2 | Extract openai_common module (incl. codex_utils migration) | Low |
| 3 | Upgrade Responses API + Anthropic types | Low |
| 4 | ProtocolAdapter trait migration (add stream_deltas with bridge) | Medium |
| 5 | OpenAI Responses stream_deltas() + Codex merge | High |
| 6 | Rename openai.rs + Chat stream_deltas() | Medium |
| 7 | Anthropic full upgrade + stream_deltas() | High |
| 8 | Gemini + Configurable migration | Medium |
| 9 | **HttpProvider stream adaptation** (MUST precede Task 10) | Medium |
| 10 | Remove old ProtocolAdapter methods | Medium |
| 11 | LoopProvider + AgentLoop + Bridge | High |
| 12 | Final cleanup | Low |
| 13 | Integration smoke test | Low |

### Task Dependencies

```
1 → 2 → 3 → 4 → [5,6,7,8] → 9 → 10 → 11 → 12 → 13
```

Tasks 5-8 can be done in parallel (each adapter is independent) but all must complete before Task 9.
Task 9 (HttpProvider) MUST complete before Task 10 (remove old methods) — HttpProvider is the last consumer of `parse_response()`.
