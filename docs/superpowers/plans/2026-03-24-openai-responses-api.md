# OpenAI Responses API Protocol Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a standard OpenAI Responses API protocol (`/v1/responses`) so users can configure OpenRouter and other relay providers that support the Responses API natively.

**Architecture:** Extract shared Responses API logic (types, message conversion, SSE parsing) from the existing Codex protocol into a reusable module. Create a new `OpenAiResponsesProtocol` that implements `ProtocolAdapter` using standard `/v1/responses` endpoint with Bearer auth. Codex protocol is refactored to reuse the shared layer while keeping its private endpoint and special headers.

**Tech Stack:** Rust, reqwest, serde, async-trait, futures (SSE streaming)

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `src/providers/responses/mod.rs` | Module definition + re-exports |
| Create | `src/providers/responses/types.rs` | Shared Responses API types (InputItem, OutputItem, StreamEvent, etc.) |
| Create | `src/providers/responses/shared.rs` | Shared logic: convert_messages, build_reasoning, build_tools, parse_sse, extract_text/tools |
| Create | `src/providers/protocols/openai_responses.rs` | New `OpenAiResponsesProtocol` implementing `ProtocolAdapter` |
| Modify | `src/providers/codex/types.rs` | Remove types migrated to `responses/types.rs`, keep Codex-only types (ChatRequirements, ProofOfWork) |
| Modify | `src/providers/protocols/codex.rs` | Refactor to use `responses::shared` instead of inline logic |
| Modify | `src/providers/protocols/mod.rs` | Export new `OpenAiResponsesProtocol` |
| Modify | `src/providers/protocols/registry.rs` | Register `"openai-responses"` as builtin protocol |
| Modify | `src/providers/mod.rs` | Add `pub mod responses;` |
| Modify | `src/providers/presets.rs` | Update `"openrouter"` preset to `protocol: "openai-responses"`, update `valid_protocols` test |

---

### Task 1: Create shared Responses API types

**Files:**
- Create: `src/providers/responses/mod.rs`
- Create: `src/providers/responses/types.rs`
- Modify: `src/providers/mod.rs`

These types are currently in `src/providers/codex/types.rs`. We move the wire-format types (InputItem, OutputItem, StreamEvent, etc.) to the shared location. Codex-only types (ChatRequirements, ProofOfWork, TextConfig) stay.

- [ ] **Step 1: Create `responses/types.rs`**

Copy the following types from `codex/types.rs` to the new file, adjusting doc comments to be protocol-agnostic:

- `ResponsesRequest` — three field type changes needed:
  - `store: bool` → `store: Option<bool>` with `#[serde(skip_serializing_if = "Option::is_none")]` (Codex sets `Some(false)`, standard API omits)
  - `text: Option<TextConfig>` → `text: Option<serde_json::Value>` (Codex passes `serde_json::to_value(TextConfig{...})`, standard API passes `None`)
  - Add `#[serde(skip_serializing_if = "Option::is_none")]` to `store` field (already present on `include`)
- `FunctionToolDef`
- `InputItem` (Message, FunctionCall, FunctionCallOutput)
- `MessageContent` (Text, Multimodal) + `as_text()` method
- `InputContentPart` (InputText, InputImage)
- `ReasoningConfig`
- `ResponseResource`
- `OutputItem` (Message, Reasoning, FunctionCall)
- `ContentPart`
- `UsageInfo`
- `ResponseError`
- `StreamEvent` (all variants)

- [ ] **Step 2: Create `responses/mod.rs`**

```rust
//! Shared types and logic for the OpenAI Responses API wire format.
//!
//! Used by both the standard OpenAI Responses protocol (`/v1/responses`)
//! and the Codex protocol (`chatgpt.com/backend-api/codex/responses`).

pub mod types;
pub mod shared;

pub use types::*;
pub use shared::*;
```

- [ ] **Step 3: Add `pub mod responses;` to `providers/mod.rs`**

Add `pub mod responses;` after the existing `pub mod codex;` line (line 56).

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS (no code references the new module yet)

