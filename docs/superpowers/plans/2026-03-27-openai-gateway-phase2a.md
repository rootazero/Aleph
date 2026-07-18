# OpenAI Gateway Phase 2A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `/v1/embeddings`, `/v1/responses` (passthrough), and tool calling support to the OpenAI-compatible gateway.

**Architecture:** Three independent capabilities extending Phase 1's `openai_api/` module. Embeddings uses the existing `EmbeddingProvider` trait. Responses creates a new SSE format translator for the Responses API wire format. Tool passthrough adds conversion functions in the existing `passthrough.rs`.

**Tech Stack:** Rust, axum, `EmbeddingProvider::embed_batch()`, `HttpProvider::stream_raw()`, `ToolDefinition`, `ToolChoice`

**Spec:** `docs/superpowers/specs/2026-03-27-openai-gateway-phase2a-design.md`

---

### Task 1: Augment types.rs with Embedding, Responses, and tool_call_id types

**Files:**
- Modify: `src/gateway/openai_api/types.rs`

Add all new types needed by subsequent tasks, plus `tool_call_id` on `ChatMessage`.

- [ ] **Step 1: Add `tool_call_id` to ChatMessage**

Add after the existing `tool_calls` field:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub tool_call_id: Option<String>,
```

- [ ] **Step 2: Add Embedding types**

```rust
// === Embedding types ===

