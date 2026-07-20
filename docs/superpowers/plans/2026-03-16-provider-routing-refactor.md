# Provider Routing Refactor Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace flat-string provider routing with structured `UnifiedMessage` types, enabling correct multi-turn conversations across all protocols.

**Architecture:** New `UnifiedMessage`/`ContentBlock` types in `providers/message.rs` become the universal message format. `RequestPayload.input: &str` becomes `RequestPayload.messages: &[UnifiedMessage]`. `AiProvider` trait simplifies from 7 request methods to 1. Each protocol adapter implements its own `convert_messages()`. Agent loop uses `UnifiedMessage` directly, eliminating XML serialization.

**Tech Stack:** Rust, serde, serde_json, async-trait, reqwest

**Spec:** `docs/superpowers/specs/2026-03-16-provider-routing-refactor-design.md`

---

## Chunk 1: New Types (message.rs)

### Task 1: Create UnifiedMessage and ContentBlock types

**Files:**
- Create: `src/providers/message.rs`
- Modify: `src/providers/mod.rs` (add `pub mod message;`)

- [ ] **Step 1: Create `src/providers/message.rs` with type definitions**

```rust
//! Unified message types for provider-agnostic conversation representation.
//!
//! These types are the single data model for all provider interactions.
//! Protocol adapters convert these to their native API formats.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unified message type — the single data model for all provider interactions.
///
/// Modeled after pi-mono's `Message = UserMessage | AssistantMessage | ToolResultMessage`.
/// Each protocol adapter converts these to its native format in `convert_messages()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UnifiedMessage {
    /// User message
    User { content: Vec<ContentBlock> },
    /// Assistant message (one turn may contain text + thinking + tool calls)
    Assistant { content: Vec<ContentBlock> },
    /// Tool execution result
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        content: Vec<ContentBlock>,
        is_error: bool,
    },
}

/// Content block — one atomic unit within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ContentBlock {
    /// Plain text
    Text { text: String },
    /// Structured JSON (preserves tool output structure)
    Json { value: Value },
    /// Thinking/reasoning trace
    Thinking { thinking: String },
    /// Tool call (only in Assistant messages)
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
    },
    /// Image (base64-encoded)
    Image { data: String, mime_type: String },
}

// === Convenience constructors ===

impl UnifiedMessage {
    /// Single text user message
    pub fn user(text: impl Into<String>) -> Self {
        Self::User {
            content: vec![ContentBlock::Text {
                text: text.into(),
            }],
        }
    }

    /// Single text assistant message
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::Assistant {
            content: vec![ContentBlock::Text {
                text: text.into(),
            }],
        }
    }

    /// Tool result with text output
    pub fn tool_result(
        call_id: impl Into<String>,
        name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_call_id: call_id.into(),
            tool_name: name.into(),
            content: vec![ContentBlock::Text {
                text: output.into(),
            }],
            is_error,
        }
    }

    /// Tool result with structured JSON output
    pub fn tool_result_json(
        call_id: impl Into<String>,
        name: impl Into<String>,
        value: Value,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_call_id: call_id.into(),
            tool_name: name.into(),
            content: vec![ContentBlock::Json { value }],
            is_error,
        }
    }

    /// Build an Assistant message from a ProviderResponse
    pub fn from_provider_response(resp: &super::adapter::ProviderResponse) -> Self {
        let mut content = Vec::new();
        if let Some(ref thinking) = resp.thinking {
            content.push(ContentBlock::Thinking {
                thinking: thinking.clone(),
            });
        }
        if let Some(ref text) = resp.text {
            content.push(ContentBlock::Text { text: text.clone() });
        }
        for tc in &resp.tool_calls {
            content.push(ContentBlock::ToolCall {
                id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            });
        }
        UnifiedMessage::Assistant { content }
    }

    /// Get mutable access to content blocks (for PII filtering)
    pub fn content_blocks_mut(&mut self) -> &mut Vec<ContentBlock> {
        match self {
            Self::User { content } => content,
            Self::Assistant { content } => content,
            Self::ToolResult { content, .. } => content,
        }
    }

    /// Get read access to content blocks
    pub fn content_blocks(&self) -> &[ContentBlock] {
        match self {
            Self::User { content } => content,
            Self::Assistant { content } => content,
            Self::ToolResult { content, .. } => content,
        }
    }

    /// Extract concatenated text from a slice of messages (for leak detection)
    pub fn extract_all_text(messages: &[UnifiedMessage]) -> String {
        let mut parts = Vec::new();
        for msg in messages {
            for block in msg.content_blocks() {
                if let ContentBlock::Text { text } = block {
                    parts.push(text.as_str());
                }
            }
        }
        parts.join("\n")
    }

    /// Check if this is a ToolCall-bearing Assistant message
    pub fn has_tool_calls(&self) -> bool {
        match self {
            Self::Assistant { content } => content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolCall { .. })),
            _ => false,
        }
    }

    /// Extract tool calls from an Assistant message
    pub fn tool_calls(&self) -> Vec<(&str, &str, &Value)> {
        match self {
            Self::Assistant { content } => content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolCall {
                        id,
                        name,
                        arguments,
                    } => Some((id.as_str(), name.as_str(), arguments)),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        }
    }
}

