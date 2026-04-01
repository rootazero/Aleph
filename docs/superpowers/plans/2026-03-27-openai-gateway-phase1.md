# OpenAI Gateway Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the OpenAI-compatible gateway to Aleph's execution engine with dual-mode completions (passthrough + agent) and a hybrid model listing.

**Architecture:** Two independent code paths branched by model prefix — `aleph/*` routes through `ExecutionAdapter` + `EventEmitter` for full agent loop; all other models route through `HttpProvider::stream_raw()` for lightweight LLM proxy. Both paths emit OpenAI-format SSE via a shared `stream.rs` formatter.

**Tech Stack:** Rust, axum (Router/State/SSE), tokio mpsc channels, `HttpProvider::stream_raw()` for streaming, `ExecutionAdapter` trait for agent execution.

**Spec:** `docs/superpowers/specs/2026-03-27-openai-gateway-phase1-design.md`

---

### Task 1: Extend ApiError and types.rs

**Files:**
- Modify: `src/gateway/openai_api/auth.rs`
- Modify: `src/gateway/openai_api/types.rs`

This task adds the missing error variants, the `code` field to error JSON output, and augments the OpenAI types needed by subsequent tasks.

- [ ] **Step 1: Add new ApiError variants and code field**

In `src/gateway/openai_api/auth.rs`, add `NotFound`, `Conflict`, `BadGateway`, `GatewayTimeout` variants and a `code()` method:

```rust
pub enum ApiError {
    Unauthorized(String),
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    InternalError(String),
    BadGateway(String),
    GatewayTimeout(String),
    ServiceUnavailable(String),
}

impl ApiError {
    pub fn status_code(&self) -> u16 {
        match self {
            ApiError::Unauthorized(_) => 401,
            ApiError::BadRequest(_) => 400,
            ApiError::NotFound(_) => 404,
            ApiError::Conflict(_) => 409,
            ApiError::InternalError(_) => 500,
            ApiError::BadGateway(_) => 502,
            ApiError::GatewayTimeout(_) => 504,
            ApiError::ServiceUnavailable(_) => 503,
        }
    }

    pub fn code(&self) -> &str {
        match self {
            ApiError::Unauthorized(_) => "invalid_api_key",
            ApiError::BadRequest(_) => "invalid_request_error",
            ApiError::NotFound(_) => "model_not_found",
            ApiError::Conflict(_) => "agent_busy",
            ApiError::InternalError(_) => "internal_error",
            ApiError::BadGateway(_) => "provider_error",
            ApiError::GatewayTimeout(_) => "timeout",
            ApiError::ServiceUnavailable(_) => "service_unavailable",
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let (message, error_type) = match self {
            ApiError::Unauthorized(msg) => (msg.as_str(), "authentication_error"),
            ApiError::BadRequest(msg) => (msg.as_str(), "invalid_request_error"),
            ApiError::NotFound(msg) => (msg.as_str(), "invalid_request_error"),
            ApiError::Conflict(msg) => (msg.as_str(), "conflict_error"),
            ApiError::InternalError(msg) => (msg.as_str(), "internal_error"),
            ApiError::BadGateway(msg) => (msg.as_str(), "upstream_error"),
            ApiError::GatewayTimeout(msg) => (msg.as_str(), "upstream_error"),
            ApiError::ServiceUnavailable(msg) => (msg.as_str(), "service_unavailable"),
        };

        json!({
            "error": {
                "message": message,
                "type": error_type,
                "code": self.code()
            }
        })
    }
}
```

- [ ] **Step 2: Augment types.rs**

Add `tool_choice`, `frequency_penalty`, `presence_penalty` to `ChatCompletionRequest`. Add `tool_calls` to `Delta`. Add `StreamChoice` for streaming responses:

```rust
// In ChatCompletionRequest:
#[serde(default)]
pub tool_choice: Option<serde_json::Value>,
#[serde(default)]
pub frequency_penalty: Option<f64>,
#[serde(default)]
pub presence_penalty: Option<f64>,

// New Delta field:
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

// New types:
#[derive(Debug, Serialize)]
pub struct DeltaToolCall {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<DeltaFunction>,
}

#[derive(Debug, Serialize)]
pub struct DeltaFunction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

// Streaming chunk type (separate from non-streaming ChatChoice):
#[derive(Debug, Serialize)]
pub struct StreamChoice {
    pub index: u32,
    pub delta: Delta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,  // always "chat.completion.chunk"
    pub created: u64,
    pub model: String,
    pub choices: Vec<StreamChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}
```

- [ ] **Step 3: Update existing tests in auth.rs and types.rs**

Update the existing tests to cover new error variants (status codes, JSON output with `code` field) and the new type fields.

- [ ] **Step 4: Run tests to verify**

