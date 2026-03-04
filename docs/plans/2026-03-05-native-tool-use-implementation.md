# Native Tool Use Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Migrate Aleph's LLM communication from JSON-in-text to native tool_use API across all providers.

**Architecture:** Introduce `ProviderResponse` as the unified return type from `ProtocolAdapter` and `AiProvider`, replacing the current `String` return. Each protocol (Anthropic, OpenAI, Gemini) implements native tool_use in `build_request()` and `parse_response()`. Virtual tools (complete, ask_user, fail) ensure ALL LLM decisions go through tool_use. The Thinker maps native tool calls directly to `Decision` variants, bypassing `DecisionParser` on the main path.

**Tech Stack:** Rust, serde_json, reqwest, async-trait, schemars (existing deps only)

**Design doc:** `docs/plans/2026-03-05-native-tool-use-migration-design.md`

---

### Task 1: Core Types — ProviderResponse & RequestPayload Extension

**Files:**
- Modify: `core/src/providers/adapter.rs`
- Test: `cargo test -p alephcore --lib providers::adapter`

**Context:** Currently `ProtocolAdapter::parse_response()` returns `String` and `RequestPayload` has no tool-related fields. We need new types that all protocols will return, and a way to pass tool definitions through.

**Step 1: Add ProviderResponse and related types to adapter.rs**

After the existing `RequestPayload` impl block (around line 80), add:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::dispatcher::types::ToolDefinition;

/// Structured response from an LLM provider
#[derive(Debug, Clone, Default)]
pub struct ProviderResponse {
    /// LLM text output (for non-tool responses)
    pub text: Option<String>,
    /// Native tool calls from the LLM
    pub tool_calls: Vec<NativeToolCall>,
    /// Thinking/reasoning process (extended thinking)
    pub thinking: Option<String>,
    /// Why the LLM stopped generating
    pub stop_reason: StopReason,
    /// Token usage statistics
    pub usage: Option<TokenUsage>,
}

impl ProviderResponse {
    /// Create a text-only response (for fallback providers)
    pub fn text_only(text: String) -> Self {
        Self {
            text: Some(text),
            stop_reason: StopReason::EndTurn,
            ..Default::default()
        }
    }

    /// Whether this response contains native tool calls
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// A native tool call from the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeToolCall {
    /// Provider-assigned ID (used for tool_result passback)
    pub id: String,
    /// Tool name
    pub name: String,
    /// Tool arguments as JSON
    pub arguments: Value,
}

/// Why the LLM stopped generating
#[derive(Debug, Clone, Default, PartialEq)]
pub enum StopReason {
    /// LLM finished its response naturally
    #[default]
    EndTurn,
    /// LLM wants to call a tool
    ToolUse,
    /// Hit max_tokens limit
    MaxTokens,
    /// Unknown or unsupported stop reason
    Unknown,
}

/// Token usage statistics
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
}
```

**Step 2: Add `tools` field to `RequestPayload`**

In the `RequestPayload` struct definition (line ~20), add after `max_tokens`:

```rust
    /// Tool definitions for native tool_use (None = no tools / fallback mode)
    pub tools: Option<&'a [ToolDefinition]>,
```

Add builder method after the existing `with_max_tokens`:

```rust
    pub fn with_tools(mut self, tools: Option<&'a [ToolDefinition]>) -> Self {
        self.tools = tools;
        self
    }
