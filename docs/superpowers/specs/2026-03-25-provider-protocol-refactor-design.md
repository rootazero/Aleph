# Provider Protocol Refactor — Stream-First Architecture

**Date**: 2026-03-25
**Status**: Approved
**Scope**: OpenAI (Chat + Responses) + Anthropic protocol adapters, AgentLoop streaming integration

## Summary

Refactor the provider protocol layer to a stream-first architecture with fine-grained `ProviderDelta` events, upgrade OpenAI Responses API to full feature parity, upgrade Anthropic to latest beta features, consolidate Codex into Responses adapter, and clean up dead code.

**Phase 1** (this spec): Provider protocol upgrade + AgentLoop stream consumption
**Phase 2** (future): ReplyEmitter + Gateway + frontend end-to-end streaming

## Goals

1. **ProviderDelta as universal output** — all protocol adapters emit `Stream<ProviderDelta>` instead of `ProviderResponse`
2. **OpenAI Responses API full support** — `previous_response_id`, `context_management`, `store`, `text.format` structured output
3. **Anthropic protocol upgrade** — prompt caching, interleaved thinking, fine-grained tool streaming, service tier, complete SSE parsing
4. **Code consolidation** — merge Codex into Responses adapter, extract `openai_common` module, eliminate duplicate logic
5. **Phase 2 ready** — `DeltaSink` interface pre-wired for end-to-end streaming

## Non-Goals

- Gemini protocol upgrade (separate task)
- End-to-end streaming to frontend (Phase 2)
- New provider support

---

## Section 1: ProviderDelta Data Model

### ProviderDelta Enum

```rust
/// Fine-grained streaming event from any AI provider.
///
/// Each variant maps to specific SSE event types across protocols:
/// - OpenAI Responses: response.output_text.delta, response.function_call_arguments.delta, etc.
/// - Anthropic: content_block_delta (text/thinking/tool_use), message_delta, etc.
/// - OpenAI Chat: choices[0].delta.content, choices[0].delta.tool_calls, etc.
#[derive(Debug, Clone)]
pub enum ProviderDelta {
    /// Incremental text output
    TextDelta(String),

    /// Incremental thinking/reasoning output
    ThinkingDelta(String),

    /// A new tool call started (provides id and name)
    ToolCallStart {
        /// Provider-assigned call ID (e.g. "call_abc", "toolu_123")
        id: String,
        /// Tool name (already desanitized to Aleph internal name)
        name: String,
    },

    /// Incremental tool call arguments (JSON fragment)
    ToolCallArgDelta {
        /// Matches the id from ToolCallStart
        id: String,
        /// JSON argument fragment
        delta: String,
    },

    /// Tool call arguments complete
    ToolCallEnd {
        /// Matches the id from ToolCallStart
        id: String,
    },

    /// Token usage report (typically arrives with final event)
    Usage(TokenUsage),

    /// Stream completed successfully
    Done(StopReason),

    /// Provider-level error during streaming
    Error(String),
}
```

### Design Decisions

- **Three-phase tool calls** (`Start` → `ArgDelta` → `End`) — enables Phase 2 real-time tool argument rendering
- **`Usage` as independent event** — some providers (Anthropic) emit usage before `Done`
- **`Error` as event, not `Result::Err`** — allows partial recovery; upper layer decides whether to abort

### DeltaCollector

Aggregates `ProviderDelta` events back into `ProviderResponse` for backward compatibility with AgentLoop's think→act cycle:

```rust
pub struct DeltaCollector {
    text: String,
    thinking: String,
    tool_calls: HashMap<String, (String, String)>, // id -> (name, accumulated_args)
    usage: Option<TokenUsage>,
    stop_reason: StopReason,
}

impl DeltaCollector {
    pub fn new() -> Self { ... }
    pub fn push(&mut self, delta: ProviderDelta) { ... }
    pub fn finish(self) -> ProviderResponse { ... }
}
```

---

## Section 2: ProtocolAdapter Interface Changes