Run: `cargo test -p alephcore --lib openai_api`
Expected: All tests PASS including new and updated tests.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/openai_api/auth.rs src/gateway/openai_api/types.rs
git commit -m "gateway: extend OpenAI API types and error handling"
```

---

### Task 2: Create state.rs and router.rs

**Files:**
- Create: `src/gateway/openai_api/state.rs`
- Create: `src/gateway/openai_api/router.rs`
- Modify: `src/gateway/openai_api/mod.rs`

This task creates the new state struct with all injected dependencies and the route registration.

- [ ] **Step 1: Create state.rs**

```rust
//! State for the OpenAI-compatible API routes.

use std::collections::HashMap;
use crate::sync_primitives::Arc;

use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::providers::http_provider::HttpProvider;
use crate::providers::ProviderConfig;

/// Shared state for OpenAI-compatible API handlers.
pub struct OpenAiApiState {
    /// Server identifier for health checks
    pub server_id: String,
    /// Expected API token (None = accept any bearer token)
    pub api_token: Option<String>,
    /// Execution adapter for agent mode (type-erased ExecutionEngine)
    pub execution_adapter: Option<Arc<dyn ExecutionAdapter>>,
    /// Model name → HttpProvider index for passthrough mode
    pub provider_map: Arc<HashMap<String, Arc<HttpProvider>>>,
    /// Agent registry for agent lookup + /v1/models virtual IDs
    pub agent_registry: Option<Arc<AgentRegistry>>,
    /// Provider configs for /v1/models real model listing
    pub provider_configs: Arc<Vec<(String, ProviderConfig)>>,  // (name, config) pairs
    /// Server startup timestamp for model `created` field
    pub created_at: u64,
}
```

Note: We use `Option<Arc<dyn ExecutionAdapter>>` instead of directly holding `ExecutionEngine<P, R>` — this avoids the generic parameter problem entirely. `ExecutionAdapter` is the existing type-erased trait that `ExecutionEngine` already implements.

- [ ] **Step 2: Create router.rs**

```rust
//! Axum router for the OpenAI-compatible API.

use std::sync::Arc;
use axum::routing::{get, post};
use axum::{extract::State, Json, Router};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;

use super::auth::ApiError;
use super::state::OpenAiApiState;

pub fn openai_routes(state: Arc<OpenAiApiState>) -> Router {
    Router::new()
        .route("/v1/models", get(super::models::list_models))
        .route("/v1/models/{model_id}", get(super::models::get_model))
        .route("/v1/chat/completions", post(super::completions::handle))
        .route("/v1/health", get(health))
        .with_state(state)
}

async fn health(State(state): State<Arc<OpenAiApiState>>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "server_id": state.server_id,
    }))
}

// IntoResponse for ApiError (moved from old routes.rs)
impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = StatusCode::from_u16(self.status_code())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self.to_json())).into_response()
    }
}
```

- [ ] **Step 3: Update mod.rs**

Replace `src/gateway/openai_api/mod.rs` contents:

```rust
//! OpenAI-compatible API — dual-mode chat completions gateway.
//!
//! Routes: `/v1/models`, `/v1/chat/completions`, `/v1/health`

pub mod auth;
pub mod completions;
pub mod models;
pub mod router;
pub mod state;
pub mod stream;
pub mod types;

// Re-exports for server integration
pub use router::openai_routes;
pub use state::OpenAiApiState;
```

- [ ] **Step 4: Create placeholder modules**

Create empty placeholder files so it compiles. These will be filled in subsequent tasks:

`src/gateway/openai_api/models.rs`:
```rust
//! GET /v1/models handlers (placeholder)
```

`src/gateway/openai_api/stream.rs`:
```rust
//! Shared SSE formatting utilities (placeholder)
```

`src/gateway/openai_api/completions/mod.rs`:
```rust
//! POST /v1/chat/completions — dual-mode dispatch (placeholder)
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles (modules referenced but empty placeholders are fine for now). Fix any import issues.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/openai_api/
git commit -m "gateway: create OpenAI API state, router, and module structure"
```

---

### Task 3: Implement /v1/models (hybrid list)

**Files:**
- Modify: `src/gateway/openai_api/models.rs`

- [ ] **Step 1: Implement list_models and get_model**

```rust
//! GET /v1/models — hybrid model listing.
//!
//! Returns virtual agent IDs (aleph/default, aleph/{agent_id}) plus
//! real model names from all configured providers.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;

use super::auth::{extract_bearer_token, ApiError};
use super::state::OpenAiApiState;
use super::types::{ModelList, ModelObject};
use axum::http::HeaderMap;

