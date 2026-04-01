# Provider Routing Refactor — Unified Message Architecture

**Date:** 2026-03-16
**Status:** Approved
**Scope:** `src/providers/`, `src/agent_loop/`, 46 caller files

## Problem

Aleph's current provider routing layer serializes structured conversation history (`LoopMessage[]`) into XML-tagged text via `AiProviderBridge::format_messages()`. This flat string is passed as `RequestPayload.input: &str` to protocol adapters, which treat it as a single user message.

**Consequences:**
- **Anthropic/OpenAI protocols broken for multi-turn**: They wrap the entire XML blob in one `role: "user"` message. The LLM cannot distinguish prior tool calls from new user input, causing premature `EndTurn` or incoherent responses.
- **ChatGPT protocol works by accident**: It parses the XML back into `InputItem` structs, but this round-trip is fragile and lossy.
- **Token counting broken**: `tokens=0` always — ChatGPT protocol's `parse_response` never extracts `usage`.
- **Stop reason incomplete**: `StopReason::MaxTokens` never returned by ChatGPT protocol, causing `hit_limit` misclassification.
- **Assistant turn missing from history**: The loop pushes `ToolUse` messages without an `Assistant` turn wrapper, violating API contracts.

**Root cause:** The abstraction boundary is wrong. `RequestPayload` carries a flat string instead of structured messages. Protocol adapters have no access to conversation structure.

## Reference Architecture

pi-mono/openclaw (TypeScript) solves this with:
1. **Unified `Message` type** — `UserMessage | AssistantMessage | ToolResultMessage` with rich content blocks
2. **`transformMessages()` pre-processing** — Cross-model normalization, orphaned tool call repair
3. **Per-provider `convertMessages()`** — Each provider converts unified messages to its native API format
4. **Agent loop works with unified types only** — Never touches provider specifics

## Design Decisions

| # | Decision | Choice |
|---|----------|--------|
| 1 | Message modeling style | pi-mono style: `AssistantMessage { content: Vec<ContentBlock> }` |
| 2 | Type location | `src/providers/message.rs` |
| 3 | AiProvider migration strategy | One-step replacement — all 46 callers updated at once |
| 4 | ProtocolAdapter change | `RequestPayload.input` → `RequestPayload.messages`, trait signature unchanged |
| 5 | Protocol adapter priority | All 5 adapters implemented simultaneously |
| 6 | Usage/StopReason tracking | Each adapter extracts + centralized validation logging |
| 7 | transform_messages layer | Yes — orphaned tool call repair + cross-model normalization stub |
| 8 | Single-turn caller migration | `UnifiedMessage::user("text")` convenience constructor |

## Unified Message Types

**File: `src/providers/message.rs`**

```rust
/// Unified message type — the single data model for all provider interactions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UnifiedMessage {
    /// User message
    User {
        content: Vec<ContentBlock>,
    },
    /// Assistant message (one turn may contain multiple content blocks)
    Assistant {
        content: Vec<ContentBlock>,
    },
    /// Tool execution result
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        content: Vec<ContentBlock>,
        is_error: bool,
    },
}

/// Content block — one atomic unit within a message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ContentBlock {
    /// Plain text
    Text { text: String },
    /// Structured JSON (preserves tool output structure)
    Json { value: Value },
    /// Thinking/reasoning trace (extended thinking)
    Thinking { thinking: String },
    /// Tool call (only in Assistant messages)
    ToolCall { id: String, name: String, arguments: Value },
    /// Image
    Image { data: String, mime_type: String },
}
```

**Convenience constructors:**

```rust
impl UnifiedMessage {
    pub fn user(text: impl Into<String>) -> Self;
    pub fn assistant(text: impl Into<String>) -> Self;
    pub fn tool_result(call_id: impl Into<String>, name: impl Into<String>, output: impl Into<String>, is_error: bool) -> Self;
}
```

## RequestPayload Refactor

**File: `src/providers/adapter.rs`**

```rust
pub struct RequestPayload<'a> {
    pub messages: &'a [UnifiedMessage],     // replaces input: &str
    pub system_prompt: Option<&'a str>,
    pub tools: Option<&'a [ToolDefinition]>,
    pub think_level: Option<ThinkLevel>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}
```