- [ ] **Step 5: Commit**

```
responses: add shared Responses API types module
```

---

### Task 2: Create shared Responses API logic

**Files:**
- Create: `src/providers/responses/shared.rs`

Extract reusable logic from `codex.rs` into standalone functions that both `OpenAiResponsesProtocol` and `CodexProtocol` can call.

- [ ] **Step 1: Write tests for shared functions**

Add tests in `shared.rs` for:
- `convert_messages()` — user text, multimodal, assistant+tool_call, tool_result (port from codex.rs tests)
- `build_reasoning()` — Low/Medium/High/None mapping
- `build_tools()` — tool definition conversion with schema cleanup
- `extract_text()` — from ResponseResource
- `extract_tool_calls()` — from ResponseResource
- `parse_sse_data()` — TextDelta, Completed, Failed, [DONE]

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib responses::shared`
Expected: FAIL (functions not defined)

- [ ] **Step 3: Implement shared functions**

```rust
//! Shared logic for Responses API protocols

use crate::error::{AlephError, Result};
use crate::providers::adapter::{NativeToolCall, StopReason, TokenUsage, ToolChoice};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::protocols::openai::{desanitize_tool_name_pub, sanitize_tool_name_pub};
use crate::providers::responses::types::*;
use crate::agents::thinking::ThinkLevel;
use crate::tools::definition::ToolDefinition;
use tracing::debug;

/// Convert UnifiedMessages to Responses API InputItems
pub fn convert_messages(messages: &[UnifiedMessage]) -> Vec<InputItem> { ... }

/// Map ThinkLevel to Responses API reasoning config
pub fn build_reasoning(think_level: Option<ThinkLevel>) -> Option<ReasoningConfig> { ... }

/// Convert ToolDefinitions to Responses API FunctionToolDefs
pub fn build_tools(tools: Option<&[ToolDefinition]>) -> Option<Vec<FunctionToolDef>> { ... }

/// Map ToolChoice to Responses API string
pub fn map_tool_choice(choice: Option<&ToolChoice>) -> Option<String> { ... }

/// Extract text from completed ResponseResource
pub fn extract_text(response: &ResponseResource) -> Option<String> { ... }

/// Extract tool calls from completed ResponseResource
pub fn extract_tool_calls(response: &ResponseResource) -> Vec<NativeToolCall> { ... }

/// Parse SSE data line into StreamEvent
pub fn parse_sse_data(data: &str) -> Option<StreamEvent> { ... }

/// Parse full SSE response body into ProviderResponse.
/// Used by non-streaming `parse_response()` in both protocols.
pub fn parse_sse_body(body: &str) -> Result<(String, Vec<NativeToolCall>, bool, Option<TokenUsage>)> { ... }

/// Build SSE streaming parser as BoxStream.
/// Used by `parse_stream()` in both protocols.
pub fn build_sse_stream(response: reqwest::Response) -> Result<BoxStream<'static, Result<String>>> { ... }
```

The implementations are extractions from `codex.rs`. Key complexity notes:

- `convert_messages()` — straightforward extraction from codex.rs lines 68-173
- `build_reasoning()` — simple extraction from codex.rs lines 48-65
- `extract_text()` / `extract_tool_calls()` — straightforward from codex.rs lines 242-282
- `parse_sse_data()` — simple from codex.rs lines 285-290
- `parse_sse_body()` — **most complex** (~100 lines), extracted from codex.rs `parse_response()` lines 370-478. Must handle:
  - Text delta accumulation from `StreamEvent::TextDelta`
  - Function call metadata tracking via `fc_meta: HashMap<String, (String, String)>` (item_id → call_id, name)
  - Function call argument delta accumulation via `fc_args: HashMap<String, String>`
  - `OutputItemAdded` / `OutputItemDone` for FunctionCall metadata
  - `FunctionCallArgumentsDelta` / `FunctionCallArgumentsDone` for argument streaming
  - `Completed` event: extract usage, detect `status == "incomplete"`, merge accumulated args with completed response
  - `Failed` event: return error immediately
- `build_sse_stream()` — extraction from codex.rs `parse_stream()` lines 508-575, uses Arc<Mutex<String>> line buffer

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib responses::shared`
Expected: PASS