impl ContentBlock {
    /// Extract text content if this is a Text block
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }
}
```

- [ ] **Step 2: Add module declaration to `src/providers/mod.rs`**

Find `pub mod adapter;` and add after it:

```rust
pub mod message;
```

- [ ] **Step 3: Run `cargo check -p alephcore` to verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles successfully (message.rs is standalone, no existing code depends on it yet)

- [ ] **Step 4: Commit**

```bash
git add src/providers/message.rs src/providers/mod.rs
git commit -m "providers: add UnifiedMessage and ContentBlock types

New unified message types for provider-agnostic conversation representation.
These replace the flat string input in RequestPayload."
```

### Task 2: Add transform_messages pre-processing

**Files:**
- Modify: `src/providers/message.rs`

- [ ] **Step 1: Add transform_messages and repair_orphaned_tool_calls to message.rs**

Append to the end of `message.rs`:

```rust
// === Message pre-processing ===

/// Pre-process messages before sending to any provider.
///
/// 1. Repairs orphaned tool calls (Assistant ToolCall without matching ToolResult)
/// 2. Normalizes cross-model content (no-op for now, reserved for thinking signatures)
pub fn transform_messages(
    messages: &[UnifiedMessage],
    _target_provider: Option<&str>,
) -> Vec<UnifiedMessage> {
    let mut result = messages.to_vec();
    repair_orphaned_tool_calls(&mut result);
    // normalize_cross_model is a no-op for now
    result
}