**Removed fields:**
- `input: &str` → replaced by `messages`
- `image: Option<&ImageData>` → migrated to `ContentBlock::Image`
- `attachments: Option<&[MediaAttachment]>` → migrated to `ContentBlock`
- `force_standard_mode: bool` → each adapter decides internally

`ProtocolAdapter` trait method signatures are syntactically unchanged, but semantically different: `build_request` now receives a `RequestPayload` whose `messages` field replaces the old `input` field. Every adapter's `build_request` implementation must change its internal logic to read structured messages instead of a flat string.

## AiProvider Trait Simplification

**File: `src/providers/mod.rs`**

7 request methods → 1:

```rust
pub trait AiProvider: Send + Sync {
    fn process(
        &self,
        payload: RequestPayload<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + '_>>;

    fn name(&self) -> &str;
    fn color(&self) -> &str;
    fn supports_native_tools(&self) -> bool { false }
    fn supports_thinking(&self) -> bool { false }
}
```

**Removed:** `process(&str, ...)`, `process_with_image`, `process_with_attachments`, `process_with_thinking`, `process_with_overrides`, `process_with_payload`.

**Caller migration patterns:**

```rust
// Pattern 1: Text-only (most common, ~40 callers)
// Before:
let text = provider.process("input", Some("system")).await?;
// After:
let msgs = [UnifiedMessage::user("input")];
let resp = provider.process(
    RequestPayload { messages: &msgs, system_prompt: Some("system"), ..Default::default() }
).await?;
let text = resp.text_content();

// Pattern 2: With image/attachments (multimodal callers)
// Before:
let text = provider.process_with_image("describe this", Some(&image), Some("system")).await?;
// After:
let msgs = [UnifiedMessage::User {
    content: vec![
        ContentBlock::Text { text: "describe this".into() },
        ContentBlock::Image { data: image.base64, mime_type: image.mime_type },
    ],
}];
let resp = provider.process(
    RequestPayload { messages: &msgs, system_prompt: Some("system"), ..Default::default() }
).await?;
let text = resp.text_content();

// Pattern 3: With thinking/temperature/max_tokens overrides
// Before:
let text = provider.process_with_overrides("input", system, ThinkLevel::High, Some(0.7), Some(4096)).await?;
// After:
let msgs = [UnifiedMessage::user("input")];
let resp = provider.process(
    RequestPayload {
        messages: &msgs, system_prompt: system,
        think_level: Some(ThinkLevel::High), temperature: Some(0.7), max_tokens: Some(4096),
        ..Default::default()
    }
).await?;
let text = resp.text_content();
```

## Per-Protocol convert_messages

### ChatGPT (Codex Responses API)

| UnifiedMessage | Codex InputItem |
|---|---|
| `User { Text }` | `Message { role: "user", content }` |
| `Assistant { Text + ToolCall[] }` | `Message { role: "assistant", content }` + `FunctionCall { call_id, name, arguments }` per ToolCall |
| `ToolResult { ... }` | `FunctionCallOutput { call_id, output }` |

Also: extract `usage` from `Completed` event, map `response.status == "incomplete"` → `StopReason::MaxTokens`.

### Anthropic (Messages API)

| UnifiedMessage | Anthropic format |
|---|---|
| `User { Text }` | `{ role: "user", content: [{ type: "text", text }] }` |
| `Assistant { Text + Thinking + ToolCall[] }` | `{ role: "assistant", content: [text_block, thinking_block, tool_use_block] }` |
| `ToolResult { ... }` (consecutive) | Merge into single `{ role: "user", content: [{ type: "tool_result", tool_use_id, content }...] }` |

Anthropic-specific: tool_use_id sanitization `[^a-zA-Z0-9_-]` → `_`, max 64 chars.

### OpenAI (Chat Completions API)

| UnifiedMessage | OpenAI format |
|---|---|
| `User { Text }` | `{ role: "user", content: "text" }` |
| `Assistant { Text + ToolCall[] }` | `{ role: "assistant", content: "text", tool_calls: [{ id, type: "function", function: { name, arguments: JSON } }] }` |
| `ToolResult { ... }` | `{ role: "tool", content: "output", tool_call_id: "id" }` (one per result) |

OpenAI-specific: assistant `content` must be plain string (not array), `arguments` must be `JSON.stringify`.

### Gemini (Generative AI API)