/// Build the full model list (called by both list and get).
async fn build_model_list(state: &OpenAiApiState) -> Vec<ModelObject> {
    let mut models = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // 1. Virtual agent IDs
    models.push(ModelObject {
        id: "aleph/default".to_string(),
        object: "model".to_string(),
        created: state.created_at,
        owned_by: "aleph".to_string(),
    });
    seen.insert("aleph/default".to_string());

    if let Some(ref registry) = state.agent_registry {
        for agent_id in registry.list().await {
            let model_id = format!("aleph/{}", agent_id);
            if seen.insert(model_id.clone()) {
                models.push(ModelObject {
                    id: model_id,
                    object: "model".to_string(),
                    created: state.created_at,
                    owned_by: "aleph".to_string(),
                });
            }
        }
    }

    // 2. Real models from provider configs (first occurrence wins)
    for (provider_name, config) in state.provider_configs.iter() {
        for model_name in &config.models {
            if seen.insert(model_name.clone()) {
                models.push(ModelObject {
                    id: model_name.clone(),
                    object: "model".to_string(),
                    created: state.created_at,
                    owned_by: provider_name.clone(),
                });
            }
        }
    }

    models
}

pub async fn list_models(
    State(state): State<Arc<OpenAiApiState>>,
) -> Json<ModelList> {
    let models = build_model_list(&state).await;
    Json(ModelList {
        object: "list".to_string(),
        data: models,
    })
}

pub async fn get_model(
    State(state): State<Arc<OpenAiApiState>>,
    Path(model_id): Path<String>,
) -> Result<Json<ModelObject>, ApiError> {
    let models = build_model_list(&state).await;
    models
        .into_iter()
        .find(|m| m.id == model_id)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("Model '{}' not found", model_id)))
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```bash
git add src/gateway/openai_api/models.rs
git commit -m "gateway: implement /v1/models hybrid listing"
```

---

### Task 4: Implement stream.rs (shared SSE formatter)

**Files:**
- Modify: `src/gateway/openai_api/stream.rs`

This is the core SSE formatting utility shared by both passthrough and agent paths.

- [ ] **Step 1: Implement SseStream and ProviderDelta→SSE mapping**