### New Interface

```rust
#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    /// Build the HTTP request
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder>;

    /// Parse HTTP response into a stream of ProviderDelta events
    ///
    /// Single output path — replaces both parse_response() and parse_stream().
    async fn stream_deltas(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<ProviderDelta>>>;

    /// Protocol name for logging/debugging
    fn name(&self) -> &'static str;

    /// Capability flags
    fn supports_native_tools(&self) -> bool { false }
    fn supports_thinking(&self) -> bool { false }
    fn supports_strict_schema(&self) -> bool { false }
}
```

### Removed

- `parse_response()` — replaced by `stream_deltas()` + `DeltaCollector`
- `parse_stream()` — replaced by `stream_deltas()` (returns `ProviderDelta` not `String`)
- `build_request()` `is_streaming` parameter — always streaming

### HttpProvider Adaptation

```rust
impl AiProvider for HttpProvider {
    async fn process(&self, payload: RequestPayload<'_>) -> Result<ProviderResponse> {
        let request = self.adapter.build_request(&payload, &self.config)?;
        let response = request.send().await?;
        let stream = self.adapter.stream_deltas(response).await?;

        let mut collector = DeltaCollector::new();
        pin_mut!(stream);
        while let Some(delta) = stream.next().await {
            collector.push(delta?);
        }
        Ok(collector.finish())
    }
}
```

`AiProvider::process()` signature unchanged — external callers unaffected.

---

## Section 3: OpenAI Protocol Reorganization

### File Structure (After)

```
providers/
  protocols/
    openai_common/
      mod.rs          — public re-exports
      messages.rs     — shared message utilities (not conversion — formats differ too much)
      tools.rs        — ToolDefinition formatting, sanitize/desanitize
      sse.rs          — SSE line buffering infrastructure, IndexIdTracker
    openai_chat.rs    — Chat Completions adapter (/v1/chat/completions)
    openai_responses.rs — Responses API adapter (/v1/responses), includes Codex variant
    anthropic.rs
    gemini.rs
  responses/
    types.rs          — Responses API types (merged with codex/types)
```

### Codex Merged into Responses via ResponsesVariant

```rust
#[derive(Debug, Clone, Default)]
pub struct ResponsesVariant {
    /// Override endpoint path (None = /v1/responses)
    pub endpoint_path: Option<String>,
    /// Extra headers (OAuth, beta flags, etc.)
    pub extra_headers: Vec<(String, String)>,
    /// Force store value (Codex: false for ZDR compliance)
    pub store: Option<bool>,
    /// Codex text config (verbosity)
    pub text: Option<Value>,
    /// Include fields (e.g. reasoning.encrypted_content)
    pub include: Option<Vec<String>>,
}

impl ResponsesVariant {
    pub fn codex() -> Self {
        Self {
            endpoint_path: Some("/backend-api/codex/responses".into()),
            store: Some(false),
            text: Some(json!({"verbosity": "medium"})),
            include: Some(vec!["reasoning.encrypted_content".into()]),
            ..Default::default()
        }
    }
}
```

Codex OAuth headers (`chatgpt-account-id` from JWT) injected in `build_request()` based on variant.

### Responses API New Features

New fields in `ResponsesRequest`:

```rust
pub struct ResponsesRequest {
    // ... existing fields ...

    /// Reference previous response for context chaining
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,

    /// Server-side context management
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_management: Option<ContextManagement>,

    /// Structured output format / Codex text config
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextConfig>,
}

#[derive(Debug, Serialize)]
pub struct ContextManagement {
    #[serde(rename = "type")]
    pub mgmt_type: String, // "compaction"
}

#[derive(Debug, Serialize)]
pub struct TextConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<TextFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum TextFormat {
    #[serde(rename = "json_schema")]
    JsonSchema { name: String, schema: Value },
    #[serde(rename = "json_object")]
    JsonObject,
}
```

### Auto-Enable Server Optimizations for Official OpenAI Endpoints

