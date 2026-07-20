# OpenAI Gateway Phase 1 — Dual-Mode Chat Completions

**Date**: 2026-03-27
**Status**: Approved
**Scope**: `/v1/chat/completions` dual-mode + `/v1/models` hybrid list + streaming SSE

## Background

Aleph's OpenAI-compatible gateway (`/v1/chat/completions`, `/v1/models`) is currently stub-only — returning hardcoded responses and empty model lists. External clients (Cursor, Continue, RAG pipelines) cannot use Aleph as an OpenAI-compatible endpoint.

Reference: OpenClaw recently shipped full `/v1/models`, `/v1/embeddings`, `/v1/chat/completions`, `/v1/responses` with model override forwarding. This design brings Aleph's gateway to parity and beyond, leveraging Aleph's dual-mode architecture as a differentiator.

## Design Decisions

1. **Dual-mode completions** (Approach B — dual path): Passthrough mode bypasses agent loop for zero-overhead LLM proxy; Agent mode enters full ExecutionEngine for tool calling + memory + personality.
2. **Model field as routing signal**: `model: "aleph/{agent_id}"` → Agent mode; anything else → Passthrough mode. No extra headers needed.
3. **Hybrid /v1/models**: Returns both virtual agent IDs (`aleph/default`, `aleph/{agent_id}`) and real models from provider configs. Client sees all options in one list.
4. **Phase 1 scope**: `/v1/chat/completions` + `/v1/models` only. `/v1/embeddings` and `/v1/responses` deferred to Phase 2.

## Module Structure

```
src/gateway/openai_api/
├── mod.rs                  # Public re-exports
├── router.rs               # axum Router construction (replaces old routes.rs)
├── state.rs                # OpenAiApiState with injected dependencies
├── auth.rs                 # Bearer token extraction (existing, minor adaptation)
├── types.rs                # OpenAI-compatible types (existing, augmented)
├── models.rs               # GET /v1/models handler
├── completions/
│   ├── mod.rs              # POST entry point — model prefix dispatch
│   ├── passthrough.rs      # Passthrough path: ProviderRegistry → provider → SSE
│   └── agent.rs            # Agent path: ExecutionEngine → SSE
└── stream.rs               # Shared SSE formatting utilities
```

Old `routes.rs` is deleted entirely — all stub code removed.

## State Injection

```rust
// state.rs
pub struct OpenAiApiState {
    pub server_id: String,
    pub api_token: Option<String>,
    pub execution_engine: Arc<ExecutionEngine<P, R>>,  // Agent path — full loop
    pub http_provider_map: Arc<HashMap<String, Arc<HttpProvider>>>,  // Passthrough — model→provider index
    pub agent_registry: Arc<AgentRegistry>,  // Agent lookup + /v1/models virtual IDs
    pub agent_instances: Arc<RwLock<HashMap<String, Arc<AgentInstance>>>>,  // Active agent instances
    pub provider_configs: Arc<Vec<ProviderConfig>>,  // /v1/models real model list
}
```

**`http_provider_map`**: Built at server startup from `provider_configs` — iterates each config's `models` list and maps model name → `HttpProvider`. This is the model-name-to-provider index needed by the passthrough path. First occurrence wins for dedup.

**`agent_instances`**: Shared with the WebSocket channel's agent lifecycle. `ExecutionEngine::execute()` requires `Arc<AgentInstance>`, not `AgentDef`. The HTTP handler looks up the active instance; if none exists, creates one via the same factory the WebSocket path uses.

Constructed in `GatewayServer::build_router()` using existing infrastructure.

## Route Registration

```rust
// router.rs
pub fn openai_routes(state: Arc<OpenAiApiState>) -> Router {
    Router::new()
        .route("/models", get(models::list_models))
        .route("/models/{model_id}", get(models::get_model))
        .route("/chat/completions", post(completions::handle))
        .route("/health", get(health))
        .with_state(state)
}
```

## Completions — Dual-Mode Dispatch