```rust
//! Shared SSE formatting for OpenAI-compatible streaming.
//!
//! Converts `ProviderDelta` (passthrough) or `StreamEvent` (agent) into
//! OpenAI-format SSE frames: `data: {json}\n\n` ... `data: [DONE]\n\n`.

use futures::stream::{self, BoxStream, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;

use crate::providers::delta::ProviderDelta;
use super::types::{
    ChatCompletionChunk, Delta, DeltaFunction, DeltaToolCall, StreamChoice, Usage,
};

/// Generate a unique completion ID
pub fn completion_id() -> String {
    format!("chatcmpl-{}", uuid::Uuid::new_v4())
}

/// Current Unix timestamp
pub fn now_timestamp() -> u64 {
    chrono::Utc::now().timestamp() as u64
}

/// Format a chunk as an SSE data line: `data: {json}\n\n`
pub fn sse_data(chunk: &ChatCompletionChunk) -> String {
    format!("data: {}\n\n", serde_json::to_string(chunk).unwrap_or_default())
}

/// The terminal SSE frame
pub const SSE_DONE: &str = "data: [DONE]\n\n";

/// Track tool call state for index assignment during streaming
#[derive(Default)]
pub struct ToolCallTracker {
    /// Maps tool_call_id → index
    ids: std::collections::HashMap<String, u32>,
    next_index: u32,
}

impl ToolCallTracker {
    pub fn index_for(&mut self, id: &str) -> u32 {
        if let Some(&idx) = self.ids.get(id) {
            idx
        } else {
            let idx = self.next_index;
            self.ids.insert(id.to_string(), idx);
            self.next_index += 1;
            idx
        }
    }
}

/// Convert a stream of ProviderDelta into SSE text frames.
///
/// Used by the passthrough path.
pub fn provider_deltas_to_sse(
    deltas: BoxStream<'static, anyhow::Result<ProviderDelta>>,
    completion_id: String,
    model: String,
) -> BoxStream<'static, String> {
    let created = now_timestamp();

    Box::pin(stream::unfold(
        (deltas, ToolCallTracker::default(), None::<Usage>, false),
        move |(mut deltas, mut tracker, mut usage_acc, done)| {
            let id = completion_id.clone();
            let model = model.clone();
            async move {
                if done {
                    return None;
                }

                loop {
                    match deltas.next().await {
                        Some(Ok(delta)) => {
                            let frame = match delta {
                                ProviderDelta::TextDelta(text) => {
                                    let chunk = ChatCompletionChunk {
                                        id: id.clone(),
                                        object: "chat.completion.chunk".to_string(),
                                        created,
                                        model: model.clone(),
                                        choices: vec![StreamChoice {
                                            index: 0,
                                            delta: Delta {
                                                content: Some(text),
                                                role: None,
                                                tool_calls: None,
                                            },
                                            finish_reason: None,
                                        }],
                                        usage: None,
                                    };
                                    Some(sse_data(&chunk))
                                }
                                ProviderDelta::ToolCallStart { id: tc_id, name } => {
                                    let idx = tracker.index_for(&tc_id);
                                    let chunk = ChatCompletionChunk {
                                        id: id.clone(),
                                        object: "chat.completion.chunk".to_string(),
                                        created,
                                        model: model.clone(),
                                        choices: vec![StreamChoice {
                                            index: 0,
                                            delta: Delta {
                                                content: None,
                                                role: None,
                                                tool_calls: Some(vec![DeltaToolCall {
                                                    index: idx,
                                                    id: Some(tc_id),
                                                    r#type: Some("function".to_string()),
                                                    function: Some(DeltaFunction {
                                                        name: Some(name),
                                                        arguments: Some(String::new()),
                                                    }),
                                                }]),
                                            },
                                            finish_reason: None,
                                        }],
                                        usage: None,
                                    };
                                    Some(sse_data(&chunk))
                                }
                                ProviderDelta::ToolCallArgDelta { id: tc_id, delta: arg } => {
                                    let idx = tracker.index_for(&tc_id);
                                    let chunk = ChatCompletionChunk {
                                        id: id.clone(),
                                        object: "chat.completion.chunk".to_string(),
                                        created,
                                        model: model.clone(),
                                        choices: vec![StreamChoice {
                                            index: 0,
                                            delta: Delta {
                                                content: None,
                                                role: None,
                                                tool_calls: Some(vec![DeltaToolCall {
                                                    index: idx,
                                                    id: None,
                                                    r#type: None,
                                                    function: Some(DeltaFunction {
                                                        name: None,
                                                        arguments: Some(arg),
                                                    }),
                                                }]),
                                            },
                                            finish_reason: None,
                                        }],
                                        usage: None,
                                    };
                                    Some(sse_data(&chunk))
                                }
                                ProviderDelta::ToolCallEnd { .. } => None, // no-op
                                ProviderDelta::ThinkingDelta(_) => None,   // suppress
                                ProviderDelta::Usage(u) => {
                                    // TokenUsage fields: input_tokens: u32, output_tokens: u32
                                    usage_acc = Some(Usage {
                                        prompt_tokens: u.input_tokens,
                                        completion_tokens: u.output_tokens,
                                        total_tokens: u.input_tokens + u.output_tokens,
                                    });
                                    None
                                }
                                ProviderDelta::Done(_reason) => {
                                    let chunk = ChatCompletionChunk {
                                        id: id.clone(),
                                        object: "chat.completion.chunk".to_string(),
                                        created,
                                        model: model.clone(),
                                        choices: vec![StreamChoice {
                                            index: 0,
                                            delta: Delta {
                                                content: None,
                                                role: None,
                                                tool_calls: None,
                                            },
                                            finish_reason: Some("stop".to_string()),
                                        }],
                                        usage: usage_acc.take(),
                                    };
                                    let mut frames = sse_data(&chunk);
                                    frames.push_str(SSE_DONE);
                                    return Some((frames, (deltas, tracker, usage_acc, true)));
                                }
                                ProviderDelta::Error(e) => {
                                    let err_json = serde_json::json!({"error": e.to_string()});
                                    let mut frames = format!("data: {}\n\n", err_json);
                                    frames.push_str(SSE_DONE);
                                    return Some((frames, (deltas, tracker, usage_acc, true)));
                                }
                            };

                            if let Some(frame) = frame {
                                return Some((frame, (deltas, tracker, usage_acc, false)));
                            }
                            // continue loop for no-op deltas
                        }
                        Some(Err(e)) => {
                            let err_json = serde_json::json!({"error": e.to_string()});
                            let frames = format!("data: {}\n\n{}", err_json, SSE_DONE);
                            return Some((frames, (deltas, tracker, usage_acc, true)));
                        }
                        None => {
                            // Stream ended without Done — emit done anyway
                            return Some((SSE_DONE.to_string(), (deltas, tracker, usage_acc, true)));
                        }
                    }
                }
            }
        },
    ))
}
```

**Important codebase notes:**
- `TokenUsage` fields are `input_tokens: u32` and `output_tokens: u32` (NOT `prompt_tokens`/`completion_tokens`, NOT `Option<u32>`). There is no `total_tokens` — compute it.
- `ProviderResponse` text field is `text: Option<String>` (NOT `content`).
- Error SSE frames use `serde_json::json!` for proper escaping.

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```bash
git add src/gateway/openai_api/stream.rs
git commit -m "gateway: implement shared SSE stream formatter"
```

---

### Task 5: Implement passthrough completions path

**Files:**
- Create: `src/gateway/openai_api/completions/passthrough.rs`
- Modify: `src/gateway/openai_api/completions/mod.rs`

- [ ] **Step 1: Implement completions/mod.rs (dispatch logic)**