```

**Step 3: Write unit tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_response_text_only() {
        let resp = ProviderResponse::text_only("hello".to_string());
        assert_eq!(resp.text.as_deref(), Some("hello"));
        assert!(!resp.has_tool_calls());
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn test_provider_response_with_tool_calls() {
        let resp = ProviderResponse {
            tool_calls: vec![NativeToolCall {
                id: "call_123".into(),
                name: "search".into(),
                arguments: serde_json::json!({"query": "test"}),
            }],
            stop_reason: StopReason::ToolUse,
            ..Default::default()
        };
        assert!(resp.has_tool_calls());
        assert_eq!(resp.tool_calls[0].name, "search");
    }

    #[test]
    fn test_request_payload_with_tools() {
        let payload = RequestPayload::new("test input")
            .with_tools(None);
        assert!(payload.tools.is_none());
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib providers::adapter`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add core/src/providers/adapter.rs
git commit -m "providers: add ProviderResponse types and RequestPayload tools field"
```

---

### Task 2: ProtocolAdapter & AiProvider Trait Migration

**Files:**
- Modify: `core/src/providers/adapter.rs` (ProtocolAdapter trait)
- Modify: `core/src/providers/mod.rs` (AiProvider trait)
- Modify: `core/src/providers/http_provider.rs` (HttpProvider impl)
- Modify: All protocol impls + Ollama + consumers of AiProvider

**Context:** This is the biggest structural change. `parse_response()` returns `ProviderResponse` instead of `String`, and all `AiProvider::process_*()` methods return `ProviderResponse` instead of `String`. This will cause cascading compile errors across the codebase — fix them all in this task.

**Step 1: Change ProtocolAdapter::parse_response return type**

In `adapter.rs`, change:
```rust
// Before:
async fn parse_response(&self, response: reqwest::Response) -> Result<String>;
// After:
async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse>;
```

Add capability method:
```rust
    /// Whether this protocol supports native tool_use
    fn supports_native_tools(&self) -> bool { false }