In `openai_responses.rs` `build_request()`:

```rust
if Self::is_openai_official(&config.base_url) {
    if request.store.is_none() {
        request.store = Some(true);
    }
    request.context_management = Some(ContextManagement {
        mgmt_type: "compaction".into(),
    });
}
```

`previous_response_id` set by AgentLoop when session tracking is enabled (optional, user-configurable).

### Protocol Registry (unchanged names)

```rust
registry.register("openai", OpenAiChatProtocol::new(client));
registry.register("openai-responses", OpenAiResponsesProtocol::new(client, ResponsesVariant::default()));
registry.register("codex", OpenAiResponsesProtocol::new(client, ResponsesVariant::codex()));
registry.register("anthropic", AnthropicProtocol::new(client));
registry.register("gemini", GeminiProtocol::new(client));
```

---

## Section 4: Anthropic Protocol Upgrade

### 4.1 Beta Headers

```rust
fn build_beta_headers(model: &str) -> String {
    let mut betas = vec![
        "interleaved-thinking-2025-05-14",
        "fine-grained-tool-streaming-2025-05-14",
    ];
    if Self::is_large_context_model(model) {
        betas.push("output-128k-2025-02-19");
    }
    betas.join(",")
}
```

Added to `build_request()` as `anthropic-beta` header.

### 4.2 Prompt Caching

System prompt's last block gets `cache_control: { type: "ephemeral" }`:

```rust
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub control_type: String, // "ephemeral"
}
```

Aleph already parses `cache_read_input_tokens` in `TokenUsage`, so cache hit stats work automatically.

### 4.3 Service Tier

New optional field in `MessagesRequest`:

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub service_tier: Option<String>,  // "auto" | "standard_only"
```

### 4.4 Complete SSE Parsing via stream_deltas()

Full event handling replaces the current text-only `parse_sse_line()`:

| Anthropic SSE Event | ProviderDelta |
|---------------------|---------------|
| `content_block_start` (type=text) | — (no delta yet) |
| `content_block_start` (type=tool_use) | `ToolCallStart { id, name }` |
| `content_block_start` (type=thinking) | — (no delta yet) |
| `content_block_delta` (type=text_delta) | `TextDelta(text)` |
| `content_block_delta` (type=thinking_delta) | `ThinkingDelta(thinking)` |
| `content_block_delta` (type=input_json_delta) | `ToolCallArgDelta { id, delta }` |
| `content_block_stop` (tool_use block) | `ToolCallEnd { id }` |
| `message_delta` | `Usage(...)` + `Done(stop_reason)` |
| `error` | `Error(message)` |

### 4.5 Stream State Tracking

Anthropic uses block `index` in deltas, not `id`. Track mapping:

```rust
struct AnthropicStreamState {
    block_ids: HashMap<u32, String>, // content_block index -> tool_use id
}
```

Populated at `content_block_start`, looked up at `content_block_delta`.

---

## Section 5: LoopProvider & AgentLoop Adaptation

### LoopProvider Interface

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

Replaces `call()` which returned `ProviderResponse`.

### DeltaSink (Phase 2 Pre-wire)

```rust
#[async_trait]
pub trait DeltaSink: Send + Sync {
    async fn on_delta(&self, delta: &ProviderDelta);
}

/// Phase 1: no-op
pub struct NoopSink;
```

### AgentLoop Think Step

```rust
// BEFORE
let response = self.provider.call(&messages, &system_prompt, &tools).await?;

// AFTER
let delta_stream = self.provider.stream(&messages, &system_prompt, &tools).await?;
let mut collector = DeltaCollector::new();
pin_mut!(delta_stream);
while let Some(delta) = delta_stream.next().await {
    let delta = delta?;
    self.delta_sink.on_delta(&delta).await;
    collector.push(delta);
}
let response = collector.finish();
```

Act step (tool execution, safety check, result collection) **completely unchanged**.

### AgentLoop Constructor

```rust
pub struct AgentLoop {
    // ... existing fields ...
    delta_sink: Box<dyn DeltaSink>,  // NEW
}