```rust
//! POST /v1/chat/completions — dual-mode dispatch.
//!
//! Routes to agent path if model starts with "aleph/",
//! otherwise routes to passthrough path.

pub mod agent;
pub mod passthrough;

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::auth::{extract_bearer_token, ApiError};
use super::state::OpenAiApiState;
use super::types::ChatCompletionRequest;

pub async fn handle(
    State(state): State<Arc<OpenAiApiState>>,
    headers: HeaderMap,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Response, ApiError> {
    // Auth check
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = extract_bearer_token(auth_header)
        .ok_or_else(|| ApiError::Unauthorized("Missing or invalid Authorization header".into()))?;

    if let Some(ref expected) = state.api_token {
        if token != expected.as_str() {
            return Err(ApiError::Unauthorized("Invalid API key".into()));
        }
    }

    // Validate required fields
    if req.model.is_empty() {
        return Err(ApiError::BadRequest("Missing 'model' field".into()));
    }
    if req.messages.is_empty() {
        return Err(ApiError::BadRequest("'messages' must not be empty".into()));
    }

    // Route by model prefix
    if req.model.starts_with("aleph/") {
        agent::handle(state, &headers, req).await
    } else {
        passthrough::handle(state, req).await
    }
}
```

- [ ] **Step 2: Implement completions/passthrough.rs**

```rust
//! Passthrough path — direct LLM proxy via HttpProvider.
//!
//! Converts OpenAI messages to UnifiedMessage, calls provider.stream_raw()
//! or provider.process(), and returns the response in OpenAI format.

use std::sync::Arc;

use axum::body::Body;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use futures::StreamExt;

use crate::providers::adapter::RequestPayload;
use crate::providers::message::{ContentBlock, UnifiedMessage};

use super::super::auth::ApiError;
use super::super::state::OpenAiApiState;
use super::super::stream;
use super::super::types::{
    ChatChoice, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, Usage,
};

/// Convert OpenAI ChatMessage to Aleph UnifiedMessage.
/// System messages are excluded here — they are extracted separately via `extract_system_prompt()`.
fn convert_messages(messages: &[ChatMessage]) -> Vec<UnifiedMessage> {
    messages
        .iter()
        .filter_map(|msg| {
            let content = msg.content.as_deref().unwrap_or("");
            match msg.role.as_str() {
                "user" => Some(UnifiedMessage::User {
                    content: vec![ContentBlock::Text { text: content.to_string() }],
                }),
                "assistant" => Some(UnifiedMessage::Assistant {
                    content: vec![ContentBlock::Text { text: content.to_string() }],
                }),
                "system" => None, // Handled by extract_system_prompt()
                "tool" => None,   // Tool results would need tool_call_id — skip for now
                _ => None,
            }
        })
        .collect()
}

/// Extract system prompt from messages (first system message, if any)
fn extract_system_prompt(messages: &[ChatMessage]) -> Option<String> {
    messages
        .iter()
        .find(|m| m.role == "system")
        .and_then(|m| m.content.clone())
}

pub async fn handle(
    state: Arc<OpenAiApiState>,
    req: ChatCompletionRequest,
) -> Result<Response, ApiError> {
    // Look up provider by model name
    let provider = state
        .provider_map
        .get(&req.model)
        .cloned()
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Model '{}' not found in configured providers",
                req.model
            ))
        })?;

    // Convert messages
    let unified = convert_messages(&req.messages);
    let system_prompt = extract_system_prompt(&req.messages);

    // Build request payload
    // TODO: Convert req.tools (Vec<Value>) → Vec<ToolDefinition> if needed for full tool passthrough.
    // TODO: Convert req.tool_choice (Value) → ToolChoice enum if needed.
    // For now, temperature and max_tokens are forwarded; tools/tool_choice deferred until
    // the passthrough path needs to support tool-calling clients.
    let payload = RequestPayload {
        messages: &unified,
        system_prompt: system_prompt.as_deref(),
        tools: None,
        think_level: None,
        temperature: req.temperature.map(|t| t as f32),
        max_tokens: req.max_tokens,
        tool_choice: None,
    };

    let is_streaming = req.stream.unwrap_or(false);

    if is_streaming {
        // Streaming path: stream_raw() → ProviderDelta → SSE
        let delta_stream = provider.stream_raw(payload).await.map_err(|e| {
            ApiError::BadGateway(format!("Provider error: {}", e))
        })?;

        let id = stream::completion_id();
        let model = req.model.clone();
        let sse_stream = stream::provider_deltas_to_sse(delta_stream, id, model);

        let body = Body::from_stream(sse_stream.map(Ok::<_, std::convert::Infallible>));

        Ok(Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(body)
            .unwrap()
            .into_response())
    } else {
        // Non-streaming path: process() → ProviderResponse → JSON
        let response = provider.process(payload).await.map_err(|e| {
            ApiError::BadGateway(format!("Provider error: {}", e))
        })?;

        // ProviderResponse fields: text: Option<String>, usage: Option<TokenUsage>
        // TokenUsage fields: input_tokens: u32, output_tokens: u32
        let content = response.text.unwrap_or_default();
        let resp = ChatCompletionResponse {
            id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
            object: "chat.completion".to_string(),
            created: stream::now_timestamp(),
            model: req.model,
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some(content),
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
                delta: None,
            }],
            usage: response.usage.map(|u| Usage {
                prompt_tokens: u.input_tokens,
                completion_tokens: u.output_tokens,
                total_tokens: u.input_tokens + u.output_tokens,
            }),
        };

        Ok(axum::Json(resp).into_response())
    }
}
```