```

**Step 2: Update all ProtocolAdapter implementations to compile**

For each protocol (`openai.rs`, `anthropic.rs`, `gemini.rs`, `chatgpt.rs`), wrap the existing text extraction in `ProviderResponse::text_only()`:

```rust
// Example for openai.rs parse_response():
// Before:
Ok(response_body.choices[0].message.content.clone())
// After:
Ok(ProviderResponse::text_only(response_body.choices[0].message.content.clone()))
```

Do this for ALL protocols as a mechanical change. Native tool_use will be added per-protocol in later tasks.

Also update any `ConfigurableProtocol` or dynamic protocol implementations.

**Step 3: Change AiProvider trait return types**

In `mod.rs`, change ALL `process_*` method signatures:

```rust
// Before (for each process_* method):
-> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>>
// After:
-> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + '_>>
```

Methods to change: `process`, `process_with_image`, `process_with_attachments`, `process_with_mode`, `process_with_thinking`, `process_with_overrides`.

**Step 4: Update HttpProvider**

In `http_provider.rs`:
- `execute()` return type: `Result<String>` → `Result<ProviderResponse>`
- All `AiProvider` impl methods: already delegate to `execute()`, so just change return type annotations
- PII filtering and secret leak detection operate on `response.text` instead of the raw string:

```rust
// In execute(), after adapter.parse_response():
let response = self.adapter.parse_response(http_response).await?;
// PII/secret filtering on text field only
if let Some(ref text) = response.text {
    // existing PII/secret filtering logic applied to text
}
Ok(response)
```

**Step 5: Update Ollama provider**

`core/src/providers/ollama.rs` — if it implements `AiProvider` directly (not through `HttpProvider`), update its return types similarly, wrapping in `ProviderResponse::text_only()`.

**Step 6: Fix all consumers**

Search for all call sites that use `.process(`, `.process_with_overrides(`, etc. and expect a `String`. The main consumer is:
- `core/src/thinker/mod.rs` — `call_llm_with_level()` which calls `provider.process_with_overrides()`. For now, extract the text field:

```rust
// Temporary compatibility: extract text from ProviderResponse
let response = provider.process_with_overrides(...).await?;
let response_text = response.text.unwrap_or_default();
// Continue with existing DecisionParser logic using response_text
```

Store the full `ProviderResponse` for later use (Task 7 will use it properly).

Other consumers (if any): gateway handlers, test utilities, etc. — update to work with `ProviderResponse`.

**Step 7: Compile and run all tests**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`
Expected: Compiles and all tests pass (behavior unchanged, just type wrapping).

**Step 8: Commit**

```bash
git add -A
git commit -m "providers: migrate ProtocolAdapter and AiProvider to return ProviderResponse"
```

---

### Task 3: Anthropic Protocol — Native Tool Use

**Files:**
- Modify: `core/src/providers/anthropic/types.rs`
- Modify: `core/src/providers/protocols/anthropic.rs`
- Test: `cargo test -p alephcore --lib providers::anthropic`

**Context:** Anthropic API natively supports `tools` in requests and returns `tool_use` content blocks. This is the most important protocol to implement since Claude is the primary LLM.

**Step 1: Add tool types to `anthropic/types.rs`**

After the existing `ThinkingBlock` (line ~49), add:

```rust
/// Tool definition for Anthropic API
#[derive(Debug, Clone, Serialize)]
pub struct AnthropicTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Tool-use content block in response
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicContentBlock {
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

/// Tool result content block (for sending back results)
#[derive(Debug, Clone, Serialize)]
pub struct ToolResultBlock {
    #[serde(rename = "type")]
    pub block_type: String, // always "tool_result"
    pub tool_use_id: String,
    pub content: String,
}
```

Update `MessagesRequest` to include tools:
```rust
pub struct MessagesRequest {
    // ... existing fields ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicTool>>,
}
```

Update `MessagesResponse` to use the new content block enum:
```rust
pub struct MessagesResponse {
    pub content: Vec<AnthropicContentBlock>,
    pub stop_reason: Option<String>,
    pub usage: Option<AnthropicUsage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicUsage {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cache_read_input_tokens: Option<u32>,
}
```

**Step 2: Update `build_request()` in `anthropic.rs`**

In `build_request()` (line ~195), add tools to the request body:

```rust
// Convert ToolDefinitions to AnthropicTools
let tools: Option<Vec<AnthropicTool>> = payload.tools.map(|defs| {
    defs.iter().map(|td| AnthropicTool {
        name: td.name.clone(),
        description: td.description.clone(),
        input_schema: td.parameters.clone(),
    }).collect()
});

let request_body = MessagesRequest {
    // ... existing fields ...
    tools,
};
```

**Step 3: Update `parse_response()` to extract tool_use blocks**

Replace the current `parse_response()` implementation:

```rust
async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse> {
    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        // ... existing error handling ...
    }

    let response_body: MessagesResponse = serde_json::from_str(&body)?;

    let mut provider_response = ProviderResponse::default();

    for block in &response_body.content {
        match block {
            AnthropicContentBlock::Text { text } => {
                provider_response.text = Some(text.clone());
            }
            AnthropicContentBlock::Thinking { thinking } => {
                provider_response.thinking = Some(thinking.clone());
            }
            AnthropicContentBlock::ToolUse { id, name, input } => {
                provider_response.tool_calls.push(NativeToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: input.clone(),
                });
            }
        }
    }

    provider_response.stop_reason = match response_body.stop_reason.as_deref() {
        Some("end_turn") => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        _ => StopReason::Unknown,
    };

    if let Some(usage) = response_body.usage {
        provider_response.usage = Some(TokenUsage {
            input_tokens: usage.input_tokens.unwrap_or(0),
            output_tokens: usage.output_tokens.unwrap_or(0),
            cache_read_tokens: usage.cache_read_input_tokens,
        });
    }

    Ok(provider_response)
}
```

**Step 4: Enable native tools flag**

```rust
fn supports_native_tools(&self) -> bool { true }
```

**Step 5: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_tool_serialization() {
        let tool = AnthropicTool {
            name: "search".into(),
            description: "Search the web".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }),
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["name"], "search");
        assert!(json["input_schema"]["properties"]["query"].is_object());
    }

    #[test]
    fn test_parse_tool_use_response() {
        let json = r#"{
            "content": [
                {"type": "text", "text": "Let me search for that."},
                {"type": "tool_use", "id": "toolu_123", "name": "search", "input": {"query": "rust"}}
            ],
            "stop_reason": "tool_use"
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 2);
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
    }

    #[test]
    fn test_parse_text_only_response() {
        let json = r#"{
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn"
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 1);
    }
}
```

**Step 6: Run tests**

Run: `cargo test -p alephcore --lib providers::anthropic`
Expected: All tests pass.

**Step 7: Commit**

```bash
git add core/src/providers/anthropic/ core/src/providers/protocols/anthropic.rs
git commit -m "anthropic: implement native tool_use in request and response"
```

---

### Task 4: OpenAI Protocol — Native Function Calling

**Files:**
- Modify: `core/src/providers/openai/types.rs`
- Modify: `core/src/providers/protocols/openai.rs`
- Test: `cargo test -p alephcore --lib providers::openai`

**Context:** OpenAI API uses `tools` in requests and `tool_calls` in responses. This covers all OpenAI-compatible providers (DeepSeek, Moonshot, Doubao, SiliconFlow, Groq, etc.).

**Step 1: Add tool types to `openai/types.rs`**

After existing types (line ~76), add:

```rust
/// Tool definition for OpenAI API
#[derive(Debug, Clone, Serialize)]
pub struct OpenAiTool {
    #[serde(rename = "type")]
    pub tool_type: String, // always "function"
    pub function: OpenAiFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAiFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Tool call in response
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    pub function: OpenAiFunctionCall,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiFunctionCall {
    pub name: String,
    pub arguments: String, // JSON string, needs parsing
}
```

Update `ResponseMessage`:
```rust
pub struct ResponseMessage {
    pub content: Option<String>, // nullable when tool_calls present
    pub tool_calls: Option<Vec<OpenAiToolCall>>,
}
```

Update `Choice`:
```rust
pub struct Choice {
    pub message: ResponseMessage,
    pub finish_reason: Option<String>,
}
```

**Step 2: Update `build_request()` in `openai.rs`**

In `build_request()` (line ~251), add tools to the JSON body:

```rust
if let Some(tool_defs) = &payload.tools {
    let tools: Vec<OpenAiTool> = tool_defs.iter().map(|td| OpenAiTool {
        tool_type: "function".into(),
        function: OpenAiFunction {
            name: td.name.clone(),
            description: td.description.clone(),
            parameters: td.parameters.clone(),
        },
    }).collect();
    body["tools"] = serde_json::to_value(&tools)?;
}
```

**Step 3: Update `parse_response()` to extract tool_calls**

```rust
async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse> {
    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        // ... existing error handling ...
    }