- [ ] **Step 5: Commit**

```
responses: add shared logic for Responses API protocols
```

---

### Task 3: Refactor Codex protocol to use shared layer

**Files:**
- Modify: `src/providers/codex/types.rs`
- Modify: `src/providers/protocols/codex.rs`

- [ ] **Step 1: Update `codex/types.rs`**

Replace all migrated types with re-exports from `responses::types`:

```rust
//! Codex-specific types
//!
//! Wire-format types are in `crate::providers::responses::types`.
//! This module only contains Codex-private types.

// Re-export shared Responses API types for backward compatibility
pub use crate::providers::responses::types::*;

// ─── Codex-only Types ───────────────────────────────────────────

/// Text output verbosity configuration (Codex mode only)
#[derive(Debug, serde::Serialize)]
pub struct TextConfig {
    pub verbosity: String,
}

/// Chat requirements response (security tokens)
#[derive(Debug, serde::Deserialize)]
pub struct ChatRequirements { ... }

/// Proof-of-work challenge
#[derive(Debug, serde::Deserialize)]
pub struct ProofOfWork { ... }
```

- [ ] **Step 2: Refactor `codex.rs` to use shared functions**

Replace inline implementations with calls to `crate::providers::responses::shared::*`:
- `Self::convert_messages(...)` → `responses::shared::convert_messages(...)`
- `Self::build_reasoning(...)` → `responses::shared::build_reasoning(payload.think_level)`
- `Self::extract_text(...)` → `responses::shared::extract_text(...)`
- `Self::extract_tool_calls(...)` → `responses::shared::extract_tool_calls(...)`
- `Self::parse_sse_data(...)` → `responses::shared::parse_sse_data(...)`
- Tool conversion in `build_responses_request` → `responses::shared::build_tools(...)`
- SSE body parsing in `parse_response` → `responses::shared::parse_sse_body(...)`
- SSE stream parsing in `parse_stream` → `responses::shared::build_sse_stream(...)`

Keep Codex-specific logic in place:
- `build_endpoint()` (private endpoint `/backend-api/codex/responses`)
- `build_request()` (JWT headers, `chatgpt-account-id`, `OpenAI-Beta`, `originator`)
- `build_responses_request()` (Codex-only fields: `text`, `include`, `store: Some(false)`)
  - Update `store: false` → `store: Some(false)`
  - Update `text: Some(TextConfig{...})` → `text: Some(serde_json::to_value(TextConfig{...}).unwrap())`

- [ ] **Step 3: Run ALL existing Codex tests**

Run: `cargo test -p alephcore --lib codex`
Expected: ALL PASS (behavior unchanged, just code reuse)

- [ ] **Step 4: Run full test suite to catch regressions**

Run: `cargo test -p alephcore --lib`
Expected: No new failures

- [ ] **Step 5: Commit**

```
codex: refactor to use shared Responses API layer
```

---

### Task 4: Implement OpenAiResponsesProtocol

**Files:**
- Create: `src/providers/protocols/openai_responses.rs`
- Modify: `src/providers/protocols/mod.rs`

- [ ] **Step 1: Write tests for the new protocol**

Test in `openai_responses.rs`:
- `test_build_endpoint_default` — default `https://api.openai.com/v1/responses`
- `test_build_endpoint_custom_base` — custom base_url normalization
- `test_build_endpoint_openrouter` — `https://openrouter.ai/api/v1/responses`
- `test_build_request_basic` — request body has correct structure (input, model, stream, no Codex fields)
- `test_build_request_with_tools` — tools serialized correctly
- `test_build_request_with_reasoning` — reasoning config present
- `test_name` — returns `"openai-responses"`

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib protocols::openai_responses`
Expected: FAIL

- [ ] **Step 3: Implement `OpenAiResponsesProtocol`**

```rust
//! OpenAI Responses API protocol adapter
//!
//! Standard `/v1/responses` endpoint for OpenAI and compatible relay providers
//! (OpenRouter, etc.). Uses the same wire format as Codex but with standard
//! Bearer auth and public endpoint.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, ProviderResponse, RequestPayload, StopReason};
use crate::providers::responses::{self, types::ResponsesRequest};
use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::Client;
use tracing::debug;