Note: The exact field names on `ProviderResponse` (e.g., `content`, `usage`, `usage.prompt_tokens`) need to be verified against the actual struct. Adjust as needed during implementation.

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`

- [ ] **Step 4: Commit**

```bash
git add src/gateway/openai_api/completions/
git commit -m "gateway: implement passthrough completions path"
```

---

### Task 6: Implement agent completions path

**Files:**
- Modify: `src/gateway/openai_api/completions/agent.rs`

- [ ] **Step 1: Implement agent path with EventEmitter**

```rust
//! Agent path — full agent loop via ExecutionAdapter.
//!
//! Maps StreamEvent to OpenAI SSE format using a channel-based EventEmitter.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::router::SessionKey;

use super::super::auth::ApiError;
use super::super::state::OpenAiApiState;
use super::super::stream;
use super::super::types::{
    ChatChoice, ChatCompletionChunk, ChatCompletionResponse, ChatMessage,
    Delta, DeltaFunction, DeltaToolCall, StreamChoice, Usage,
};

/// EventEmitter that sends SSE-formatted strings through an mpsc channel.
struct SseEventEmitter {
    tx: mpsc::Sender<String>,
    completion_id: String,
    model: String,
    created: u64,
    seq: std::sync::atomic::AtomicU64,
}