    let response_body: ChatCompletionResponse = serde_json::from_str(&body)?;
    let choice = &response_body.choices[0];

    let mut provider_response = ProviderResponse::default();

    // Extract text
    if let Some(ref content) = choice.message.content {
        if !content.is_empty() {
            provider_response.text = Some(content.clone());
        }
    }

    // Extract tool calls
    if let Some(ref tool_calls) = choice.message.tool_calls {
        for tc in tool_calls {
            let arguments: Value = serde_json::from_str(&tc.function.arguments)
                .unwrap_or(Value::Object(Default::default()));
            provider_response.tool_calls.push(NativeToolCall {
                id: tc.id.clone(),
                name: tc.function.name.clone(),
                arguments,
            });
        }
    }

    provider_response.stop_reason = match choice.finish_reason.as_deref() {
        Some("stop") => StopReason::EndTurn,
        Some("tool_calls") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        _ => StopReason::Unknown,
    };

    Ok(provider_response)
}
```

**Step 4: Enable native tools flag**

```rust
fn supports_native_tools(&self) -> bool { true }
```

**Step 5: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_tool_serialization() {
        let tool = OpenAiTool {
            tool_type: "function".into(),
            function: OpenAiFunction {
                name: "search".into(),
                description: "Search the web".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "search");
    }

    #[test]
    fn test_parse_tool_calls_response() {
        let json = r#"{
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {"name": "search", "arguments": "{\"query\":\"test\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;
        let resp: ChatCompletionResponse = serde_json::from_str(json).unwrap();
        assert!(resp.choices[0].message.tool_calls.is_some());
    }

    #[test]
    fn test_parse_text_response() {
        let json = r#"{
            "choices": [{
                "message": {"content": "Hello!", "tool_calls": null},
                "finish_reason": "stop"
            }]
        }"#;
        let resp: ChatCompletionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.choices[0].message.content.as_deref(), Some("Hello!"));
    }
}
```

**Step 6: Run tests**

Run: `cargo test -p alephcore --lib providers::openai`
Expected: All tests pass.

**Step 7: Commit**

```bash
git add core/src/providers/openai/ core/src/providers/protocols/openai.rs
git commit -m "openai: implement native function calling in request and response"
```

---