pub struct OpenAiResponsesProtocol {
    client: Client,
}

impl OpenAiResponsesProtocol {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_endpoint(config: &ProviderConfig) -> String {
        let base_url = config.base_url.as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://api.openai.com".to_string());

        let base = base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        format!("{}/v1/responses", base)
    }

    fn build_responses_request(payload: &RequestPayload, model: &str) -> ResponsesRequest {
        ResponsesRequest {
            model: model.to_string(),
            input: responses::shared::convert_messages(payload.messages),
            instructions: payload.system_prompt.map(|s| s.to_string()),
            stream: true,
            store: None,  // Not set for standard API (unlike Codex's false)
            reasoning: responses::shared::build_reasoning(payload.think_level),
            tools: responses::shared::build_tools(payload.tools),
            tool_choice: responses::shared::map_tool_choice(payload.tool_choice.as_ref()),
            parallel_tool_calls: Some(true),
            text: None,           // No Codex TextConfig
            max_output_tokens: payload.max_tokens,
            include: None,        // No Codex encrypted_content
        }
    }
}

#[async_trait]
impl ProtocolAdapter for OpenAiResponsesProtocol {
    fn supports_native_tools(&self) -> bool { true }
    fn supports_strict_schema(&self) -> bool { true }
    fn name(&self) -> &'static str { "openai-responses" }

    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        _is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let request = Self::build_responses_request(payload, config.default_model());
        let api_key = config.api_key.as_ref().ok_or_else(|| {
            AlephError::invalid_config("API key not set for OpenAI Responses provider")
        })?;

        let builder = self.client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&request);

        Ok(builder)
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse> {
        // Standard HTTP error handling (similar to OpenAI protocol)
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(match status.as_u16() {
                401 => AlephError::provider("Authentication failed — check API key"),
                429 => AlephError::provider("Rate limit exceeded — try again later"),
                _ => AlephError::provider(format!("API error ({}): {}", status, error_text)),
            });
        }

        let body = response.text().await
            .map_err(|e| AlephError::provider(format!("Failed to read response: {}", e)))?;

        // Reuse shared SSE body parser
        let (text, tool_calls, is_incomplete, usage) = responses::shared::parse_sse_body(&body)?;

        let stop_reason = if is_incomplete {
            StopReason::MaxTokens
        } else if !tool_calls.is_empty() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };

        Ok(ProviderResponse {
            text: if text.is_empty() { None } else { Some(text) },
            tool_calls,
            stop_reason,
            usage,
            ..Default::default()
        })
    }

    async fn parse_stream(&self, response: reqwest::Response) -> Result<BoxStream<'static, Result<String>>> {
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!("API error ({}): {}", status, error_text)));
        }
        responses::shared::build_sse_stream(response)
    }
}
```

- [ ] **Step 4: Add export to `protocols/mod.rs`**

Add:
```rust
pub mod openai_responses;
pub use openai_responses::OpenAiResponsesProtocol;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib protocols::openai_responses`
Expected: PASS

- [ ] **Step 6: Commit**

```
responses: add OpenAiResponsesProtocol for /v1/responses endpoint
```

---

### Task 5: Register protocol and add preset

**Files:**
- Modify: `src/providers/protocols/registry.rs`
- Modify: `src/providers/presets.rs`

- [ ] **Step 1: Write tests**

In `registry.rs` tests:
```rust
#[test]
fn test_openai_responses_protocol_registered() {
    let registry = ProtocolRegistry::new();
    registry.register_builtin();
    assert!(registry.get("openai-responses").is_some());
}
```

In `presets.rs` tests (or at module bottom):
```rust
#[test]
fn test_openrouter_preset() {
    let preset = get_preset("openrouter");
    assert!(preset.is_some());
    let p = preset.unwrap();
    assert_eq!(p.protocol, "openai-responses");
}
```

- [ ] **Step 2: Register `openai-responses` in `registry.rs`**

In `register_builtin()`, after the codex/chatgpt entries, add:

```rust
use super::OpenAiResponsesProtocol;