| UnifiedMessage | Gemini format |
|---|---|
| `User { Text }` | `{ role: "user", parts: [{ text }] }` |
| `Assistant { Text + ToolCall[] }` | `{ role: "model", parts: [{ text }, { functionCall: { name, args } }] }` |
| `ToolResult { ... }` (consecutive) | Merge into `{ role: "user", parts: [{ functionResponse: { name, response } }...] }` |

Gemini-specific: strict user/model alternation enforced.

### Configurable (Template Protocol)

Text-only fallback: extracts last User message text as input. Multi-turn history ignored by design.

## transform_messages Pre-processing

**File: `src/providers/message.rs`**

```rust
pub fn transform_messages(messages: &[UnifiedMessage], target_provider: Option<&str>) -> Vec<UnifiedMessage> {
    let mut result = messages.to_vec();
    repair_orphaned_tool_calls(&mut result);
    normalize_cross_model(&mut result, target_provider);
    result
}
```

**repair_orphaned_tool_calls:** Scans for Assistant ToolCall blocks without matching ToolResult. Inserts synthetic error ToolResult:
```rust
UnifiedMessage::tool_result(orphaned_id, orphaned_name, "No result provided — tool call was interrupted", true)
```

**normalize_cross_model:** Currently no-op. Reserved for thinking signature downgrade when switching providers mid-conversation.

**Call site:** `AiProviderBridge::call()`, before passing messages to `provider.process()`. This ensures all providers (including `OllamaProvider` which bypasses `HttpProvider`) benefit from the pre-processing. `HttpProvider` does NOT call `transform_messages` — it is the bridge's responsibility.

## ProviderResponse Validation

**File: `src/providers/adapter.rs`**

```rust
impl ProviderResponse {
    pub fn validate(&self, protocol_name: &str) {
        if self.usage.is_none() {
            tracing::warn!(protocol = protocol_name, "Provider response missing usage data");
        }
        if self.stop_reason == StopReason::Unknown {
            tracing::warn!(protocol = protocol_name, "Provider response has Unknown stop_reason");
        }
    }

    /// Convenience: extract text content
    pub fn text_content(&self) -> String {
        self.text.clone().unwrap_or_default()
    }
}
```

Called after every `parse_response()` in `HttpProvider::execute()`.

## PII Filtering Migration

Current `HttpProvider::execute()` runs PII filtering on `payload.input` (a `&str`) and leak detection via `LeakDetector::scan_outbound(payload.input)`. After the refactor, these must iterate over all `ContentBlock::Text` blocks across all messages.

**PII filtering** uses per-block iteration (not extract-all), because it mutates text:

```rust
// In http_provider.rs execute():
// Iterate all messages, filter each Text block individually
fn filter_pii_in_messages(messages: &mut [UnifiedMessage], engine: &PiiEngine) {
    for msg in messages.iter_mut() {
        for block in msg.content_blocks_mut() {
            if let ContentBlock::Text { ref mut text } = block {
                let result = engine.filter(text);
                if result.has_detections() {
                    *text = result.text;
                }
            }
        }
    }
}
```

**Leak detection** uses read-only `extract_all_text()` for scanning (concatenates all Text blocks):

```rust
impl UnifiedMessage {
    pub fn extract_all_text(messages: &[UnifiedMessage]) -> String {
        // Concatenate all Text blocks from all messages
    }
}
```

This preserves message structure during PII mutation while keeping leak detection simple.

## Streaming Scope

Streaming (`parse_stream`) is **deferred** from this refactor. Current streaming only yields text deltas (`BoxStream<Result<String>>`). The non-streaming `parse_response` path handles tool calls. The agent loop uses non-streaming mode.

Future work: streaming should yield structured events (text delta, tool call start/delta/end) like pi-mono's `AssistantMessageEventStream`. This requires a new `StreamEvent` enum and changes to `parse_stream` return type. Not in scope for this refactor.

## Agent Loop Changes

**`LoopMessage` eliminated** — `AgentLoop` uses `Vec<UnifiedMessage>` directly.

**`LoopProvider` trait updated:**
```rust
pub trait LoopProvider: Send + Sync {
    async fn call(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<ProviderResponse>;
}
```

**`AiProviderBridge` simplified** — No XML serialization. Constructs `RequestPayload { messages, system_prompt, tools }` and delegates to `provider.process()`.