```rust
// completions/mod.rs
pub async fn handle(..., Json(req): Json<ChatCompletionRequest>) -> Response {
    if req.model.starts_with("aleph/") {
        agent::handle(state, headers, req).await
    } else {
        passthrough::handle(state, headers, req).await
    }
}
```

### Passthrough Path

```
Client → auth → http_provider_map.get(model) → HttpProvider::stream_raw(payload) → ProviderDelta stream → SSE
```

**Mechanism**: Uses `HttpProvider::stream_raw(RequestPayload)` which returns `BoxStream<ProviderDelta>`. This requires converting incoming OpenAI `ChatMessage[]` to `UnifiedMessage[]` — a lightweight in-memory mapping (no LLM calls, no session lookup). The `ProviderDelta` stream is then mapped to OpenAI SSE chunks via `stream.rs`.

This is not a raw HTTP proxy — it goes through Aleph's provider abstraction for protocol normalization (the same `HttpProvider` handles OpenAI, Anthropic, Gemini protocols transparently). The overhead is minimal (message format conversion + delta-to-SSE mapping) but the benefit is significant: a single `/v1/chat/completions` endpoint works regardless of which provider backs the model.

**ProviderDelta → OpenAI SSE mapping** (in `stream.rs`):

| ProviderDelta | SSE delta |
|---------------|-----------|
| `TextDelta(s)` | `delta.content = s` |
| `ToolCallStart { id, name }` | `delta.tool_calls = [{ index, id, function: { name } }]` |
| `ToolCallArgDelta { id, delta }` | `delta.tool_calls = [{ index, function: { arguments: delta } }]` |
| `ToolCallEnd { id }` | (no-op, finish signaled by Done) |
| `ThinkingDelta(s)` | Not emitted (internal reasoning) |
| `Usage(u)` | Stored, included in final chunk's `usage` field |
| `Done(reason)` | `finish_reason: "stop"` + `[DONE]` |
| `Error(e)` | SSE error + close stream |

- **Stateless**: No session, no history. Client manages context.
- **Parameter passthrough**: temperature, max_tokens, top_p, tools, tool_choice — all forwarded as-is via `RequestPayload`.
- **Tool calls**: If provider returns tool_calls (ToolCallStart/ArgDelta/End), they are passed through to client. Aleph does not execute them.
- **Non-streaming** (`stream: false`): Use `HttpProvider::process()` (non-streaming), wrap `ProviderResponse` as `ChatCompletionResponse` → JSON.

### Agent Path

```
Client → auth → parse "aleph/{agent_id}" → AgentRegistry.get(agent_id)
       → build RunRequest → ExecutionEngine::execute(request, agent, emitter) → SSE
```

- **Stateful**: `gateway::router::SessionKey::PerPeer { agent_id, peer_id, epoch: 0 }` — same peer_id shares session history across requests.
- **peer_id**: Derived from bearer token or `x-aleph-user` header.
- **Input extraction**: Last user message as input; if session is empty and client sends full messages array, seed session with that history.
- **Tool execution**: Aleph executes tools internally; tool_calls appear in SSE stream.
- **System prompt**: Agent's configured personality and system prompt are used.
- **Provider**: Determined by agent config, not by model field.

StreamEvent mapping:

| StreamEvent | SSE Output |
|-------------|------------|
| `ResponseChunk { content }` | `delta.content` |
| `ToolCalling { tool_name, arguments }` | `delta.tool_calls` |
| `ToolResult` | Not emitted (internal) |
| `Reasoning` | Not emitted (internal) |
| `RunComplete` | `finish_reason: "stop"` + `[DONE]` |
| `Error` | SSE error event + close stream |

## /v1/models — Hybrid List

**GET /v1/models** returns:

```json
{
  "object": "list",
  "data": [
    { "id": "aleph/default", "object": "model", "created": 1700000000, "owned_by": "aleph" },
    { "id": "aleph/iris", "object": "model", "created": 1700000000, "owned_by": "aleph" },
    { "id": "gpt-4o", "object": "model", "created": 1700000000, "owned_by": "openai" },
    { "id": "claude-sonnet-4-20250514", "object": "model", "created": 1700000000, "owned_by": "anthropic" }
  ]
}
```