### Task 5: Gemini Protocol — Native Function Declarations

**Files:**
- Modify: `core/src/providers/protocols/gemini.rs`
- Test: `cargo test -p alephcore --lib providers::protocols::gemini`

**Context:** Gemini uses `functionDeclarations` in requests and `functionCall` in response parts.

**Step 1: Add tool types**

In `gemini.rs`, add types (or in a separate gemini types file if one exists):

```rust
#[derive(Debug, Clone, Serialize)]
pub struct GeminiToolConfig {
    #[serde(rename = "functionDeclarations")]
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeminiFunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GeminiFunctionCall {
    pub name: String,
    pub args: Value,
}
```

**Step 2: Update `build_request()` to include tools**

```rust
if let Some(tool_defs) = &payload.tools {
    let declarations: Vec<GeminiFunctionDeclaration> = tool_defs.iter().map(|td| {
        GeminiFunctionDeclaration {
            name: td.name.clone(),
            description: td.description.clone(),
            parameters: td.parameters.clone(),
        }
    }).collect();
    body["tools"] = serde_json::json!([{
        "functionDeclarations": declarations
    }]);
}
```

**Step 3: Update `parse_response()` to extract function calls**

```rust
// In parse_response(), after extracting text from parts:
for part in &candidate.content.parts {
    if let Some(ref text) = part.text {
        provider_response.text = Some(text.clone());
    }
    if let Some(ref fc) = part.function_call {
        provider_response.tool_calls.push(NativeToolCall {
            id: uuid::Uuid::new_v4().to_string(), // Gemini doesn't assign IDs
            name: fc.name.clone(),
            arguments: fc.args.clone(),
        });
    }
}
provider_response.stop_reason = match candidate.finish_reason.as_deref() {
    Some("STOP") => StopReason::EndTurn,
    Some("FUNCTION_CALL") => StopReason::ToolUse,
    Some("MAX_TOKENS") => StopReason::MaxTokens,
    _ => StopReason::Unknown,
};
```

**Step 4: Enable native tools flag**

```rust
fn supports_native_tools(&self) -> bool { true }
```

**Step 5: Write tests, run, commit**

Run: `cargo test -p alephcore --lib providers::protocols::gemini`

```bash
git add core/src/providers/protocols/gemini.rs
git commit -m "gemini: implement native functionDeclarations in request and response"
```

---

### Task 6: Virtual Tools — System Tool Definitions

**Files:**
- Create: `core/src/thinker/virtual_tools.rs`
- Modify: `core/src/thinker/mod.rs` (add mod declaration)
- Test: `cargo test -p alephcore --lib thinker::virtual_tools`

**Context:** When using native tool_use, ALL LLM decisions (including Complete, AskUser, Fail) must go through tool calls. We register "virtual tools" — they're not real tools, they're decision signals.

**Step 1: Create virtual_tools.rs**