builtin.insert(
    "openai-responses".to_string(),
    (|client| Arc::new(OpenAiResponsesProtocol::new(client)) as Arc<dyn ProtocolAdapter>)
        as ProtocolFactory,
);
```

- [ ] **Step 3: Update `openrouter` preset in `presets.rs`**

Change the existing `openrouter` preset (line 271-279) from:
```rust
base_url: "https://openrouter.ai/api/v1",
protocol: "openai",
color: "#7c3aed",
default_model: "anthropic/claude-sonnet-4-5",
```
to:
```rust
base_url: "https://openrouter.ai/api",
protocol: "openai-responses",
color: "#6467f2",
default_model: "openai/gpt-4o",
```

Note: This changes the default protocol for OpenRouter users. Users who explicitly set `protocol: "openai"` in their config are unaffected (preset only applies when protocol is not specified).

- [ ] **Step 4: Update `test_presets_have_valid_protocol` test in `presets.rs`**

Change line 413:
```rust
let valid_protocols = ["openai", "anthropic", "gemini", "codex"];
```
to:
```rust
let valid_protocols = ["openai", "openai-responses", "anthropic", "gemini", "codex"];
```

- [ ] **Step 5: Update `registry.rs` import**

Add `OpenAiResponsesProtocol` to the import line at the top of `registry.rs`:

```rust
use crate::providers::protocols::{
    AnthropicProtocol, CodexProtocol, GeminiProtocol, OpenAiProtocol, OpenAiResponsesProtocol,
};
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib registry && cargo test -p alephcore --lib presets`
Expected: PASS

- [ ] **Step 7: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: No new failures

- [ ] **Step 8: Commit**

```
responses: register openai-responses protocol and add openrouter preset
```

---

### Task 6: Integration test — factory creation

**Files:**
- Modify: `src/providers/mod.rs` (add test)

- [ ] **Step 1: Add factory integration test**

In `mod.rs` tests section:

```rust
#[test]
fn test_create_openai_responses_provider() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.protocol = Some("openai-responses".to_string());
    config.base_url = Some("https://openrouter.ai/api".to_string());

    let provider = create_provider("openrouter", config);
    assert!(provider.is_ok(), "Should create openai-responses provider: {:?}", provider.err());
    assert_eq!(provider.unwrap().name(), "openrouter");
}

#[test]
fn test_create_openrouter_via_preset() {
    let config = ProviderConfig::test_config("openai/gpt-4o");
    let provider = create_provider("openrouter", config);
    assert!(provider.is_ok());
    assert_eq!(provider.unwrap().name(), "openrouter");
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test -p alephcore --lib providers::tests`
Expected: PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -50`
Expected: No new warnings

- [ ] **Step 4: Commit**

```
responses: add integration tests for OpenAI Responses protocol
```

---

### Task 7: Update module documentation

**Files:**
- Modify: `src/providers/mod.rs` (doc comment at top)

- [ ] **Step 1: Update the module-level doc comment**

Add `OpenAI Responses` to the architecture section:

```rust
/// - **OpenAI Responses Protocol**: Handled by `HttpProvider` + `OpenAiResponsesProtocol` adapter
///   - Supports: OpenAI /v1/responses API and compatible relay providers (OpenRouter, etc.)
///   - Configuration: Use presets (e.g., `openrouter`) or set `protocol: "openai-responses"`
```

Also update the `create_provider()` doc:
```rust
/// - `"openai-responses"` - OpenAI Responses API (via HttpProvider), for OpenRouter etc.
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Commit**

```
docs: document OpenAI Responses protocol in provider module
```