- Virtual agent IDs: `aleph/default` + `aleph/{agent_id}` for each registered agent. `owned_by: "aleph"`.
- Real models: Aggregated from all `ProviderConfig.models` lists. `owned_by` = provider name.
- Deduplication: First occurrence wins if same model ID appears in multiple providers.
- `created`: Server startup timestamp.

**GET /v1/models/{model_id}**: Lookup in the same list; 404 if not found.

## SSE Streaming Format

Shared by both paths via `stream.rs`:

```
data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":<ts>,"model":"<model>","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}

data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":<ts>,"model":"<model>","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}

data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":<ts>,"model":"<model>","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

## Error Handling

All errors return OpenAI-compatible format:

```json
{ "error": { "message": "...", "type": "...", "code": "..." } }
```

| Scenario | HTTP | type | code |
|----------|------|------|------|
| Missing/invalid bearer token | 401 | `authentication_error` | `invalid_api_key` |
| Missing model field | 400 | `invalid_request_error` | `missing_model` |
| Model not found in providers | 404 | `invalid_request_error` | `model_not_found` |
| Agent not found | 404 | `invalid_request_error` | `model_not_found` |
| Agent busy | 409 | `conflict_error` | `agent_busy` |
| Provider upstream error | 502 | `upstream_error` | `provider_error` |
| Provider timeout | 504 | `upstream_error` | `timeout` |

**Required `ApiError` extensions**: The existing `ApiError` enum in `auth.rs` only has `Unauthorized`, `BadRequest`, `InternalError`, `ServiceUnavailable`. Must add: `NotFound(404)`, `Conflict(409)`, `BadGateway(502)`, `GatewayTimeout(504)`. The JSON output must also include a `code` field (currently only emits `message` and `type`).

## Security

- **Auth**: Reuse existing Bearer token validation against configured `api_token`.
- **API key isolation**: Provider API keys stored in Vault; never exposed to clients.
- **Input validation**: messages array non-empty, model non-empty. No deep content inspection (R8 — LLM sovereignty).
- **Rate limiting**: Not in Phase 1 (self-hosted single-user). Can add via tower middleware later.

## Required Type Augmentations

`ChatCompletionRequest` in `types.rs` needs these fields added:
- `tool_choice`: `Option<Value>` — forwarded as-is to provider (passthrough) or agent loop (agent mode)
- `tools`: `Option<Vec<ToolDefinition>>` — function definitions for tool calling
- `top_p`, `frequency_penalty`, `presence_penalty`: Optional float parameters
- Ensure `stream` field exists (for mode detection)

`ChatCompletionResponse` streaming delta types need:
- `delta.tool_calls` array support (for ToolCallStart/ArgDelta mapping)
- `usage` field in final chunk

**Non-streaming agent path**: When `stream: false`, use a collecting emitter (accumulate all `StreamEvent`s) then compose a final `ChatCompletionResponse` with full content + tool_calls + usage.

## Cleanup

Delete all stub code:
- `routes.rs` — entire file (replaced by `router.rs` + new modules)
- `OpenAiApiState` old field definitions in `server/mod.rs`

## Out of Scope (Phase 2)

- `/v1/embeddings` — requires new `EmbeddingProvider` trait
- `/v1/responses` — OpenAI Responses API (complex protocol)
- Tools visibility — `tools.effective`, Panel UI "Available Right Now"
- Rate limiting

## Acceptance Criteria

1. `GET /v1/models` returns hybrid list (virtual agent IDs + real models from config)
2. `POST /v1/chat/completions` with real model ID (e.g., `gpt-4o`) transparently proxies to provider with streaming SSE
3. `POST /v1/chat/completions` with `aleph/{agent_id}` enters full agent loop with tool calling and streaming SSE
4. Non-streaming requests (`stream: false`) return complete JSON response
5. All errors return OpenAI-compatible error format
6. Old stub code completely removed, no dead code