#[derive(Debug, Deserialize)]
pub struct EmbeddingRequest {
    #[serde(default)]
    pub model: Option<String>,
    pub input: EmbeddingInput,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EmbeddingInput {
    Single(String),
    Batch(Vec<String>),
}

#[derive(Debug, Serialize)]
pub struct EmbeddingResponse {
    pub object: String,
    pub data: Vec<EmbeddingData>,
    pub model: String,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct EmbeddingData {
    pub object: String,
    pub index: u32,
    pub embedding: Vec<f32>,
}
```

- [ ] **Step 3: Add Responses types**

```rust
// === Responses API types ===

#[derive(Debug, Deserialize)]
pub struct ResponsesRequest {
    pub model: String,
    pub input: ResponsesInput,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ResponsesInput {
    Text(String),
    Messages(Vec<ResponsesMessage>),
}

#[derive(Debug, Deserialize)]
pub struct ResponsesMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<serde_json::Value>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResponsesResponse {
    pub id: String,
    pub object: String,
    pub created_at: u64,
    pub status: String,
    pub model: String,
    pub output: Vec<serde_json::Value>,  // flexible — message or function_call items
    pub usage: ResponsesUsage,
}

#[derive(Debug, Serialize)]
pub struct ResponsesUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}
```

Note: `ResponsesResponse.output` is `Vec<Value>` for flexibility — it can contain both `message` and `function_call` output items without needing an enum.

- [ ] **Step 4: Fix all existing ChatMessage construction sites**

Every place that constructs `ChatMessage` must add `tool_call_id: None`. Search for `ChatMessage {` in:
- `completions/passthrough.rs` (line ~124)
- `completions/agent.rs` (non-streaming response)
- `types.rs` tests

- [ ] **Step 5: Add tests for new types**

Add tests for `EmbeddingInput` deserialization (single string and batch), `EmbeddingResponse` serialization, `ResponsesRequest` deserialization, `ResponsesInput` variants.

- [ ] **Step 6: Verify**

Run: `cargo test -p alephcore --lib openai_api`
Expected: All tests pass (existing + new).

- [ ] **Step 7: Commit**

```
gateway: add embedding, responses, and tool_call_id types
```

---

### Task 2: Implement /v1/embeddings endpoint

**Files:**
- Create: `src/gateway/openai_api/embeddings.rs`
- Modify: `src/gateway/openai_api/state.rs` — add `embedding_provider` field
- Modify: `src/gateway/openai_api/mod.rs` — add `pub mod embeddings`
- Modify: `src/gateway/openai_api/router.rs` — add route

- [ ] **Step 1: Add embedding_provider to state.rs**

Add field to `OpenAiApiState`:
```rust
pub embedding_provider: Option<Arc<dyn crate::memory::EmbeddingProvider>>,
```

Add import: `use crate::sync_primitives::Arc;` (already present).

- [ ] **Step 2: Create embeddings.rs**

```rust
//! POST /v1/embeddings — embedding generation via EmbeddingProvider.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::auth::{extract_bearer_token, ApiError};
use super::state::OpenAiApiState;
use super::types::{EmbeddingData, EmbeddingInput, EmbeddingRequest, EmbeddingResponse, Usage};

const MAX_INPUTS: usize = 128;
const MAX_INPUT_CHARS: usize = 8192;

pub async fn handle(
    State(state): State<Arc<OpenAiApiState>>,
    headers: HeaderMap,
    Json(req): Json<EmbeddingRequest>,
) -> Result<Response, ApiError> {
    // Auth
    let auth_header = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let token = extract_bearer_token(auth_header)
        .ok_or_else(|| ApiError::Unauthorized("Missing or invalid Authorization header".into()))?;
    if let Some(ref expected) = state.api_token {
        if token != expected.as_str() {
            return Err(ApiError::Unauthorized("Invalid API key".into()));
        }
    }

    // Get provider
    let provider = state.embedding_provider.as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Embedding provider not configured".into()))?;

    // Normalize input
    let texts: Vec<String> = match req.input {
        EmbeddingInput::Single(s) => vec![s],
        EmbeddingInput::Batch(v) => v,
    };

    // Validate
    if texts.len() > MAX_INPUTS {
        return Err(ApiError::BadRequest(format!("Too many inputs (max {})", MAX_INPUTS)));
    }
    for (i, text) in texts.iter().enumerate() {
        if text.len() > MAX_INPUT_CHARS {
            return Err(ApiError::BadRequest(
                format!("Input {} exceeds max length ({} chars)", i, MAX_INPUT_CHARS),
            ));
        }
    }

    // Embed
    let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let embeddings = provider.embed_batch(&text_refs).await
        .map_err(|e| ApiError::BadGateway(format!("Embedding error: {e}")))?;

    // Build response
    let data: Vec<EmbeddingData> = embeddings
        .into_iter()
        .enumerate()
        .map(|(i, emb)| EmbeddingData {
            object: "embedding".to_string(),
            index: i as u32,
            embedding: emb,
        })
        .collect();

    let resp = EmbeddingResponse {
        object: "list".to_string(),
        data,
        model: provider.model_name().to_string(),
        usage: Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        },
    };

    Ok(Json(resp).into_response())
}
```

- [ ] **Step 3: Add module and route**

In `mod.rs`, add: `pub mod embeddings;`

In `router.rs`, add route:
```rust
.route("/v1/embeddings", post(super::embeddings::handle))
```

- [ ] **Step 4: Update server/mod.rs — add field and wire**

Add `embedding_provider` field to `GatewayServer` struct (similar to how `execution_adapter` was added in Phase 1). Initialize as `None` in `new()`. In `build_router()`, pass it to `OpenAiApiState`.

- [ ] **Step 5: Wire from startup**

In `src/bin/aleph-server/commands/start/mod.rs`, after `agent_result` is built:
```rust
server.embedding_provider = agent_result.embedder.clone();
```

The `AgentHandlersResult` already has `pub embedder: Option<Arc<dyn EmbeddingProvider>>`.

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore`

- [ ] **Step 7: Commit**

```
gateway: implement /v1/embeddings endpoint
```

---

### Task 3: Implement tool passthrough in completions

**Files:**
- Modify: `src/gateway/openai_api/completions/passthrough.rs`

This task adds tools/tool_choice forwarding, tool message support, and tool_calls in non-streaming responses.

- [ ] **Step 1: Add tool conversion imports**

Add to passthrough.rs imports:
```rust
use crate::dispatcher::types::definition::ToolDefinition;
use crate::dispatcher::types::ToolCategory;
use crate::providers::adapter::ToolChoice;
use serde_json::{json, Value};
```

- [ ] **Step 2: Add convert_openai_tools function**

```rust
/// Convert OpenAI function tool definitions to internal ToolDefinition.
fn convert_openai_tools(tools: &[Value]) -> Vec<ToolDefinition> {
    tools
        .iter()
        .filter_map(|t| {
            let func = t.get("function")?;
            Some(ToolDefinition {
                name: func.get("name")?.as_str()?.to_string(),
                description: func.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string(),
                parameters: func.get("parameters").cloned().unwrap_or(json!({})),
                requires_confirmation: false,
                category: ToolCategory::default(),
                llm_context: None,
                strict: func.get("strict").and_then(|v| v.as_bool()).unwrap_or(false),
            })
        })
        .collect()
}
```

- [ ] **Step 3: Add convert_tool_choice function**

```rust
/// Convert OpenAI tool_choice value to internal ToolChoice enum.
fn convert_tool_choice(choice: &Value) -> Option<ToolChoice> {
    match choice {
        Value::String(s) => match s.as_str() {
            "auto" => Some(ToolChoice::Auto),
            "none" => Some(ToolChoice::None),
            "required" => Some(ToolChoice::Required),
            _ => None,
        },
        Value::Object(obj) => obj
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
            .map(|name| ToolChoice::Specific(name.to_string())),
        _ => None,
    }
}
```

- [ ] **Step 4: Add tool message support to convert_messages**

Add `"tool"` arm to the match in `convert_messages()`:

```rust
"tool" => {
    let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
    Some(UnifiedMessage::ToolResult {
        tool_call_id,
        tool_name: String::new(),
        content: vec![ContentBlock::Text {
            text: content_text.to_string(),
        }],
        is_error: false,
    })
}
```

- [ ] **Step 5: Wire tools/tool_choice in RequestPayload**

Replace the hardcoded `tools: None, tool_choice: None` in `handle()`:

```rust
// Convert tools and tool_choice if provided
let tool_defs = req.tools.as_ref().map(|t| convert_openai_tools(t));
let tool_choice = req.tool_choice.as_ref().and_then(convert_tool_choice);

let payload = RequestPayload {
    messages: &unified_messages,
    system_prompt: system_prompt.as_deref(),
    tools: tool_defs.as_deref(),
    think_level: None,
    temperature: req.temperature.map(|t| t as f32),
    max_tokens: req.max_tokens,
    tool_choice,
};
```

- [ ] **Step 6: Add tool_calls to non-streaming response**

In the non-streaming path, replace `tool_calls: None` in the ChatMessage construction:

```rust
let tool_calls = if response.tool_calls.is_empty() {
    None
} else {
    Some(
        response.tool_calls.iter().map(|tc| {
            json!({
                "id": tc.id,
                "type": "function",
                "function": {
                    "name": tc.name,
                    "arguments": tc.arguments.to_string()
                }
            })
        }).collect::<Vec<_>>()
    )
};

// In ChatMessage construction:
ChatMessage {
    role: "assistant".to_string(),
    content: response.text,
    tool_calls,
    tool_call_id: None,
}
```

Also update `finish_reason`: if `tool_calls` is `Some`, use `"tool_calls"` instead of `"stop"`:

```rust
finish_reason: Some(if tool_calls.is_some() { "tool_calls" } else { "stop" }.to_string()),
```

- [ ] **Step 7: Verify**

Run: `cargo check -p alephcore` and `cargo test -p alephcore --lib openai_api`

- [ ] **Step 8: Commit**

```
gateway: add tool/tool_choice passthrough + tool message support
```

---

### Task 4: Implement Responses SSE formatter

**Files:**
- Create: `src/gateway/openai_api/responses/sse.rs`
- Create: `src/gateway/openai_api/responses/mod.rs` (placeholder handler initially)

- [ ] **Step 1: Create responses/sse.rs**

This module converts `BoxStream<ProviderDelta>` to Responses API SSE format.

Key differences from chat completions SSE (`stream.rs`):
- Uses `event: <type>\ndata: <json>\n\n` format (two lines per frame)
- First event is `response.created` (emitted before any deltas)
- Different event types for text, tool calls, reasoning
- Final event is `response.completed` with full response object

```rust
//! Responses API SSE formatting.
//!
//! Converts ProviderDelta stream to OpenAI Responses API SSE events.
//! Format: `event: <type>\ndata: <json>\n\n`

use std::collections::HashMap;
use futures::stream::{self as fstream, BoxStream, StreamExt};
use serde_json::{json, Value};
use crate::providers::delta::ProviderDelta;
use super::super::stream as oai_stream;  // reuse now_timestamp
use super::super::types::ResponsesUsage;

/// Format a Responses SSE event (event: + data: lines)
pub fn sse_event(event_type: &str, data: &Value) -> String {
    format!("event: {event_type}\ndata: {}\n\n", serde_json::to_string(data).unwrap_or_default())
}

/// State accumulated during streaming for the final `response.completed` event
struct StreamState {
    response_id: String,
    model: String,
    created_at: u64,
    text_content: String,
    tool_calls: Vec<Value>,  // accumulated function_call output items
    tool_args: HashMap<String, String>,  // tool_call_id → accumulated arguments
    usage: ResponsesUsage,
}

/// Convert ProviderDelta stream to Responses SSE text frames.
pub fn provider_deltas_to_responses_sse(
    deltas: BoxStream<'static, anyhow::Result<ProviderDelta>>,
    model: String,
) -> BoxStream<'static, String> {
    let response_id = format!("resp-{}", uuid::Uuid::new_v4());
    let created_at = oai_stream::now_timestamp();

    // Emit initial response.created event
    let created_event = sse_event("response.created", &json!({
        "type": "response.created",
        "response": {
            "id": &response_id,
            "object": "response",
            "created_at": created_at,
            "status": "in_progress",
            "model": &model,
        }
    }));

    let state = StreamState {
        response_id,
        model,
        created_at,
        text_content: String::new(),
        tool_calls: Vec::new(),
        tool_args: HashMap::new(),
        usage: ResponsesUsage { input_tokens: 0, output_tokens: 0, total_tokens: 0 },
    };

    let delta_frames = fstream::unfold((deltas, state, false), move |(mut deltas, mut state, done)| {
        async move {
            if done { return None; }
            loop {
                match deltas.next().await {
                    Some(Ok(delta)) => {
                        let frame = match delta {
                            ProviderDelta::TextDelta(text) => {
                                state.text_content.push_str(&text);
                                Some(sse_event("response.output_text.delta", &json!({
                                    "type": "response.output_text.delta",
                                    "output_index": 0,
                                    "content_index": 0,
                                    "delta": text
                                })))
                            }
                            ProviderDelta::ThinkingDelta(text) => {
                                Some(sse_event("response.reasoning.delta", &json!({
                                    "type": "response.reasoning.delta",
                                    "delta": text
                                })))
                            }
                            ProviderDelta::ToolCallStart { id, name } => {
                                state.tool_args.insert(id.clone(), String::new());
                                Some(sse_event("response.output_item.added", &json!({
                                    "type": "response.output_item.added",
                                    "output_index": state.tool_calls.len(),
                                    "item": {
                                        "type": "function_call",
                                        "call_id": id,
                                        "name": name,
                                    }
                                })))
                            }
                            ProviderDelta::ToolCallArgDelta { id, delta } => {
                                if let Some(args) = state.tool_args.get_mut(&id) {
                                    args.push_str(&delta);
                                }
                                Some(sse_event("response.function_call_arguments.delta", &json!({
                                    "type": "response.function_call_arguments.delta",
                                    "delta": delta
                                })))
                            }
                            ProviderDelta::ToolCallEnd { id } => {
                                let args = state.tool_args.remove(&id).unwrap_or_default();
                                state.tool_calls.push(json!({
                                    "type": "function_call",
                                    "call_id": id,
                                    "arguments": args,
                                }));
                                Some(sse_event("response.function_call_arguments.done", &json!({
                                    "type": "response.function_call_arguments.done"
                                })))
                            }
                            ProviderDelta::Usage(u) => {
                                state.usage = ResponsesUsage {
                                    input_tokens: u.input_tokens,
                                    output_tokens: u.output_tokens,
                                    total_tokens: u.input_tokens + u.output_tokens,
                                };
                                None // accumulated, emitted in completed event
                            }
                            ProviderDelta::Done(_) => {
                                // Build output array
                                let mut output = vec![];
                                if !state.text_content.is_empty() {
                                    output.push(json!({
                                        "type": "message",
                                        "role": "assistant",
                                        "content": [{"type": "output_text", "text": &state.text_content}]
                                    }));
                                }
                                output.extend(state.tool_calls.drain(..));

                                let completed = sse_event("response.completed", &json!({
                                    "type": "response.completed",
                                    "response": {
                                        "id": &state.response_id,
                                        "object": "response",
                                        "created_at": state.created_at,
                                        "status": "completed",
                                        "model": &state.model,
                                        "output": output,
                                        "usage": {
                                            "input_tokens": state.usage.input_tokens,
                                            "output_tokens": state.usage.output_tokens,
                                            "total_tokens": state.usage.total_tokens,
                                        }
                                    }
                                }));
                                return Some((completed, (deltas, state, true)));
                            }
                            ProviderDelta::Error(e) => {
                                let failed = sse_event("response.failed", &json!({
                                    "type": "response.failed",
                                    "error": { "message": e.to_string() }
                                }));
                                return Some((failed, (deltas, state, true)));
                            }
                        };
                        if let Some(frame) = frame {
                            return Some((frame, (deltas, state, false)));
                        }
                    }
                    Some(Err(e)) => {
                        let failed = sse_event("response.failed", &json!({
                            "type": "response.failed",
                            "error": { "message": e.to_string() }
                        }));
                        return Some((failed, (deltas, state, true)));
                    }
                    None => return None,
                }
            }
        }
    });

    // Prepend the created event
    Box::pin(fstream::once(async move { created_event }).chain(delta_frames))
}
```

- [ ] **Step 2: Create responses/mod.rs (placeholder)**

```rust
//! POST /v1/responses — OpenAI Responses API passthrough.

pub mod sse;

use std::sync::Arc;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use super::auth::ApiError;
use super::state::OpenAiApiState;
use super::types::ResponsesRequest;

pub async fn handle(
    State(_state): State<Arc<OpenAiApiState>>,
    _headers: HeaderMap,
    Json(_req): Json<ResponsesRequest>,
) -> Result<Response, ApiError> {
    Err(ApiError::ServiceUnavailable("Responses API not yet implemented".into()))
}
```

- [ ] **Step 3: Add module and route**

In `mod.rs`: add `pub mod responses;`
In `router.rs`: add `.route("/v1/responses", post(super::responses::handle))`

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`

- [ ] **Step 5: Commit**

```
gateway: implement Responses SSE formatter with placeholder handler
```

---

### Task 5: Implement /v1/responses handler

**Files:**
- Modify: `src/gateway/openai_api/responses/mod.rs` (replace placeholder)

- [ ] **Step 1: Implement full handler**

Replace the placeholder with the real implementation:

```rust
//! POST /v1/responses — OpenAI Responses API passthrough.

pub mod sse;

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use serde_json::json;

use crate::providers::adapter::RequestPayload;
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::AiProvider;  // needed for .process() on HttpProvider

use super::auth::{extract_bearer_token, ApiError};
use super::state::OpenAiApiState;
use super::stream;
use super::types::{ResponsesRequest, ResponsesInput, ResponsesResponse, ResponsesUsage};

/// Extract text from ResponsesMessage.content (string or array of content parts)
fn extract_text(content: &Option<serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => {
            arr.iter()
                .filter_map(|item| {
                    if item.get("type")?.as_str()? == "input_text" {
                        item.get("text")?.as_str().map(String::from)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("")
        }
        _ => String::new(),
    }
}

/// Convert Responses input to UnifiedMessage list
fn convert_input(input: &ResponsesInput) -> Vec<UnifiedMessage> {
    match input {
        ResponsesInput::Text(s) => vec![UnifiedMessage::user(s.clone())],
        ResponsesInput::Messages(msgs) => {
            msgs.iter()
                .filter_map(|msg| {
                    let text = extract_text(&msg.content);
                    match msg.role.as_str() {
                        "user" => Some(UnifiedMessage::user(text)),
                        "assistant" => Some(UnifiedMessage::assistant(text)),
                        "system" => None, // handled as instructions
                        _ => None,
                    }
                })
                .collect()
        }
    }
}

pub async fn handle(
    State(state): State<Arc<OpenAiApiState>>,
    headers: HeaderMap,
    Json(req): Json<ResponsesRequest>,
) -> Result<Response, ApiError> {
    // Auth
    let auth_header = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let token = extract_bearer_token(auth_header)
        .ok_or_else(|| ApiError::Unauthorized("Missing or invalid Authorization header".into()))?;
    if let Some(ref expected) = state.api_token {
        if token != expected.as_str() {
            return Err(ApiError::Unauthorized("Invalid API key".into()));
        }
    }

    // Lookup provider
    let provider = state.provider_map.get(&req.model)
        .ok_or_else(|| ApiError::NotFound(format!("Model '{}' not found", req.model)))?
        .clone();

    // Convert input
    let messages = convert_input(&req.input);
    let system_prompt = req.instructions.clone();

    // Build payload
    // Reuse tool conversion from completions passthrough if tools are provided
    let payload = RequestPayload {
        messages: &messages,
        system_prompt: system_prompt.as_deref(),
        tools: None,       // TODO: convert tools (same as passthrough)
        think_level: None,
        temperature: req.temperature.map(|t| t as f32),
        max_tokens: req.max_output_tokens,
        tool_choice: None, // TODO: convert tool_choice
    };

    let is_streaming = req.stream.unwrap_or(false);

    if is_streaming {
        let delta_stream = provider.stream_raw(payload).await
            .map_err(|e| ApiError::BadGateway(format!("Provider error: {e}")))?;

        let sse_stream = sse::provider_deltas_to_responses_sse(delta_stream, req.model);
        let body = Body::from_stream(sse_stream.map(Ok::<_, std::convert::Infallible>));

        Ok(Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(body)
            .unwrap()
            .into_response())
    } else {
        let response = provider.process(payload).await
            .map_err(|e| ApiError::BadGateway(format!("Provider error: {e}")))?;

        let mut output = vec![];
        if let Some(text) = &response.text {
            output.push(json!({
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": text}]
            }));
        }
        for tc in &response.tool_calls {
            output.push(json!({
                "type": "function_call",
                "call_id": tc.id,
                "name": tc.name,
                "arguments": tc.arguments.to_string(),
            }));
        }

        let usage = response.usage.map(|u| ResponsesUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            total_tokens: u.input_tokens + u.output_tokens,
        }).unwrap_or(ResponsesUsage { input_tokens: 0, output_tokens: 0, total_tokens: 0 });

        let resp = ResponsesResponse {
            id: format!("resp-{}", uuid::Uuid::new_v4()),
            object: "response".to_string(),
            created_at: stream::now_timestamp(),
            status: "completed".to_string(),
            model: req.model,
            output,
            usage,
        };

        Ok(Json(resp).into_response())
    }
}
```

- [ ] **Step 2: Verify compilation and tests**

Run: `cargo check -p alephcore` and `cargo test -p alephcore --lib openai_api`

- [ ] **Step 3: Commit**

```
gateway: implement /v1/responses passthrough handler
```

---

### Task 6: Wire embedding_provider from startup + final verification

**Files:**
- Modify: `src/gateway/server/mod.rs` — add `embedding_provider` field
- Modify: `src/bin/aleph-server/commands/start/mod.rs` — wire embedder

- [ ] **Step 1: Add field to GatewayServer**

In `src/gateway/server/mod.rs`, add to `GatewayServer` struct:
```rust
pub embedding_provider: Option<std::sync::Arc<dyn crate::memory::EmbeddingProvider>>,
```

Initialize as `None` in `new()` and `with_config()`.

In `build_router()`, pass to `OpenAiApiState`:
```rust
embedding_provider: self.embedding_provider.clone(),
```

- [ ] **Step 2: Wire from startup**

In `src/bin/aleph-server/commands/start/mod.rs`, after the server creation and provider wiring block:
```rust
server.embedding_provider = agent_result.embedder.clone();
```

`AgentHandlersResult.embedder` is `Option<Arc<dyn EmbeddingProvider>>` — direct assignment.

- [ ] **Step 3: Full verification**

Run: `cargo check` (full workspace)
Run: `cargo test -p alephcore --lib openai_api`

All Phase 1 + Phase 2A tests must pass.

- [ ] **Step 4: Commit**

```
gateway: wire embedding provider from startup
```

---

## Dependency Graph

```
Task 1 (types)
  ↓
Task 2 (embeddings)     Task 3 (tool passthrough)     Task 4 (responses SSE)
  ↓                        ↓                              ↓
  └──────────────────────────────────────────┬─────────────┘
                                             ↓
                                    Task 5 (responses handler)
                                             ↓
                                    Task 6 (wiring + verification)
```

Tasks 2, 3, 4 are independent and can run in parallel. Task 5 depends on Task 4 (SSE formatter). Task 6 depends on Task 2 (embedding state).
