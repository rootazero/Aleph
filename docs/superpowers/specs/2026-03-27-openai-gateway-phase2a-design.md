# OpenAI Gateway Phase 2A — Embeddings, Responses, Tool Passthrough

**Date**: 2026-03-27
**Status**: Approved
**Scope**: `/v1/embeddings` + `/v1/responses` (passthrough) + tools/tool_choice forwarding in completions passthrough
**Depends on**: Phase 1 (completed)

## Background

Phase 1 delivered dual-mode `/v1/chat/completions` and `/v1/models`. Phase 2A extends the gateway with three independent capabilities:

1. **`/v1/embeddings`** — Expose Aleph's existing `EmbeddingProvider` via standard OpenAI endpoint
2. **`/v1/responses`** — Transparent proxy for OpenAI Responses API (used by OpenRouter and other intermediaries)
3. **Tool passthrough** — Forward tools/tool_choice in chat completions + support `role: "tool"` messages for multi-turn tool calling

## Design Decisions

1. **Embeddings: single provider** — Uses the memory subsystem's already-configured `EmbeddingProvider`. No model routing. Zero extra configuration.
2. **Responses: passthrough only** — Protocol translation + provider forwarding. No `previous_response_id`, no file upload, no agent mode. Same pattern as chat completions passthrough.
3. **Tool passthrough: complete round-trip** — Forward tools/tool_choice to provider, support tool result messages, return tool_calls in responses.

## Module Structure

```
src/gateway/openai_api/
├── (Phase 1 — unchanged)
│   ├── mod.rs, auth.rs, models.rs, stream.rs
│   └── completions/{mod.rs, agent.rs}
│
├── (Phase 2A — new)
│   ├── embeddings.rs               # POST /v1/embeddings
│   └── responses/
│       ├── mod.rs                   # POST /v1/responses (passthrough)
│       └── sse.rs                   # Responses SSE format (event: + data:)
│
└── (Phase 2A — modified)
    ├── types.rs                     # Embedding + Responses types, ChatMessage.tool_call_id
    ├── state.rs                     # + embedding_provider field
    ├── router.rs                    # + /v1/embeddings, /v1/responses routes
    ├── mod.rs                       # + pub mod embeddings, responses
    └── completions/passthrough.rs   # tools/tool_choice forwarding + tool messages
```

## /v1/embeddings

### State Change

```rust
// state.rs — add field:
pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
```

Injected from memory subsystem's configured provider at startup.

### Handler Flow

```
POST /v1/embeddings { model: "...", input: "hello" | ["hello", "world"] }
  ↓
1. Auth check (Bearer token, same as completions)
  ↓
2. Input validation
   - input: string or string[] → normalize to Vec<String>
   - Max 128 inputs, max 8192 chars per input
   - model: ignored (single provider, no routing)
  ↓
3. embedding_provider.embed_batch(&texts)
  ↓
4. Return OpenAI format response
```

### Response Format

```json
{
  "object": "list",
  "data": [
    { "object": "embedding", "index": 0, "embedding": [0.1, 0.2, ...] }
  ],
  "model": "text-embedding-3-small",
  "usage": { "prompt_tokens": 0, "total_tokens": 0 }
}
```