**Key fix:** Assistant response creates one `UnifiedMessage::Assistant` with all content blocks (text + tool calls), not separate messages. This fixes the missing assistant turn bug.

**ProviderResponse → UnifiedMessage::Assistant conversion:**

```rust
impl UnifiedMessage {
    pub fn from_provider_response(resp: &ProviderResponse) -> Self {
        let mut content = Vec::new();
        if let Some(ref thinking) = resp.thinking {
            content.push(ContentBlock::Thinking { thinking: thinking.clone() });
        }
        if let Some(ref text) = resp.text {
            content.push(ContentBlock::Text { text: text.clone() });
        }
        for tc in &resp.tool_calls {
            content.push(ContentBlock::ToolCall {
                id: tc.id.clone(), name: tc.name.clone(), arguments: tc.arguments.clone(),
            });
        }
        UnifiedMessage::Assistant { content }
    }
}
```

**Tool name threading:** Current `LoopMessage::ToolResult` has no `tool_name` field. The new `UnifiedMessage::ToolResult` requires it (needed for Gemini `functionResponse`). The tool name is available from `tc.name` in the loop's `for tc in &response.tool_calls` iteration — thread it through to the ToolResult construction.

## Complete Data Flow (After Refactor)

```
User: "创建贪吃蛇"
  ↓
AgentLoop.messages = [UnifiedMessage::user("创建贪吃蛇")]
  ↓
AiProviderBridge::call(messages, system_prompt, tools)
  → transform_messages()     // repair orphaned tool calls
  → RequestPayload { messages, system_prompt, tools }
  ↓
HttpProvider::execute(payload)
  → PII filtering            // on text content blocks via extract_all_text()
  → leak detection           // on text content blocks
  → adapter.build_request()  // protocol-specific convert_messages
  ↓
ChatGptProtocol::build_request()
  → convert_messages()       // UnifiedMessage[] → InputItem[]
  → HTTP request
  ↓
Codex API → SSE response
  ↓
ChatGptProtocol::parse_response()
  → extract text, tool_calls, usage, stop_reason
  → response.validate("chatgpt")
  ↓
AgentLoop
  → push UnifiedMessage::Assistant { [Text, ToolCall, ToolCall] }
  → execute tools
  → push UnifiedMessage::tool_result(...) per tool
  → loop continues until EndTurn or limit
```

## File Change Summary

| Category | File | Change |
|----------|------|--------|
| **New** | `providers/message.rs` | UnifiedMessage, ContentBlock, transform_messages |
| **Core** | `providers/adapter.rs` | RequestPayload.messages, validate(), text_content() |
| **Core** | `providers/mod.rs` | AiProvider: 7 methods → 1 |
| **Core** | `providers/http_provider.rs` | PII filtering on messages, validate, remove transform_messages |
| **Protocol** | `protocols/chatgpt.rs` | convert_messages, usage extraction, status→StopReason |
| **Protocol** | `protocols/anthropic.rs` | convert_messages, merge ToolResult to user turn |
| **Protocol** | `protocols/openai.rs` | convert_messages, JSON.stringify arguments |
| **Protocol** | `protocols/gemini.rs` | convert_messages, user/model alternation |
| **Protocol** | `protocols/configurable.rs` | fallback: extract last user text |
| **Loop** | `agent_loop/loop_core.rs` | LoopMessage → UnifiedMessage, complete assistant turn |
| **Loop** | `agent_loop/provider_bridge.rs` | remove XML serialization, add transform_messages, pass-through |
| **Direct impl** | `providers/ollama.rs` | update AiProvider impl to new process(RequestPayload) signature |
| **Direct impl** | `providers/mock.rs` | update AiProvider impl |
| **Direct impl** | `providers/failover.rs` | update AiProvider impl (delegates to inner providers) |
| **Direct impl** | `providers/auth_profile_registry.rs` | update NoProfileProvider AiProvider impl |
| **Callers** | ~46 files | migrate to new process(RequestPayload) — see migration patterns above |
| **Tests** | all affected modules | update to new types |

**Unchanged:** ProtocolAdapter trait method signatures (syntactically), ProviderConfig, Gateway, Channel, Tool system.

**Deferred:** Streaming tool call events (`parse_stream` refactor).