#[async_trait]
impl EventEmitter for SseEventEmitter {
    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        let frame = match &event {
            StreamEvent::ResponseChunk { content, .. } => {
                let chunk = ChatCompletionChunk {
                    id: self.completion_id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: self.created,
                    model: self.model.clone(),
                    choices: vec![StreamChoice {
                        index: 0,
                        delta: Delta {
                            content: Some(content.clone()),
                            role: None,
                            tool_calls: None,
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                };
                Some(stream::sse_data(&chunk))
            }
            StreamEvent::ToolStart { tool_name, tool_id, params, .. } => {
                let chunk = ChatCompletionChunk {
                    id: self.completion_id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: self.created,
                    model: self.model.clone(),
                    choices: vec![StreamChoice {
                        index: 0,
                        delta: Delta {
                            content: None,
                            role: None,
                            tool_calls: Some(vec![DeltaToolCall {
                                index: 0,
                                id: Some(tool_id.clone()),
                                r#type: Some("function".to_string()),
                                function: Some(DeltaFunction {
                                    name: Some(tool_name.clone()),
                                    arguments: Some(params.to_string()),
                                }),
                            }]),
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                };
                Some(stream::sse_data(&chunk))
            }
            StreamEvent::RunComplete { .. } => {
                let chunk = ChatCompletionChunk {
                    id: self.completion_id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: self.created,
                    model: self.model.clone(),
                    choices: vec![StreamChoice {
                        index: 0,
                        delta: Delta {
                            content: None,
                            role: None,
                            tool_calls: None,
                        },
                        finish_reason: Some("stop".to_string()),
                    }],
                    usage: None,
                };
                let mut frame = stream::sse_data(&chunk);
                frame.push_str(stream::SSE_DONE);
                Some(frame)
            }
            StreamEvent::RunError { error, .. } => {
                let frame = format!("data: {{\"error\": \"{}\"}}\n\n{}", error, stream::SSE_DONE);
                Some(frame)
            }
            // Suppress internal events (no catch-all — new variants trigger compile error)
            StreamEvent::Reasoning { .. }
            | StreamEvent::ToolEnd { .. }
            | StreamEvent::ToolUpdate { .. }
            | StreamEvent::RunAccepted { .. }
            | StreamEvent::AskUser { .. } => None,
        };

        if let Some(frame) = frame {
            self.tx.send(frame).await.map_err(|_| {
                EventEmitError::ChannelClosed
            })?;
        }

        Ok(())
    }
}

pub async fn handle(
    state: Arc<OpenAiApiState>,
    headers: &HeaderMap,
    req: super::super::types::ChatCompletionRequest,
) -> Result<Response, ApiError> {
    let execution_adapter = state
        .execution_adapter
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Agent execution not available".into()))?;

    let agent_registry = state
        .agent_registry
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Agent registry not available".into()))?;

    // Parse agent ID from model field: "aleph/iris" → "iris", "aleph/default" → default agent
    let agent_id_part = req.model.strip_prefix("aleph/").unwrap_or("default");

    let agent = if agent_id_part == "default" {
        agent_registry.get_default().await
    } else {
        agent_registry.get(agent_id_part).await
    };

    let agent = agent.ok_or_else(|| {
        ApiError::NotFound(format!("Agent '{}' not found", agent_id_part))
    })?;

    // Build peer_id from x-aleph-user header or bearer token
    let peer_id = headers
        .get("x-aleph-user")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "openai-api-client".to_string());

    // Extract input: last user message
    let input = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.clone())
        .unwrap_or_default();

    let session_key = SessionKey::PerPeer {
        agent_id: agent.id().to_string(),
        peer_id,
        epoch: 0,
    };

    let run_request = RunRequest {
        run_id: format!("openai-{}", uuid::Uuid::new_v4()),
        input,
        session_key,
        timeout_secs: None,
        metadata: HashMap::new(),
        attachments: Vec::new(),
        pending_media: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
    };

    let is_streaming = req.stream.unwrap_or(false);

    if is_streaming {
        let (tx, mut rx) = mpsc::channel::<String>(256);
        let completion_id = stream::completion_id();
        let model = req.model.clone();
        let created = stream::now_timestamp();

        // Send initial role chunk
        let initial_chunk = ChatCompletionChunk {
            id: completion_id.clone(),
            object: "chat.completion.chunk".to_string(),
            created,
            model: model.clone(),
            choices: vec![StreamChoice {
                index: 0,
                delta: Delta {
                    content: None,
                    role: Some("assistant".to_string()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        };
        let _ = tx.send(stream::sse_data(&initial_chunk)).await;

        let emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(SseEventEmitter {
            tx,
            completion_id,
            model,
            created,
            seq: std::sync::atomic::AtomicU64::new(0),
        });

        // Spawn execution in background
        let adapter = execution_adapter.clone();
        tokio::spawn(async move {
            if let Err(e) = adapter.execute(run_request, agent, emitter).await {
                tracing::error!("Agent execution failed: {}", e);
            }
        });

        // Stream SSE from receiver
        let body_stream = async_stream::stream! {
            while let Some(frame) = rx.recv().await {
                yield Ok::<_, std::convert::Infallible>(frame);
            }
        };

        let body = Body::from_stream(body_stream);

        Ok(Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(body)
            .unwrap()
            .into_response())
    } else {
        // Non-streaming: collect all events, build final response
        let (tx, mut rx) = mpsc::channel::<StreamEvent>(256);

        struct CollectingEmitter {
            tx: mpsc::Sender<StreamEvent>,
            seq: std::sync::atomic::AtomicU64,
        }

        #[async_trait]
        impl EventEmitter for CollectingEmitter {
            fn next_seq(&self) -> u64 {
                self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            }
            async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
                self.tx.send(event).await.map_err(|_| EventEmitError::ChannelClosed)
            }
        }

        let emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(CollectingEmitter {
            tx,
            seq: std::sync::atomic::AtomicU64::new(0),
        });

        let adapter = execution_adapter.clone();
        let agent_clone = agent.clone();
        let handle = tokio::spawn(async move {
            adapter.execute(run_request, agent_clone, emitter).await
        });

        // Collect response content and usage from events
        let mut content = String::new();
        let mut usage = Usage { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 };
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::ResponseChunk { content: chunk, .. } => {
                    content.push_str(&chunk);
                }
                StreamEvent::RunComplete { summary, .. } => {
                    // Extract usage from RunSummary if available
                    usage.total_tokens = summary.total_tokens.unwrap_or(0);
                }
                _ => {}
            }
        }

        // Wait for execution to complete
        let _ = handle.await;

        let resp = ChatCompletionResponse {
            id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
            object: "chat.completion".to_string(),
            created: stream::now_timestamp(),
            model: req.model,
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some(content),
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
                delta: None,
            }],
            usage: Some(usage),
        };

        Ok(axum::Json(resp).into_response())
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```bash
git add src/gateway/openai_api/completions/agent.rs
git commit -m "gateway: implement agent completions path with SSE streaming"
```

---

### Task 7: Wire into GatewayServer and cleanup

**Files:**
- Modify: `src/gateway/server/mod.rs` (build_router)
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (pass dependencies)
- Delete: `src/gateway/openai_api/routes.rs`

This task replaces the old stub with the new state + router and wires the execution adapter and agent registry through from server startup.

- [ ] **Step 1: Update GatewayServer to accept new dependencies**

Add fields to `GatewayServer`:

```rust
// In GatewayServer struct:
pub execution_adapter: Option<Arc<dyn ExecutionAdapter>>,
pub agent_registry_instances: Option<Arc<crate::gateway::agent_instance::AgentRegistry>>,
pub provider_configs: Vec<(String, crate::providers::ProviderConfig)>,
pub provider_map: Arc<HashMap<String, Arc<crate::providers::http_provider::HttpProvider>>>,
```

Add setter methods and update `new()` to initialize them as `None`/empty.

- [ ] **Step 2: Update build_router() to use new OpenAiApiState**

Replace the old `OpenAiApiState` construction:

```rust
// In build_router():
use super::openai_api::{openai_routes, OpenAiApiState};

let openai_state = Arc::new(OpenAiApiState {
    server_id: format!("aleph-{}", self.addr),
    api_token: None,
    execution_adapter: self.execution_adapter.clone(),
    provider_map: self.provider_map.clone(),
    agent_registry: self.agent_registry_instances.clone(),
    provider_configs: Arc::new(self.provider_configs.clone()),
    created_at: std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs(),
});
let openai = openai_routes(openai_state);
```

- [ ] **Step 3: Wire dependencies from startup code**

In `src/bin/aleph-server/commands/start/mod.rs`, after `agent_result` is built, pass the execution adapter and agent registry to the gateway server:

```rust
// After agent initialization:
server.execution_adapter = agent_result.execution_adapter.clone();
server.agent_registry_instances = agent_result.agent_registry.clone()
    .map(|_| /* need the gateway::agent_instance::AgentRegistry */ );
```

Note: There are TWO `AgentRegistry` types — `agents::registry::AgentRegistry` (holds `AgentDef`) and `gateway::agent_instance::AgentRegistry` (holds `AgentInstance`). The startup code creates `gateway::agent_instance::AgentRegistry` for the execution engine. Wire that one through.

Also build the `provider_map` from configs:

```rust
// Build model → provider map
let mut provider_map = HashMap::new();
for (name, config) in &app_config.providers {
    if let Ok(provider) = create_http_provider(name, config) {
        for model in &config.models {
            provider_map.entry(model.clone()).or_insert_with(|| Arc::new(provider.clone()));
        }
    }
}
server.provider_map = Arc::new(provider_map);
```

- [ ] **Step 4: Delete old routes.rs**

```bash
rm src/gateway/openai_api/routes.rs
```

Ensure no remaining references to the deleted file. Update any imports.

- [ ] **Step 5: Verify full compilation**

Run: `cargo check -p alephcore`
Then: `cargo check` (full workspace)

- [ ] **Step 6: Verify tests pass**

Run: `cargo test -p alephcore --lib openai_api`

Some old tests in the deleted `routes.rs` will need to be recreated or adapted in the new router module. The auth and types tests should still pass.

- [ ] **Step 7: Commit**

```bash
git add -A src/gateway/openai_api/ src/gateway/server/mod.rs src/bin/
git commit -m "gateway: wire OpenAI API to execution engine, delete old stubs"
```

---

### Task 8: Integration test and final verification

**Files:**
- No new files — runtime verification

- [ ] **Step 1: Build release binary**

Run: `cargo build --bin aleph-server`
Expected: Clean compilation, no warnings in openai_api module.

- [ ] **Step 2: Start server and test /v1/models**

```bash
# Kill any existing instances first
pkill -f "target/debug/aleph-server" 2>/dev/null; sleep 2
target/debug/aleph-server start &
sleep 3

# Test models endpoint
curl -s http://localhost:3000/v1/models | jq .
```

Expected: JSON with `"object": "list"` and `data` array containing `aleph/default` + any configured real models.

- [ ] **Step 3: Test passthrough streaming**

```bash
curl -s -N http://localhost:3000/v1/chat/completions \
  -H "Authorization: Bearer test" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o","messages":[{"role":"user","content":"Say hello"}],"stream":true}'
```

Expected: SSE stream with `data: {...}` chunks ending in `data: [DONE]`.

- [ ] **Step 4: Test agent mode**

```bash
curl -s -N http://localhost:3000/v1/chat/completions \
  -H "Authorization: Bearer test" \
  -H "Content-Type: application/json" \
  -d '{"model":"aleph/default","messages":[{"role":"user","content":"Hello"}],"stream":true}'
```

Expected: SSE stream from agent loop with content chunks.

- [ ] **Step 5: Test error cases**

```bash
# Missing auth
curl -s http://localhost:3000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4","messages":[{"role":"user","content":"Hi"}]}'

# Unknown model
curl -s http://localhost:3000/v1/chat/completions \
  -H "Authorization: Bearer test" \
  -H "Content-Type: application/json" \
  -d '{"model":"nonexistent","messages":[{"role":"user","content":"Hi"}]}'
```

Expected: 401 with `authentication_error`, 404 with `model_not_found`.

- [ ] **Step 6: Kill test server**

```bash
pkill -f "target/debug/aleph-server" 2>/dev/null
```

- [ ] **Step 7: Final commit (if any fixes needed)**

```bash
git add -A && git commit -m "gateway: fix integration issues from testing"
```

---

## Dependency Graph

```
Task 1 (types + errors)
  ↓
Task 2 (state + router + mod.rs)
  ↓
Task 3 (/v1/models)     Task 4 (stream.rs)
  ↓                        ↓
  └─────────┬──────────────┘
            ↓
Task 5 (passthrough)     Task 6 (agent)
  ↓                        ↓
  └─────────┬──────────────┘
            ↓
Task 7 (wire + cleanup)
            ↓
Task 8 (integration test)
```

Tasks 3 and 4 can be done in parallel. Tasks 5 and 6 can be done in parallel (both depend on 4).