- `model`: echoes provider's `model_name()`
- `usage`: hardcoded 0 (EmbeddingProvider doesn't return token counts)
- No `encoding_format: "base64"` support (YAGNI)
- No `dimensions` parameter (provider config determines this)

### Types (in types.rs)

```rust
#[derive(Debug, Deserialize)]
pub struct EmbeddingRequest {
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
    pub object: String,           // "list"
    pub data: Vec<EmbeddingData>,
    pub model: String,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct EmbeddingData {
    pub object: String,           // "embedding"
    pub index: u32,
    pub embedding: Vec<f32>,
}
```

### Error Cases

| Scenario | HTTP | code |
|----------|------|------|
| No embedding provider configured | 503 | `service_unavailable` |
| Too many inputs (>128) | 400 | `invalid_request_error` |
| Input too long (>8192 chars) | 400 | `invalid_request_error` |
| Provider error | 502 | `provider_error` |

## /v1/responses (Passthrough)

### Handler Flow

```
POST /v1/responses { model, input, stream, instructions, tools, tool_choice, ... }
  ↓
1. Auth check
  ↓
2. provider_map.get(model) → 404 if not found
  ↓
3. Convert input → UnifiedMessage[]
   - string → single User message
   - array → map by role (user/assistant/system)
   - instructions field → system_prompt
  ↓
4. Build RequestPayload (temperature, max_tokens, tools, tool_choice forwarded)
  ↓
5. Streaming: provider.stream_raw() → ProviderDelta → Responses SSE
   Non-streaming: provider.process() → ProviderResponse → Responses JSON
```

### SSE Format (responses/sse.rs)

Responses API uses a different SSE format from Chat Completions:

```
event: response.created
data: {"type":"response.created","response":{"id":"resp-abc","object":"response","status":"in_progress"}}

event: response.output_text.delta
data: {"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"Hello"}

event: response.output_text.delta
data: {"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":" world"}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp-abc","object":"response","status":"completed","output":[...],"usage":{...}}}
```

ProviderDelta mapping:

| ProviderDelta | Responses SSE event |
|---------------|---------------------|
| `TextDelta(s)` | `response.output_text.delta` |
| `ToolCallStart { id, name }` | `response.output_item.added` (type: function_call) |
| `ToolCallArgDelta { delta }` | `response.function_call_arguments.delta` |
| `ToolCallEnd { id }` | `response.function_call_arguments.done` |
| `ThinkingDelta(s)` | `response.reasoning.delta` (expose reasoning unlike completions) |
| `Usage(u)` | Accumulated, included in `response.completed` |
| `Done(reason)` | `response.completed` with full response object |
| `Error(e)` | `response.failed` |

### Non-Streaming Response

```json
{
  "id": "resp-<uuid>",
  "object": "response",
  "created_at": 1700000000,
  "status": "completed",
  "model": "gpt-4o",
  "output": [
    {
      "type": "message",
      "role": "assistant",
      "content": [{ "type": "output_text", "text": "Paris is the capital of France." }]
    }
  ],
  "usage": { "input_tokens": 10, "output_tokens": 8, "total_tokens": 18 }
}
```

### Types (in types.rs)

```rust
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
    pub content: Option<serde_json::Value>,  // string or array of content parts
    #[serde(default)]
    pub name: Option<String>,  // for function_call_output items
}

#[derive(Debug, Serialize)]
pub struct ResponsesResponse {
    pub id: String,
    pub object: String,           // "response"
    pub created_at: u64,
    pub status: String,           // "completed" | "failed"
    pub model: String,
    pub output: Vec<ResponsesOutputItem>,
    pub usage: ResponsesUsage,
}

#[derive(Debug, Serialize)]
pub struct ResponsesOutputItem {
    pub r#type: String,           // "message"
    pub role: String,
    pub content: Vec<ResponsesContentBlock>,
}

#[derive(Debug, Serialize)]
pub struct ResponsesContentBlock {
    pub r#type: String,           // "output_text"
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct ResponsesUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}
```

### Implementation Notes

- **Initial SSE event**: Emit `response.created` immediately upon stream creation (before first ProviderDelta), with `status: "in_progress"` and generated response ID.
- **Non-streaming tool_calls**: If provider returns `tool_calls`, include `function_call` output items alongside message items in `ResponsesResponse.output`. The `ResponsesOutputItem` type should support `type: "function_call"` with `call_id`, `name`, `arguments` fields.
- **Input content handling**: `ResponsesMessage.content` is `Value` — extract text from string values directly, from array values by finding `input_text` type blocks.

### Not Supported (Phase 2A)

- `previous_response_id` — no session continuation
- File upload / image input
- Agent mode (`aleph/*` models)
- `store` parameter
- `context` / `context_management` parameters

## Tool Passthrough (completions/passthrough.rs)

### 1. tools forwarding

Convert OpenAI `tools` JSON array to `Vec<ToolDefinition>`:

```rust
fn convert_openai_tools(tools: &[Value]) -> Vec<ToolDefinition> {
    tools.iter().filter_map(|t| {
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
    }).collect()
}
```

### 2. tool_choice forwarding

Convert OpenAI `tool_choice` (string or object) to `ToolChoice` enum:

```rust
fn convert_tool_choice(choice: &Value) -> Option<ToolChoice> {
    match choice {
        Value::String(s) => match s.as_str() {
            "auto" => Some(ToolChoice::Auto),
            "none" => Some(ToolChoice::None),
            "required" => Some(ToolChoice::Required),
            _ => None,
        },
        Value::Object(obj) => {
            obj.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(|name| ToolChoice::Specific(name.to_string()))
        },
        _ => None,
    }
}
```

### 3. tool message support

Add `tool_call_id` to `ChatMessage` (types.rs):

```rust
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,  // NEW
}
```

**Note:** All existing `ChatMessage` construction sites (passthrough.rs, types.rs tests, agent.rs) must add `tool_call_id: None` to avoid compile errors.

Update `convert_messages()` — handle `role: "tool"`:

```rust
"tool" => {
    let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
    Some(UnifiedMessage::ToolResult {
        tool_call_id,
        tool_name: String::new(),  // OpenAI tool messages don't include tool_name
        content: vec![ContentBlock::Text { text: content.to_string() }],
        is_error: false,
    })
}
```

**Known limitation:** `tool_name` is empty because OpenAI's `role: "tool"` messages only carry `tool_call_id`, not the tool name. Most providers (OpenAI, Gemini) match by ID and ignore the name. Anthropic requires `tool_name` — if this becomes an issue, the conversion can look up the name from a preceding assistant message's `tool_calls` array by matching `tool_call_id`. Deferred for now (YAGNI — passthrough targets OpenAI-protocol providers).

### 4. Non-streaming tool_calls in response

Map `ProviderResponse.tool_calls` back to OpenAI format in the non-streaming path. Note: `tool_calls` is `Vec<NativeToolCall>` (not `Option`), so check `is_empty()`:

```rust
let tool_calls = if response.tool_calls.is_empty() {
    None
} else {
    Some(response.tool_calls.iter().map(|tc| json!({
        "id": tc.id,
        "type": "function",
        "function": { "name": tc.name, "arguments": tc.arguments.to_string() }
    })).collect::<Vec<_>>())
};
```

## Wiring Changes

### state.rs

Add `embedding_provider: Option<Arc<dyn EmbeddingProvider>>` field.

### router.rs

Add routes:
```rust
.route("/v1/embeddings", post(super::embeddings::handle))
.route("/v1/responses", post(super::responses::handle))
```

### server/mod.rs + startup

Add `embedding_provider` field to `GatewayServer`, wire from memory subsystem at startup.

## Error Handling

All errors use the existing `ApiError` enum (extended in Phase 1). No new variants needed.

## Cleanup

No old code to delete — this is purely additive to Phase 1.

## Acceptance Criteria

1. `POST /v1/embeddings` accepts string or string[], returns OpenAI-format embedding list
2. `POST /v1/responses` proxies to provider, streams Responses SSE format
3. `POST /v1/responses` non-streaming returns Responses JSON format
4. `POST /v1/chat/completions` passthrough forwards tools + tool_choice to provider
5. `POST /v1/chat/completions` passthrough supports `role: "tool"` messages
6. Non-streaming passthrough responses include `tool_calls` when provider returns them
7. All Phase 1 tests continue passing
