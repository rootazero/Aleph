# Codex Responses API Protocol Adapter Design

> Replaces the ChatGPT conversation endpoint with the Codex Responses API endpoint.

## Background

The initial ChatGPT subscription provider used `/backend-api/conversation` with a Chat Completions-like wire format. Research revealed that OpenAI's Codex CLI uses a different endpoint (`/backend-api/codex/responses`) with the **Responses API** format — a more powerful protocol that supports reasoning items, typed tool calls, and incremental streaming deltas.

This redesign switches to the Responses API format exclusively (方案 B), enabling access to Codex models (codex-mini-latest, gpt-5.2-codex, gpt-5.3-codex) and first-class reasoning support.

## Architecture

### Scope of Changes

| File | Change | Notes |
|------|--------|-------|
| `chatgpt/types.rs` | **Rewrite** | Conversation types → Responses API types |
| `protocols/chatgpt.rs` | **Rewrite** | Conversation protocol → Responses API protocol |
| `chatgpt/auth.rs` | **No change** | OAuth browser flow identical for both endpoints |
| `chatgpt/security.rs` | **No change** | CSRF/requirements/PoW reused |
| `presets.rs` | **Minor** | default_model → `codex-mini-latest` |

### Endpoint Change

```
Before: POST https://chatgpt.com/backend-api/conversation
After:  POST https://chatgpt.com/backend-api/codex/responses
```

## Responses API Types

### Request

```rust
#[derive(Debug, Serialize)]
pub struct ResponsesRequest {
    pub model: String,
    pub input: Vec<InputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    pub stream: bool,
    pub store: bool,  // always false for Codex subscription
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum InputItem {
    #[serde(rename = "message")]
    Message { role: String, content: String },
}

#[derive(Debug, Serialize)]
pub struct ReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,   // "low" | "medium" | "high"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,  // "auto" | "concise" | "detailed"
}

#[derive(Debug, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,  // "function"
    pub function: FunctionDef,
}

#[derive(Debug, Serialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}
```

### Response

```rust
#[derive(Debug, Deserialize)]
pub struct ResponseResource {
    pub id: String,
    pub status: String,  // "completed" | "failed" | "in_progress" | "cancelled" | "incomplete"
    pub model: String,
    pub output: Vec<OutputItem>,
    #[serde(default)]
    pub usage: Option<UsageInfo>,
    #[serde(default)]
    pub error: Option<ResponseError>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum OutputItem {
    #[serde(rename = "message")]
    Message {
        id: String,
        role: String,
        content: Vec<ContentPart>,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        id: String,
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        summary: Option<String>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
}

#[derive(Debug, Deserialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub part_type: String,  // "output_text"
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct UsageInfo {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Deserialize)]
pub struct ResponseError {
    pub code: String,
    pub message: String,
}
```

### Streaming Events

SSE events use typed `event:` fields (not raw `data:` lines):

```
event: response.created
data: {"type":"response.created","response":{...}}

event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"Hello","output_index":0,"content_index":0}

event: response.completed
data: {"type":"response.completed","response":{...}}
```

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    #[serde(rename = "response.created")]
    Created { response: ResponseResource },
    #[serde(rename = "response.in_progress")]
    InProgress { response: ResponseResource },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded { output_index: usize, item: OutputItem },
    #[serde(rename = "response.output_text.delta")]
    TextDelta { delta: String, output_index: usize, content_index: usize },
    #[serde(rename = "response.output_text.done")]
    TextDone { text: String, output_index: usize, content_index: usize },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone { output_index: usize, item: OutputItem },
    #[serde(rename = "response.completed")]
    Completed { response: ResponseResource },
    #[serde(rename = "response.failed")]
    Failed { response: ResponseResource },
}
```

## Protocol Adapter

### Key Changes from Conversation Protocol

| Aspect | Before (conversation) | After (Responses API) |
|--------|----------------------|----------------------|
| Endpoint | `/backend-api/conversation` | `/backend-api/codex/responses` |
| System prompt | Prepended to user message | `instructions` field |
| Messages | `messages[]` with `author.role` | `input[]` with `InputItem::Message` |
| Streaming | Cumulative text → compute delta | Direct `TextDelta` events |
| Non-streaming | Collect SSE → take last text | Parse `ResponseResource.output` |
| store param | N/A | `store: false` (required) |
| Reasoning | Not supported | `reasoning.effort` + reasoning output items |

### Streaming Strategy

The Responses API sends incremental `TextDelta` events directly — no cumulative-to-delta conversion needed:

```
Before: "Hello" → "Hello world" → delta = " world" (computed)
After:  delta: "Hello" → delta: " world" (direct from API)
```

### ThinkLevel Mapping

| Aleph ThinkLevel | Responses API reasoning.effort |
|-----------------|-------------------------------|
| None / Off | omit reasoning field entirely |
| Low | "low" |
| Medium | "medium" |
| High | "high" |

## Preset Update

```rust
ProviderPreset {
    name: "chatgpt",
    base_url: "https://chatgpt.com",
    protocol: "chatgpt",
    default_model: "codex-mini-latest",  // changed from gpt-4o
    color: "#10a37f",
}
```

## References

- [OpenAI Codex CLI](https://github.com/openai/codex) — open-source, uses `backend-api/codex/responses`
- [OpenClaw](~/Workspace/openclaw) — reference implementation of Codex OAuth + Responses API
- [Codex Config](https://developers.openai.com/codex/config-sample/) — wire_api = "responses"
- Previous design: `docs/plans/2026-03-04-openai-subscription-provider-design.md`