```rust
//! Virtual tools for native tool_use mode
//!
//! These are not real tools — they are decision signals registered as tool definitions
//! so the LLM can express Complete/AskUser/Fail decisions through the native tool_use API.

use crate::dispatcher::types::ToolDefinition;
use serde_json::json;

/// Names of virtual tools (used in Thinker to distinguish from real tools)
pub const VIRTUAL_COMPLETE: &str = "__complete";
pub const VIRTUAL_ASK_USER: &str = "__ask_user";
pub const VIRTUAL_FAIL: &str = "__fail";

/// Generate virtual tool definitions for native tool_use mode
pub fn virtual_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            VIRTUAL_COMPLETE,
            "Report that the task is complete. Call this when you have finished the user's request and want to provide a final summary.",
            json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "A concise summary of what was accomplished and the final result"
                    }
                },
                "required": ["summary"]
            }),
            Default::default(),
        ),
        ToolDefinition::new(
            VIRTUAL_ASK_USER,
            "Ask the user a question when you need clarification or input before proceeding.",
            json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question to ask the user"
                    },
                    "options": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional list of suggested answer choices"
                    }
                },
                "required": ["question"]
            }),
            Default::default(),
        ),
        ToolDefinition::new(
            VIRTUAL_FAIL,
            "Report that the task cannot be completed. Call this when you encounter an unrecoverable error.",
            json!({
                "type": "object",
                "properties": {
                    "reason": {
                        "type": "string",
                        "description": "Explanation of why the task failed"
                    }
                },
                "required": ["reason"]
            }),
            Default::default(),
        ),
    ]
}

/// Check if a tool name is a virtual tool
pub fn is_virtual_tool(name: &str) -> bool {
    matches!(name, VIRTUAL_COMPLETE | VIRTUAL_ASK_USER | VIRTUAL_FAIL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_virtual_tool_definitions() {
        let defs = virtual_tool_definitions();
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].name, VIRTUAL_COMPLETE);
        assert_eq!(defs[1].name, VIRTUAL_ASK_USER);
        assert_eq!(defs[2].name, VIRTUAL_FAIL);
    }

    #[test]
    fn test_is_virtual_tool() {
        assert!(is_virtual_tool(VIRTUAL_COMPLETE));
        assert!(is_virtual_tool(VIRTUAL_ASK_USER));
        assert!(!is_virtual_tool("search"));
        assert!(!is_virtual_tool("pdf_generate"));
    }

    #[test]
    fn test_virtual_tools_have_valid_schemas() {
        for def in virtual_tool_definitions() {
            assert!(def.parameters.is_object());
            let props = &def.parameters["properties"];
            assert!(props.is_object());
            let required = &def.parameters["required"];
            assert!(required.is_array());
        }
    }
}
```

**Step 2: Add module declaration**

In `core/src/thinker/mod.rs`, add:
```rust
pub mod virtual_tools;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib thinker::virtual_tools`
Expected: All 3 tests pass.

**Step 4: Commit**

```bash
git add core/src/thinker/virtual_tools.rs core/src/thinker/mod.rs
git commit -m "thinker: add virtual tools for native tool_use decision signaling"
```

---

### Task 7: Thinker — Native Tool Use Integration

**Files:**
- Modify: `core/src/thinker/mod.rs`
- Modify: `core/src/thinker/layers/tools.rs`
- Modify: `core/src/thinker/layers/response_format.rs`
- Test: `cargo test -p alephcore --lib thinker`

**Context:** The Thinker is the orchestrator. It needs to: (1) pass tool definitions through RequestPayload when the provider supports native tools, (2) map native tool calls directly to Decision variants bypassing DecisionParser, (3) conditionally skip ToolsLayer and ResponseFormatLayer prompt injection.

**Step 1: Update Thinker to pass tools via RequestPayload**

In `think_with_level()` (around line 431), after tool filtering:

```rust
// Determine if provider supports native tools
let native_tools = provider.supports_native_tools();

// Build tool definitions for native mode
let all_tool_defs: Vec<ToolDefinition> = if native_tools {
    let mut defs: Vec<ToolDefinition> = filtered_tools.iter()
        .map(|t| t.definition())
        .collect();
    // Add virtual tools
    defs.extend(virtual_tools::virtual_tool_definitions());
    defs
} else {
    vec![]
};
```

Then when calling `provider.process_with_overrides()`, pass tools:

```rust
let payload = RequestPayload::new(&full_prompt)
    .with_system(Some(&system_prompt))
    .with_think_level(Some(think_level))
    .with_temperature(temperature)
    .with_max_tokens(max_tokens)
    .with_tools(if native_tools { Some(&all_tool_defs) } else { None });
```

Note: This requires changing how the provider is called — currently it uses `provider.process_with_overrides()` which doesn't take a `RequestPayload`. We need to add a new method or modify the existing one. The cleanest approach is to add `process_with_payload()` to `AiProvider`:

```rust
// In AiProvider trait:
fn process_with_payload(
    &self,
    payload: RequestPayload<'_>,
) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + '_>> {
    // Default implementation delegates to process_with_overrides for backward compat
    let input = payload.input.to_string();
    let system = payload.system_prompt.map(|s| s.to_string());
    let think = payload.think_level.unwrap_or(ThinkLevel::Off);
    let temp = payload.temperature;
    let max_tok = payload.max_tokens;
    Box::pin(async move {
        self.process_with_overrides(&input, system.as_deref(), think, temp, max_tok).await
    })
}
```