/// Scan for Assistant ToolCall blocks without matching ToolResult.
/// Insert synthetic error ToolResult for each orphan.
fn repair_orphaned_tool_calls(messages: &mut Vec<UnifiedMessage>) {
    // Collect all tool_call_ids that have a matching ToolResult
    let answered_ids: std::collections::HashSet<&str> = messages
        .iter()
        .filter_map(|m| match m {
            UnifiedMessage::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();

    // Find orphaned tool calls (in Assistant messages, no matching ToolResult)
    let mut orphans: Vec<(String, String)> = Vec::new();
    for msg in messages.iter() {
        if let UnifiedMessage::Assistant { content } = msg {
            for block in content {
                if let ContentBlock::ToolCall { id, name, .. } = block {
                    if !answered_ids.contains(id.as_str()) {
                        orphans.push((id.clone(), name.clone()));
                    }
                }
            }
        }
    }

    // Insert synthetic error ToolResult for each orphan
    for (id, name) in orphans {
        messages.push(UnifiedMessage::tool_result(
            id,
            name,
            "No result provided — tool call was interrupted",
            true,
        ));
    }
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_user_convenience() {
        let msg = UnifiedMessage::user("hello");
        match &msg {
            UnifiedMessage::User { content } => {
                assert_eq!(content.len(), 1);
                assert_eq!(content[0].as_text(), Some("hello"));
            }
            _ => panic!("expected User"),
        }
    }

    #[test]
    fn test_assistant_convenience() {
        let msg = UnifiedMessage::assistant("response");
        match &msg {
            UnifiedMessage::Assistant { content } => {
                assert_eq!(content[0].as_text(), Some("response"));
            }
            _ => panic!("expected Assistant"),
        }
    }

    #[test]
    fn test_tool_result_convenience() {
        let msg = UnifiedMessage::tool_result("call_1", "search", "found 3 results", false);
        match &msg {
            UnifiedMessage::ToolResult {
                tool_call_id,
                tool_name,
                is_error,
                content,
            } => {
                assert_eq!(tool_call_id, "call_1");
                assert_eq!(tool_name, "search");
                assert!(!is_error);
                assert_eq!(content[0].as_text(), Some("found 3 results"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_json() {
        let msg = UnifiedMessage::tool_result_json(
            "call_1",
            "search",
            json!({"results": [1, 2, 3]}),
            false,
        );
        match &msg {
            UnifiedMessage::ToolResult { content, .. } => {
                assert!(matches!(&content[0], ContentBlock::Json { .. }));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_from_provider_response() {
        use super::super::adapter::{NativeToolCall, ProviderResponse};
        let resp = ProviderResponse {
            text: Some("I'll search for that.".into()),
            tool_calls: vec![NativeToolCall {
                id: "call_1".into(),
                name: "search".into(),
                arguments: json!({"query": "rust"}),
            }],
            thinking: Some("Let me think...".into()),
            ..Default::default()
        };
        let msg = UnifiedMessage::from_provider_response(&resp);
        match &msg {
            UnifiedMessage::Assistant { content } => {
                assert_eq!(content.len(), 3); // thinking + text + tool_call
                assert!(matches!(&content[0], ContentBlock::Thinking { .. }));
                assert!(matches!(&content[1], ContentBlock::Text { .. }));
                assert!(matches!(&content[2], ContentBlock::ToolCall { .. }));
            }
            _ => panic!("expected Assistant"),
        }
    }

    #[test]
    fn test_extract_all_text() {
        let messages = vec![
            UnifiedMessage::user("hello"),
            UnifiedMessage::assistant("world"),
        ];
        let text = UnifiedMessage::extract_all_text(&messages);
        assert_eq!(text, "hello\nworld");
    }

    #[test]
    fn test_has_tool_calls() {
        let msg = UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Text {
                    text: "searching".into(),
                },
                ContentBlock::ToolCall {
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: json!({}),
                },
            ],
        };
        assert!(msg.has_tool_calls());
        assert!(!UnifiedMessage::user("hello").has_tool_calls());
    }

    #[test]
    fn test_repair_orphaned_tool_calls_no_orphans() {
        let messages = vec![
            UnifiedMessage::user("search for rust"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: json!({"q": "rust"}),
                }],
            },
            UnifiedMessage::tool_result("c1", "search", "found", false),
        ];
        let result = transform_messages(&messages, None);
        assert_eq!(result.len(), 3); // no synthetic results added
    }

    #[test]
    fn test_repair_orphaned_tool_calls_with_orphan() {
        let messages = vec![
            UnifiedMessage::user("search for rust"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: json!({"q": "rust"}),
                }],
            },
            // Missing ToolResult for c1!
        ];
        let result = transform_messages(&messages, None);
        assert_eq!(result.len(), 3); // synthetic ToolResult added
        match &result[2] {
            UnifiedMessage::ToolResult {
                tool_call_id,
                is_error,
                ..
            } => {
                assert_eq!(tool_call_id, "c1");
                assert!(is_error);
            }
            _ => panic!("expected synthetic ToolResult"),
        }
    }

    #[test]
    fn test_content_blocks_mut() {
        let mut msg = UnifiedMessage::user("original");
        for block in msg.content_blocks_mut() {
            if let ContentBlock::Text { ref mut text } = block {
                *text = "filtered".to_string();
            }
        }
        assert_eq!(msg.content_blocks()[0].as_text(), Some("filtered"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib providers::message`
Expected: All tests pass

- [ ] **Step 3: Commit**

```bash
git add src/providers/message.rs
git commit -m "providers: add transform_messages and message type tests"
```

---

## Chunk 2: Core Trait Changes

This chunk changes `RequestPayload`, `AiProvider`, and all provider implementations atomically. These MUST be done together because Rust's type system enforces compilation coherence.

### Task 3: Refactor RequestPayload and AiProvider trait

**Files:**
- Modify: `src/providers/adapter.rs`
- Modify: `src/providers/mod.rs`

- [ ] **Step 1: Refactor `RequestPayload` in `adapter.rs`**

Replace the existing `RequestPayload` struct and its `impl` block with:

```rust
use super::message::UnifiedMessage;

/// Unified request payload for protocol adapters.
///
/// Protocol adapters translate this into provider-specific request formats.
#[derive(Debug)]
pub struct RequestPayload<'a> {
    /// Structured message list
    pub messages: &'a [UnifiedMessage],
    /// System prompt (handled differently per provider)
    pub system_prompt: Option<&'a str>,
    /// Tool definitions for native tool_use
    pub tools: Option<&'a [ToolDefinition]>,
    /// Thinking/reasoning level
    pub think_level: Option<ThinkLevel>,
    /// Per-request temperature override
    pub temperature: Option<f32>,
    /// Per-request max_tokens override
    pub max_tokens: Option<u32>,
}

impl<'a> Default for RequestPayload<'a> {
    fn default() -> Self {
        Self {
            messages: &[],
            system_prompt: None,
            tools: None,
            think_level: None,
            temperature: None,
            max_tokens: None,
        }
    }
}

impl<'a> RequestPayload<'a> {
    /// Create payload from messages
    pub fn new(messages: &'a [UnifiedMessage]) -> Self {
        Self {
            messages,
            ..Default::default()
        }
    }

    /// Add system prompt
    pub fn with_system(mut self, prompt: Option<&'a str>) -> Self {
        self.system_prompt = prompt;
        self
    }

    /// Add tools
    pub fn with_tools(mut self, tools: Option<&'a [ToolDefinition]>) -> Self {
        self.tools = tools;
        self
    }

    /// Set thinking level
    pub fn with_think_level(mut self, level: Option<ThinkLevel>) -> Self {
        self.think_level = level;
        self
    }

    /// Set temperature
    pub fn with_temperature(mut self, temperature: Option<f32>) -> Self {
        self.temperature = temperature;
        self
    }

    /// Set max_tokens
    pub fn with_max_tokens(mut self, max_tokens: Option<u32>) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}
```

Remove the old `with_image`, `with_attachments`, `with_force_standard_mode` builder methods. Remove the `image`, `attachments`, `force_standard_mode` fields. Remove the import of `ImageData` and `MediaAttachment` if no longer needed.

Add `text_content()` and `validate()` to `ProviderResponse`:

```rust
impl ProviderResponse {
    // ... existing text_only() and has_tool_calls() ...

    /// Extract text content (convenience for callers migrating from String return)
    pub fn text_content(&self) -> String {
        self.text.clone().unwrap_or_default()
    }

    /// Validate response completeness — warns on missing usage or unknown stop reason
    pub fn validate(&self, protocol_name: &str) {
        if self.usage.is_none() {
            tracing::warn!(protocol = protocol_name, "Provider response missing usage data");
        }
        if self.stop_reason == StopReason::Unknown {
            tracing::warn!(protocol = protocol_name, "Provider response has Unknown stop_reason");
        }
    }
}
```

- [ ] **Step 2: Simplify `AiProvider` trait in `mod.rs`**

Replace the 7 request methods with 1. Keep `name()`, `color()`, `supports_native_tools()`, `supports_thinking()`. Remove: `process(&str, ...)`, `process_with_image`, `process_with_attachments`, `process_with_thinking`, `process_with_overrides`, `process_with_payload`.

New trait:

```rust
pub trait AiProvider: Send + Sync {
    /// Core method — process a request and return structured response
    fn process(
        &self,
        payload: adapter::RequestPayload<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + '_>>;

    /// Provider name
    fn name(&self) -> &str;

    /// Provider brand color
    fn color(&self) -> &str;

    /// Whether this provider supports native tool_use
    fn supports_native_tools(&self) -> bool {
        false
    }

    /// Whether this provider supports extended thinking
    fn supports_thinking(&self) -> bool {
        false
    }
}
```

- [ ] **Step 3: DO NOT compile yet** — continue to Task 4 (all implementors must be updated first)

### Task 4: Update all AiProvider implementors

**Files:**
- Modify: `src/providers/http_provider.rs`
- Modify: `src/providers/ollama.rs`
- Modify: `src/providers/mock.rs`
- Modify: `src/providers/failover.rs`
- Modify: `src/providers/auth_profile_registry.rs`
- Modify: `src/providers/registry.rs` (if it has trait impls)

- [ ] **Step 1: Update `HttpProvider` in `http_provider.rs`**

The `execute()` method currently reads `payload.input` for PII filtering and leak detection. Change to iterate message text blocks:

```rust
async fn execute(&self, payload: RequestPayload<'_>) -> Result<ProviderResponse> {
    // PII filtering: filter each text block individually
    let mut filtered_messages: Vec<UnifiedMessage> = payload.messages.to_vec();
    if let Some(engine_lock) = crate::pii::PiiEngine::global() {
        if let Ok(engine) = engine_lock.read() {
            if !engine.is_provider_excluded(&self.name) {
                for msg in &mut filtered_messages {
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
        }
    }

    // Secret leak detection: scan all text content
    let detector = LeakDetector::new();
    let all_text = UnifiedMessage::extract_all_text(&filtered_messages);
    if let LeakDecision::Block { reason, .. } = detector.scan_outbound(&all_text) {
        tracing::warn!(provider = %self.name, reason = %reason, "Blocked outbound request: secret leak detected");
        return Err(crate::error::AlephError::PermissionDenied {
            message: format!("Secret leak blocked: {}", reason),
            suggestion: Some("Remove secret values from the input before sending.".into()),
        });
    }

    let final_payload = RequestPayload {
        messages: &filtered_messages,
        system_prompt: payload.system_prompt,
        tools: payload.tools,
        think_level: payload.think_level,
        temperature: payload.temperature,
        max_tokens: payload.max_tokens,
    };

    let request = self.adapter.build_request(&final_payload, &self.config, false)?;
    let response = request.send().await.map_err(|e| {
        if e.is_timeout() {
            crate::error::AlephError::Timeout {
                suggestion: Some("Request timed out. Try again or switch providers.".into()),
            }
        } else {
            crate::error::AlephError::network(format!("Network error: {}", e))
        }
    })?;

    let provider_response = self.adapter.parse_response(response).await?;

    // Validate response
    provider_response.validate(self.adapter.name());

    // Secret leak detection: scan inbound text
    if let Some(ref text) = provider_response.text {
        if let LeakDecision::Block { reason, .. } = detector.scan_inbound(text) {
            tracing::warn!(provider = %self.name, reason = %reason, "Blocked inbound response");
            return Err(crate::error::AlephError::PermissionDenied {
                message: format!("Secret leak in response blocked: {}", reason),
                suggestion: Some("The AI provider response contained a secret value.".into()),
            });
        }
    }

    Ok(provider_response)
}
```

Update the `AiProvider` impl for `HttpProvider`:

```rust
impl AiProvider for HttpProvider {
    fn process(
        &self,
        payload: RequestPayload<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + '_>> {
        Box::pin(async move { self.execute(payload).await })
    }

    fn supports_native_tools(&self) -> bool {
        self.adapter.supports_native_tools()
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.config.color
    }
}
```

- [ ] **Step 2: Update `OllamaProvider` in `ollama.rs`**

Replace the existing `AiProvider` impl. Read the current implementation first — it likely calls a local HTTP endpoint. The new impl takes `RequestPayload` and extracts the last user message text (Ollama is text-only for now):

```rust
impl AiProvider for OllamaProvider {
    fn process(
        &self,
        payload: RequestPayload<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + '_>> {
        // Extract text from messages (Ollama: last user message text)
        let input = payload.messages.iter().rev()
            .find_map(|m| match m {
                UnifiedMessage::User { content } => content.iter().find_map(|b| b.as_text()),
                _ => None,
            })
            .unwrap_or("")
            .to_string();
        let system = payload.system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            // ... existing Ollama HTTP call logic using `input` and `system` ...
            // Wrap result in ProviderResponse::text_only(result)
        })
    }
    // ... name(), color() unchanged ...
}
```

- [ ] **Step 3: Update `MockProvider` in `mock.rs`**

Read the current implementation, then update to accept `RequestPayload`. It likely echoes or returns canned responses.

- [ ] **Step 4: Update `FailoverProvider` in `failover.rs`**

Delegates to inner providers. Update to pass `RequestPayload` through. Note: `RequestPayload` has a lifetime — the failover provider must clone messages for retry with the fallback.

- [ ] **Step 5: Update `NoProfileProvider` in `auth_profile_registry.rs`**

This likely returns an error or stub. Update signature only.

- [ ] **Step 6: Update `TestProvider` in `providers/mod.rs` tests**

Update the inline test provider to match the new trait.

- [ ] **Step 7: DO NOT compile yet** — continue to Task 5

### Task 5: Update all protocol adapters

**Files:**
- Modify: `src/providers/protocols/chatgpt.rs`
- Modify: `src/providers/protocols/anthropic.rs`
- Modify: `src/providers/protocols/openai.rs`
- Modify: `src/providers/protocols/gemini.rs`
- Modify: `src/providers/protocols/configurable.rs`

Each adapter's `build_request` must change from reading `payload.input` (flat string) to reading `payload.messages` (&[UnifiedMessage]) and converting to native format.

- [ ] **Step 1: Update ChatGPT protocol (`chatgpt.rs`)**

Replace `parse_input_items()` (XML parsing) with `convert_messages()`:

```rust
fn convert_messages(messages: &[UnifiedMessage]) -> Vec<InputItem> {
    let mut items = Vec::new();
    for msg in messages {
        match msg {
            UnifiedMessage::User { content } => {
                let text = content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("\n");
                items.push(InputItem::Message { role: "user".into(), content: text });
            }
            UnifiedMessage::Assistant { content } => {
                // Text part as assistant message
                let text: String = content.iter().filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                }).collect::<Vec<_>>().join("\n");
                if !text.is_empty() {
                    items.push(InputItem::Message { role: "assistant".into(), content: text });
                }
                // Tool calls as FunctionCall items
                for block in content {
                    if let ContentBlock::ToolCall { id, name, arguments } = block {
                        items.push(InputItem::FunctionCall {
                            call_id: id.clone(),
                            name: name.clone(),
                            arguments: serde_json::to_string(arguments).unwrap_or_default(),
                        });
                    }
                }
            }
            UnifiedMessage::ToolResult { tool_call_id, content, .. } => {
                let output = content.iter().map(|b| match b {
                    ContentBlock::Text { text } => text.clone(),
                    ContentBlock::Json { value } => serde_json::to_string(value).unwrap_or_default(),
                    _ => String::new(),
                }).collect::<Vec<_>>().join("\n");
                items.push(InputItem::FunctionCallOutput {
                    call_id: tool_call_id.clone(),
                    output,
                });
            }
        }
    }
    items
}
```

In `build_request`, replace `Self::parse_input_items(payload.input)` with `Self::convert_messages(payload.messages)`.

Fix `parse_response` to extract usage from `Completed` event and map `response.status == "incomplete"` → `StopReason::MaxTokens`:

```rust
// In the Completed arm of parse_response:
StreamEvent::Completed { ref response } => {
    if let Some(full_text) = Self::extract_text(response) {
        result = full_text;
    }
    tool_calls = Self::extract_tool_calls(response);
    // Extract usage
    if let Some(ref u) = response.usage {
        usage = Some(TokenUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            ..Default::default()
        });
    }
    // Map status to stop_reason
    completed_status = Some(response.status.clone());
}

// After the SSE loop, determine stop_reason:
let stop_reason = if !tool_calls.is_empty() {
    StopReason::ToolUse
} else if completed_status.as_deref() == Some("incomplete") {
    StopReason::MaxTokens
} else {
    StopReason::EndTurn
};
```

Remove the `parse_input_items` and `try_parse_tag` methods entirely.

- [ ] **Step 2: Update Anthropic protocol (`anthropic.rs`)**

Replace `build_text_messages(payload)` and `build_multimodal_messages(payload)` with `convert_messages(payload.messages)`:

```rust
fn convert_messages(messages: &[UnifiedMessage]) -> Vec<Message> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        match &messages[i] {
            UnifiedMessage::User { content } => {
                result.push(Self::convert_user_message(content));
                i += 1;
            }
            UnifiedMessage::Assistant { content } => {
                result.push(Self::convert_assistant_message(content));
                i += 1;
            }
            UnifiedMessage::ToolResult { .. } => {
                // Collect consecutive ToolResults into one user message
                let mut tool_results = Vec::new();
                while i < messages.len() {
                    if let UnifiedMessage::ToolResult { tool_call_id, content, is_error, .. } = &messages[i] {
                        tool_results.push((tool_call_id.clone(), content.clone(), *is_error));
                        i += 1;
                    } else {
                        break;
                    }
                }
                result.push(Self::merge_tool_results(&tool_results));
            }
        }
    }
    result
}
```

Add helper methods for each conversion. Sanitize tool_use_id: `id.replace(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-', "_")` and truncate to 64 chars.

- [ ] **Step 3: Update OpenAI protocol (`openai.rs`)**

Replace `build_text_messages` with `convert_messages`:
- User → `{ role: "user", content: text_string }`
- Assistant → `{ role: "assistant", content: text_string, tool_calls: [...] }`
- ToolResult → `{ role: "tool", content: output_string, tool_call_id: id }` (one per result, NOT merged)

Arguments must be `JSON.stringify`'d strings, not objects.

- [ ] **Step 4: Update Gemini protocol (`gemini.rs`)**

Replace message construction with `convert_messages`:
- Enforce strict user/model alternation
- Merge consecutive ToolResults into one user turn with multiple `functionResponse` parts
- Map ToolCall to `functionCall` in model parts

- [ ] **Step 5: Update Configurable protocol (`configurable.rs`)**

Extract last User message text as the template input:

```rust
let input_text = payload.messages.iter().rev()
    .find_map(|m| match m {
        UnifiedMessage::User { content } => {
            Some(content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("\n"))
        }
        _ => None,
    })
    .unwrap_or_default();
```

- [ ] **Step 6: DO NOT compile yet** — continue to Task 6

---

## Chunk 3: Agent Loop Refactor

### Task 6: Replace LoopMessage with UnifiedMessage in agent loop

**Files:**
- Modify: `src/agent_loop/loop_core.rs`
- Modify: `src/agent_loop/provider_bridge.rs`
- Modify: `src/agent_loop/tool.rs` (if ToolDefinition changes)
- Modify: `src/agent_loop/mod.rs` (if it re-exports LoopMessage)

- [ ] **Step 1: Update `LoopProvider` trait in `loop_core.rs`**

Change `LoopMessage` references to `UnifiedMessage`. Remove the `LoopMessage` enum entirely.

```rust
use crate::providers::message::UnifiedMessage;

#[async_trait]
pub trait LoopProvider: Send + Sync {
    async fn call(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<ProviderResponse>;
}
```

- [ ] **Step 2: Update `AgentLoop::run_with_history` in `loop_core.rs`**

Replace all `LoopMessage` usage with `UnifiedMessage`. Key changes:

1. History parameter: `history: Vec<UnifiedMessage>` instead of `Vec<LoopMessage>`
2. Push user message: `messages.push(UnifiedMessage::user(input))`
3. After provider response, push complete assistant message:
   ```rust
   messages.push(UnifiedMessage::from_provider_response(&response));
   ```
4. Check tool calls from the response (not from messages):
   ```rust
   if !response.has_tool_calls() && response.stop_reason == StopReason::EndTurn { break; }
   ```
5. Push tool results with tool name:
   ```rust
   messages.push(UnifiedMessage::tool_result(tc.id.clone(), tc.name.clone(), output_text, is_error));
   ```
   For JSON outputs, use `UnifiedMessage::tool_result_json(...)`.

- [ ] **Step 3: Simplify `AiProviderBridge` in `provider_bridge.rs`**

Remove `format_messages()` (XML serialization), `format_json_compact()`, and `convert_tool_def()`. The bridge becomes a thin pass-through:

```rust
use crate::providers::adapter::RequestPayload;
use crate::providers::message::{transform_messages, UnifiedMessage};

pub struct AiProviderBridge {
    provider: Arc<dyn AiProvider>,
}

impl AiProviderBridge {
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl LoopProvider for AiProviderBridge {
    async fn call(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<ProviderResponse> {
        // Pre-process: repair orphaned tool calls
        let cleaned = transform_messages(messages, Some(self.provider.name()));

        // Convert loop ToolDefinitions to dispatcher ToolDefinitions
        let dispatcher_tools: Vec<DispatcherToolDefinition> =
            tools.iter().map(|def| DispatcherToolDefinition {
                name: def.name.clone(),
                description: def.description.clone(),
                parameters: def.parameters.clone(),
                requires_confirmation: false,
                category: ToolCategory::Builtin,
                llm_context: None,
                strict: false,
            }).collect();

        let payload = RequestPayload {
            messages: &cleaned,
            system_prompt: Some(system_prompt),
            tools: if dispatcher_tools.is_empty() { None } else { Some(&dispatcher_tools) },
            ..Default::default()
        };

        self.provider
            .process(payload)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
}
```

- [ ] **Step 4: Update `run_loop.rs` (gateway execution engine)**

`build_loop_history` likely constructs `Vec<LoopMessage>` from session data. Update to construct `Vec<UnifiedMessage>`.

- [ ] **Step 5: DO NOT compile yet** — continue to Task 7

---

## Chunk 4: Caller Migration

### Task 7: Migrate all ~25 AiProvider callers

**Files:** All files that call `.process()` on an `AiProvider`

Every caller follows the same mechanical pattern:

```rust
// Before:
let text = provider.process(&input, system_prompt.as_deref()).await?;

// After:
let msgs = [UnifiedMessage::user(&input)];
let text = provider
    .process(RequestPayload::new(&msgs).with_system(system_prompt.as_deref()))
    .await?
    .text_content();
```

Add these imports to each file:
```rust
use crate::providers::message::UnifiedMessage;
use crate::providers::adapter::RequestPayload;
```

- [ ] **Step 1: Migrate gateway callers (4 files)**

- `gateway/inbound_router/command_handler.rs`
- `gateway/inbound_router/switch_intent.rs`
- `gateway/intent_detector.rs`
- `gateway/handlers/providers/handlers.rs`

- [ ] **Step 2: Migrate dispatcher callers (4 files)**

- `dispatcher/analyzer.rs`
- `dispatcher/planner/llm.rs` (2 call sites)
- `dispatcher/tool_index/inference.rs`
- `dispatcher/engine/routing.rs`

- [ ] **Step 3: Migrate memory callers (7 files)**

- `memory/ai_retrieval.rs`
- `memory/compression/extractor.rs`
- `memory/evolution/detector.rs`
- `memory/value_estimator/llm_scorer.rs`
- `memory/vfs/l1_generator.rs`
- `memory/cortex/meta_cognition/reactive.rs`
- `memory/cortex/meta_cognition/critic.rs`

- [ ] **Step 4: Migrate group_chat, spec_driven, a2a, agent_init (7 files)**

- `group_chat/executor.rs` (2 call sites)
- `spec_driven/judge.rs`
- `spec_driven/spec_writer.rs`
- `spec_driven/test_writer.rs`
- `a2a/service/llm_matcher.rs`
- `bin/aleph/commands/start/builder/agent_init.rs`

- [ ] **Step 5: Search for any remaining callers**

Run: `cargo check -p alephcore 2>&1 | head -100`

Fix any remaining compilation errors from callers that were missed. Also check:
- `a2a/adapter/server/request_processor.rs`
- `a2a/adapter/server/routes.rs`
- `gateway/webhooks/handler.rs`
- `daemon/resource_governor.rs`

- [ ] **Step 6: Fix all remaining compilation errors**

Iterate on `cargo check -p alephcore` until it compiles successfully. This may require fixing:
- Test files that use old trait methods
- Mock/test providers in various test modules
- Re-exports in mod.rs files
- Lifetime issues in failover retry logic

- [ ] **Step 7: Commit once compilation passes**

```bash
git add -A
git commit -m "providers: unified message architecture refactor

Replace flat-string provider routing with structured UnifiedMessage types.
- New UnifiedMessage/ContentBlock types in providers/message.rs
- RequestPayload.input: &str → RequestPayload.messages: &[UnifiedMessage]
- AiProvider trait: 7 methods → 1 process(RequestPayload)
- Per-protocol convert_messages (chatgpt, anthropic, openai, gemini, configurable)
- Agent loop uses UnifiedMessage directly, XML serialization removed
- transform_messages pre-processing for orphaned tool call repair
- PII filtering updated to per-block iteration
- All ~25 callers migrated to new API"
```

---

## Chunk 5: Testing and Verification

### Task 8: Run tests and fix failures

**Files:** Various test files

- [ ] **Step 1: Run core tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -50`

Fix any test failures. Common issues:
- Tests that construct `LoopMessage` directly → change to `UnifiedMessage`
- Tests that call old `process(&str, ...)` → use new pattern
- Mock providers in tests that implement old trait

- [ ] **Step 2: Fix loop_core tests**

Update tests in `loop_core.rs` that use `LoopMessage` to use `UnifiedMessage` instead. Update `FakeProvider` test helpers.

- [ ] **Step 3: Fix provider_bridge tests**

Tests that verify XML format are no longer relevant. Replace with tests verifying messages pass through correctly.

- [ ] **Step 4: Fix protocol adapter tests**

Each adapter's tests need to use `RequestPayload { messages: &[...], .. }` instead of `RequestPayload { input: "...", .. }`.

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: All tests pass (except pre-existing failures in `tools::markdown_skill::loader::tests`)

- [ ] **Step 6: Commit test fixes**

```bash
git add -A
git commit -m "tests: update all tests for unified message architecture"
```

### Task 9: Verify with cargo clippy

- [ ] **Step 1: Run clippy**

Run: `cargo clippy -p alephcore 2>&1 | tail -30`

- [ ] **Step 2: Fix any new clippy warnings**

- [ ] **Step 3: Final commit**

```bash
git add -A
git commit -m "fix: address clippy warnings from provider refactor"
```