impl AgentLoop {
    pub fn new(...) -> Self {
        Self { ..., delta_sink: Box::new(NoopSink) }
    }
    pub fn with_delta_sink(mut self, sink: Box<dyn DeltaSink>) -> Self {
        self.delta_sink = sink;
        self
    }
}
```

### Test Helper

```rust
/// Convert a ProviderResponse into a one-shot delta stream (for MockProvider)
pub fn response_to_delta_stream(response: ProviderResponse) -> BoxStream<'static, anyhow::Result<ProviderDelta>> {
    let mut deltas = Vec::new();
    if let Some(thinking) = response.thinking {
        deltas.push(ProviderDelta::ThinkingDelta(thinking));
    }
    if let Some(text) = response.text {
        deltas.push(ProviderDelta::TextDelta(text));
    }
    for tc in response.tool_calls {
        deltas.push(ProviderDelta::ToolCallStart { id: tc.id.clone(), name: tc.name.clone() });
        deltas.push(ProviderDelta::ToolCallArgDelta {
            id: tc.id.clone(),
            delta: serde_json::to_string(&tc.arguments).unwrap(),
        });
        deltas.push(ProviderDelta::ToolCallEnd { id: tc.id });
    }
    if let Some(usage) = response.usage {
        deltas.push(ProviderDelta::Usage(usage));
    }
    deltas.push(ProviderDelta::Done(response.stop_reason));
    futures::stream::iter(deltas.into_iter().map(Ok)).boxed()
}
```

---

## Section 6: OpenAI Chat Completions stream_deltas()

Chat Completions SSE uses `choices[0].delta` with index-based tool call tracking (same pattern as Anthropic).

### SSE Event Mapping

| Chat SSE Field | ProviderDelta |
|----------------|---------------|
| `delta.content` | `TextDelta(text)` |
| `delta.tool_calls[i]` with `id` + `function.name` | `ToolCallStart { id, name }` |
| `delta.tool_calls[i]` with `function.arguments` | `ToolCallArgDelta { id, delta }` |
| `finish_reason: "stop"` | `Done(EndTurn)` |
| `finish_reason: "tool_calls"` | `Done(ToolUse)` |
| `finish_reason: "length"` | `Done(MaxTokens)` |
| `usage` (final chunk) | `Usage(...)` |

### IndexIdTracker (shared utility)

Both Chat Completions and Anthropic need `index → id` mapping. Extracted to `openai_common::sse`:

```rust
pub struct IndexIdTracker {
    map: HashMap<u64, String>,
}
impl IndexIdTracker {
    pub fn track(&mut self, index: u64, id: String) { ... }
    pub fn get(&self, index: u64) -> Option<&str> { ... }
}
```

### Message Conversion

Chat and Responses message formats differ significantly (`messages[]` vs `input[]` / `InputItem`). Message conversion stays in each adapter — not extracted to `openai_common`.

---

## Section 7: Code Cleanup

### Files Deleted

| File | Reason |
|------|--------|
| `protocols/codex.rs` | Merged into `openai_responses.rs` via `ResponsesVariant` |
| `providers/codex/types.rs` | Merged into `responses/types.rs` |
| `providers/codex/mod.rs` | Entire codex submodule removed |

### Methods Removed

| Method | Reason |
|--------|--------|
| `ProtocolAdapter::parse_response()` | Replaced by `stream_deltas()` + `DeltaCollector` |
| `ProtocolAdapter::parse_stream()` | Replaced by `stream_deltas()` |
| `LoopProvider::call()` | Replaced by `LoopProvider::stream()` |
| `shared::parse_sse_body()` | No longer need one-shot SSE body parsing |
| `shared::build_sse_stream()` | Replaced by `stream_deltas()` |
| `shared::extract_text()` / `extract_tool_calls()` | Delta accumulation replaces post-hoc extraction |

### Files Renamed

| Old | New | Reason |
|-----|-----|--------|
| `protocols/openai.rs` | `protocols/openai_chat.rs` | Symmetric with `openai_responses.rs` |

### Files Refactored

| File | Changes |
|------|---------|
| `responses/shared.rs` | Slimmed: keep `convert_messages()`, `build_tools()`, `build_reasoning()`, `map_tool_choice()`; remove all SSE parsing |
| `responses/types.rs` | Merge codex types; add `previous_response_id`, `context_management`, `TextConfig`, `TextFormat` |
| `providers/adapter.rs` | Simplify `ProtocolAdapter` trait; add `ProviderDelta`, `DeltaCollector` |
| `providers/http_provider.rs` | `process()` uses stream + collect |
| `agent_loop/loop_core.rs` | `LoopProvider::stream()`; Think step consumes deltas; add `DeltaSink` |

### Backward Compatibility

- **Provider presets**: all 28+ presets unchanged (protocol field names preserved)
- **User config**: existing `protocol: "openai" / "codex" / "anthropic"` continues working
- **AiProvider trait**: `process()` signature unchanged
- **AgentLoop external behavior**: `LoopRunResult` unchanged
- **Protocol registry**: `"codex"` name preserved, maps to `OpenAiResponsesProtocol + ResponsesVariant::codex()`

---

## Section 8: Review Fixes — Critical & Important Issues

Addressed from spec review feedback.

### C1. ConfigurableProtocol and GeminiProtocol Migration

Both implement `ProtocolAdapter` and must be updated for the new trait signature.

**GeminiProtocol**: Straightforward — Gemini already has SSE-like streaming. Implement `stream_deltas()` with the same pattern as other adapters, mapping Gemini's `generateContent` stream events to `ProviderDelta`.

**ConfigurableProtocol**: Two modes require different handling:
- **Minimal mode** (delegates to base adapter): Trivially delegates `stream_deltas()` to `self.base.stream_deltas(response)`
- **Custom mode** (template-rendered endpoints + JSONPath parsing): Cannot produce fine-grained deltas from arbitrary API formats. Solution: keep a `parse_custom_response()` internal method that produces a `ProviderResponse`, then wrap it via `response_to_delta_stream()` (the same test helper from Section 5). This is a one-shot "fake stream" — acceptable because custom protocols are inherently non-streaming.

### C2. HttpProvider PII/Leak/Error Handling Preservation

The Section 2 `HttpProvider` code was a simplified illustration. The actual implementation preserves all existing safety logic:

```rust
impl AiProvider for HttpProvider {
    async fn process(&self, payload: RequestPayload<'_>) -> Result<ProviderResponse> {
        // 1. PII filtering on outbound messages (PRESERVED)
        let filtered_payload = self.filter_pii(payload)?;

        // 2. Secret leak detection on outbound content (PRESERVED)
        self.check_outbound_leaks(&filtered_payload)?;

        // 3. Build request via adapter
        let request = self.adapter.build_request(&filtered_payload, &self.config)?;

        // 4. Send with timeout/network error mapping (PRESERVED)
        let response = self.send_with_error_handling(request).await?;

        // 5. Stream deltas and collect
        let stream = self.adapter.stream_deltas(response).await?;
        let mut collector = DeltaCollector::new();
        pin_mut!(stream);
        while let Some(delta) = stream.next().await {
            collector.push(delta?);
        }
        let provider_response = collector.finish();

        // 6. Secret leak detection on inbound response (PRESERVED)
        self.check_inbound_leaks(&provider_response)?;

        // 7. Response validation (PRESERVED)
        provider_response.validate()?;

        Ok(provider_response)
    }
}
```

### C3. AiProvider → LoopProvider Bridge

The bridge struct that wraps `AiProvider` into `LoopProvider` needs to produce a `Stream<ProviderDelta>`. Two options:

**Option chosen**: Add `fn stream_raw()` to `HttpProvider` (not to `AiProvider` trait) that exposes the raw delta stream WITH PII/leak filtering on outbound only (inbound leak check deferred to caller). The bridge calls `stream_raw()` directly:

```rust
impl LoopProvider for AiProviderBridge {
    async fn stream(&self, messages: &[UnifiedMessage], system_prompt: &str, tools: &[ToolDefinition])
        -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>>
    {
        // For HttpProvider: use stream_raw() for real streaming
        // For other AiProvider impls: fallback to process() + response_to_delta_stream()
        if let Some(http) = self.provider.as_http_provider() {
            let payload = build_payload(messages, system_prompt, tools);
            return http.stream_raw(payload).await;
        }

        // Fallback: call process() and wrap
        let response = self.provider.process(payload).await?;
        Ok(response_to_delta_stream(response))
    }
}
```

This keeps `AiProvider::process()` unchanged (backward compatible) while enabling real streaming through the hot path.

### I1. Removed Capability Flags

Added to Section 7 "Methods Removed" table:

| Method | Reason |
|--------|--------|
| `supports_parallel_tools()` | Only Gemini overrides to false; handled internally in Gemini adapter |
| `returns_tool_call_ids()` | Only Gemini overrides to false; handled internally in Gemini adapter |
| `supports_tool_choice()` | Default true everywhere; remove dead abstraction |

### I2. `is_streaming` Parameter for ConfigurableProtocol

`ConfigurableProtocol` custom mode uses `is_streaming` to choose between `endpoints.chat` and `endpoints.stream`. Since custom-mode protocols go through the `parse_custom_response()` fallback path (Section C1 above), they always use the non-streaming endpoint. The `build_request()` signature change is safe.

If a custom protocol YAML defines a `stream` endpoint, it will be ignored in this refactor. This is acceptable — custom protocols are rare and can be updated later.

### I3. ToolCallEnd Only for Tool-Use Blocks

Clarification for Anthropic `stream_deltas()`: `content_block_stop` only emits `ToolCallEnd` when `block_ids.contains_key(index)`. Text and thinking blocks' `content_block_stop` events are silently consumed with no delta emitted.

### I4. DeltaCollector JSON Parse Error Handling

`DeltaCollector::finish()` uses `serde_json::from_str` with fallback:

```rust
fn finish_tool_calls(&self) -> Vec<NativeToolCall> {
    self.tool_calls.iter().map(|(id, (name, args_str))| {
        let arguments = serde_json::from_str(args_str)
            .unwrap_or_else(|e| {
                tracing::warn!(tool_id = %id, error = %e, "Malformed tool call arguments, using raw string");
                Value::String(args_str.clone())
            });
        NativeToolCall { id: id.clone(), name: name.clone(), arguments }
    }).collect()
}
```

### I5. ProviderDelta::Error vs Result::Err Contract

Clear distinction:

- **`Result::Err`** in the stream: Infrastructure failures — HTTP disconnect, invalid SSE framing, UTF-8 decode error. These are unrecoverable; the stream is broken.
- **`ProviderDelta::Error(msg)`**: Provider-level semantic errors — Anthropic `error` SSE event, OpenAI `response.failed` event within an otherwise valid stream. The stream may continue (e.g., error followed by retry), or the consumer may choose to abort.

Updated `ProviderDelta::Error` doc comment accordingly.

### S1. IndexIdTracker Location

Moved from `openai_common::sse` to `providers/adapter.rs` alongside `ProviderDelta`. This is a protocol-neutral utility — both Anthropic and OpenAI Chat adapters depend on `adapter.rs` already, not on each other.

### S3. Auto-Enable store/context_management Configurability

Changed from URL-based auto-detection to `ProviderConfig`-driven:

```rust
// In ProviderConfig (or provider-level settings)
pub enable_server_context: Option<bool>,  // None = auto (true for official OpenAI)
```

`is_openai_official()` only applies when `enable_server_context` is `None` (default). Users can explicitly set `false` to opt out.