HttpProvider overrides this to pass tools through to the adapter.

**Step 2: Map native tool calls to Decision**

In `think_with_level()`, after receiving `ProviderResponse`:

```rust
let response: ProviderResponse = provider.process_with_payload(payload).await?;

let thinking = if response.has_tool_calls() {
    // Native tool_use path — direct mapping
    let tc = &response.tool_calls[0];
    let decision = match tc.name.as_str() {
        virtual_tools::VIRTUAL_COMPLETE => Decision::Complete {
            summary: tc.arguments.get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        },
        virtual_tools::VIRTUAL_ASK_USER => Decision::AskUser {
            question: tc.arguments.get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            options: tc.arguments.get("options")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()),
        },
        virtual_tools::VIRTUAL_FAIL => Decision::Fail {
            reason: tc.arguments.get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown failure")
                .to_string(),
        },
        _ => Decision::UseTool {
            tool_name: tc.name.clone(),
            arguments: tc.arguments.clone(),
        },
    };

    let reasoning = response.thinking
        .or(response.text)
        .unwrap_or_default();

    Thinking {
        reasoning: Some(reasoning),
        decision,
        structured: None,
        tokens_used: response.usage.map(|u| (u.input_tokens + u.output_tokens) as usize),
    }
} else if let Some(ref text) = response.text {
    // Fallback: JSON-in-text parsing
    self.decision_parser.parse(text)?
} else {
    return Err(AlephError::Other {
        message: "Empty LLM response (no text and no tool calls)".into(),
        suggestion: None,
    });
};
```

**Step 3: Conditionally skip prompt layers**

In `layers/tools.rs`, `ToolsLayer::inject()`:
```rust
pub fn inject(prompt: &mut String, tools: &[UnifiedTool], config: &PromptConfig) {
    // Skip tool schema injection if native tools are enabled
    if config.native_tools_enabled {
        return;
    }
    // ... existing implementation ...
}
```

In `layers/response_format.rs`, `ResponseFormatLayer::inject()`:
```rust
pub fn inject(prompt: &mut String, config: &PromptConfig) {
    // Skip JSON format instruction if native tools are enabled
    if config.native_tools_enabled {
        return;
    }
    // ... existing implementation ...
}
```

Add `native_tools_enabled: bool` to `PromptConfig` (or the struct used for layer configuration). Set this based on `provider.supports_native_tools()`.

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib thinker`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add core/src/thinker/ core/src/providers/mod.rs core/src/providers/http_provider.rs
git commit -m "thinker: integrate native tool_use with dual-path decision mapping"
```

---

### Task 8: Message Builder — Tool Result Passback

**Files:**
- Modify: `core/src/thinker/prompt_builder/messages.rs`
- Modify: `core/src/agent_loop/state.rs` (add tool_call_id to LoopStep/StepSummary)
- Modify: `core/src/agent_loop/agent_loop.rs` (record tool_call_id)
- Test: `cargo test -p alephcore --lib`

**Context:** When the LLM calls a tool via native tool_use, the result must be sent back in the provider's native format (Anthropic: `tool_result` block, OpenAI: `tool` role message). The agent loop must track the `tool_call_id` so the message builder can use it.

**Step 1: Add tool_call_id to LoopStep and StepSummary**

In `state.rs`, `LoopStep`:
```rust
pub struct LoopStep {
    // ... existing fields ...
    /// Tool call ID from native tool_use (for result passback)
    pub tool_call_id: Option<String>,
}
```

In `StepSummary`:
```rust
pub struct StepSummary {
    // ... existing fields ...
    /// Tool call ID from native tool_use
    pub tool_call_id: Option<String>,
}
```

Update `StepSummary::from()` to pass through:
```rust
tool_call_id: step.tool_call_id.clone(),
```

**Step 2: Record tool_call_id in agent_loop.rs**

When creating a `LoopStep` after tool execution, extract the tool_call_id from the Thinking decision:

In the `Decision::UseTool` handler, store the NativeToolCall's id (this requires passing it through the Thinking struct or a side channel).

The simplest approach: add `tool_call_id: Option<String>` to the `Thinking` struct:

```rust
pub struct Thinking {
    pub reasoning: Option<String>,
    pub decision: Decision,
    pub structured: Option<StructuredThinking>,
    pub tokens_used: Option<usize>,
    /// Tool call ID from native tool_use (for result passback)
    pub tool_call_id: Option<String>,
}
```

Set it in Thinker when mapping native tool calls:
```rust
Thinking {
    // ...
    tool_call_id: Some(tc.id.clone()),
}
```

Then in agent_loop.rs when creating LoopStep:
```rust
let step = LoopStep {
    // ... existing fields ...
    tool_call_id: thinking.tool_call_id.clone(),
};
```

**Step 3: Update message builder for native tool results**

In `messages.rs`, `build_messages()`, where tool results are formatted (line ~142-153):

```rust
// For steps with tool_call_id (native tool_use), use structured format
if let Some(ref tool_call_id) = step.tool_call_id {
    // Native format: the message builder will produce a structured message
    // that each ProtocolAdapter can understand
    messages.push(Message::native_tool_result(
        tool_call_id,
        &step.result_output,
    ));
} else {
    // Legacy format: text-based
    messages.push(Message::tool_result(
        &step.action_type,
        &step.result_output,
    ));
}
```

Add `Message::native_tool_result()` helper:
```rust
pub fn native_tool_result(tool_call_id: &str, result: &str) -> Self {
    // Uses a special role/format that ProtocolAdapters recognize
    Message {
        role: "tool".to_string(),
        content: MessageContent::Text {
            content: result.to_string(),
        },
        tool_call_id: Some(tool_call_id.to_string()),
    }
}
```

This requires adding `tool_call_id: Option<String>` to the Message struct (in openai/types.rs or wherever Message is defined).

Each `ProtocolAdapter::build_request()` then handles this field:
- **Anthropic**: Converts to `{"type": "tool_result", "tool_use_id": id, "content": result}`
- **OpenAI**: Converts to `{"role": "tool", "tool_call_id": id, "content": result}`
- **Gemini**: Converts to `{"role": "function", "parts": [{"functionResponse": {...}}]}`

**Step 4: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add core/src/agent_loop/ core/src/thinker/prompt_builder/
git commit -m "agent_loop: track tool_call_id and format native tool results in messages"
```

---

### Task 9: Integration Verification & Cleanup

**Files:**
- All modified files
- Test: Full test suite

**Context:** Final verification that the complete pipeline works end-to-end. Run all tests, fix any remaining compilation issues, verify fallback path still works.

**Step 1: Full compilation check**

Run: `cargo check -p alephcore`
Fix any remaining type errors.

**Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass.

**Step 3: Verify fallback path**

Ensure that when `supports_native_tools()` returns `false`, the system uses the existing JSON-in-text path:
- ToolsLayer still injects schemas into system prompt
- ResponseFormatLayer still injects JSON format instructions
- DecisionParser still parses JSON from text
- Tool results still use text format

**Step 4: Build release**

Run: `cargo build -p alephcore --release`
Expected: Builds without warnings.

**Step 5: Final commit**

```bash
git add -A
git commit -m "native-tool-use: integration verification and cleanup"
```

---

## Task Dependency Graph

```
Task 1 (Core Types)
  ↓
Task 2 (Trait Migration) ─── depends on Task 1
  ↓
Task 3 (Anthropic) ┐
Task 4 (OpenAI)    ├── depend on Task 2, independent of each other
Task 5 (Gemini)    ┘
  ↓ (all three)
Task 6 (Virtual Tools) ─── independent, but needed by Task 7
  ↓
Task 7 (Thinker Integration) ─── depends on Tasks 3-6
  ↓
Task 8 (Message Builder) ─── depends on Task 7
  ↓
Task 9 (Integration) ─── depends on all above
```

## Risk Mitigation

- **Compile-time safety**: Rust's type system ensures all callers are updated when return types change
- **Incremental verification**: Each task includes `cargo check` and `cargo test` gates
- **Fallback preserved**: JSON-in-text path remains functional for unsupported providers
- **No behavior change for fallback providers**: ChatGPT, Ollama behavior identical to pre-migration
