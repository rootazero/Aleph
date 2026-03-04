# Native Tool Use Migration Design

> **Date**: 2026-03-05
> **Status**: Approved
> **Scope**: Migrate LLM communication from JSON-in-text to native tool_use API

## Problem

Aleph 当前让 LLM 在文本中输出 JSON 格式的 action 指令，然后用 650+ 行的 `DecisionParser` 解析。这存在多个严重问题：

1. **JSON 解析脆弱**：当 LLM 的 JSON 内容包含 markdown 代码块（\`\`\`）时，`extract_from_code_block()` 将内部代码块误识为闭合标记，导致 JSON 截断
2. **超长响应截断**：LLM 输出超过 max_tokens 时 JSON 被截断，解析失败
3. **复杂 fallback 链**：4 种 JSON 提取策略 + 5 级 fallback，维护成本高
4. **Token 浪费**：工具 schema 和 JSON 格式指令注入 system prompt，每次请求浪费 2000-5000 tokens

**实际 bug 复现**：用户通过 Telegram 请求生成 A 股技术分析报告 → LLM 返回 pdf_generate 工具调用 → markdown 内容中的 \`\`\` 代码块导致 JSON 提取截断 → 整个 JSON blob 作为"直接回答"发送给 Telegram。

**根本原因**：使用 pre-function-calling 时代的方案（让 LLM 在文本中输出 JSON），而所有主流 Provider 已原生支持 tool_use API。

## Solution

迁移到 **原生 tool_use API**。每个 Provider 的 ProtocolAdapter 使用其原生的工具调用协议：

- **Anthropic**: `tools` 参数 + `tool_use` content blocks
- **OpenAI**: `tools` 参数 + `tool_calls` response field
- **Gemini**: `functionDeclarations` + `functionCall` parts
- **Fallback**: 不支持的 Provider 保留 JSON-in-text 路径

参考 OpenClaw 项目的架构：LLM 通信使用 Provider 原生的 tool_use 特性，不做文本级 JSON 解析。

## Detailed Design

### 1. Core Types (adapter.rs)

新增统一的 Provider 响应类型：

```rust
/// Provider 返回的结构化响应
pub struct ProviderResponse {
    /// LLM 文本回复（Complete/Fail 等非工具场景）
    pub text: Option<String>,
    /// 原生工具调用
    pub tool_calls: Vec<NativeToolCall>,
    /// 思考/推理过程（Anthropic extended thinking / OpenAI reasoning）
    pub thinking: Option<String>,
    /// 停止原因
    pub stop_reason: StopReason,
    /// Token 用量
    pub usage: Option<TokenUsage>,
}

/// 原生工具调用
pub struct NativeToolCall {
    /// Provider 分配的 ID（用于 tool_result 回传）
    pub id: String,
    /// 工具名
    pub name: String,
    /// 参数 JSON
    pub arguments: Value,
}

/// 停止原因
pub enum StopReason {
    EndTurn,      // LLM 主动结束
    ToolUse,      // 需要执行工具
    MaxTokens,    // 达到 token 上限
    Unknown,      // 未知/不支持
}

/// Token 用量
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
}
```

RequestPayload 扩展：

```rust
pub struct RequestPayload<'a> {
    // ... existing fields unchanged ...

    /// Tool definitions for native tool_use
    pub tools: Option<&'a [ToolDefinition]>,
}
```

### 2. ProtocolAdapter Trait Changes

```rust
#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    fn build_request(&self, payload: &RequestPayload, config: &ProviderConfig, is_streaming: bool)
        -> Result<reqwest::RequestBuilder>;

    // Return type: String → ProviderResponse
    async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse>;

    // Streaming: String → StreamChunk
    async fn parse_stream(&self, response: reqwest::Response)
        -> Result<BoxStream<'static, Result<StreamChunk>>>;

    fn name(&self) -> &'static str;

    /// Whether this protocol supports native tool_use
    fn supports_native_tools(&self) -> bool { false }
}

pub enum StreamChunk {
    Text(String),
    ToolCallStart { id: String, name: String },
    ToolCallDelta { id: String, arguments_delta: String },
    ToolCallEnd { id: String },
    Thinking(String),
    Done(StopReason),
}
```

### 3. Virtual Tools for Non-Tool Decisions

Register Complete/AskUser/Fail as system tools so ALL decisions go through tool_use:

```rust
// System tools (not user-facing, always injected)
AnthropicTool { name: "complete", description: "Report task completion with summary",
    input_schema: json!({"type": "object", "properties": {
        "summary": {"type": "string", "description": "Task completion summary"}
    }, "required": ["summary"]}) }

AnthropicTool { name: "ask_user", description: "Ask the user a question",
    input_schema: json!({"type": "object", "properties": {
        "question": {"type": "string"},
        "options": {"type": "array", "items": {"type": "string"}}
    }, "required": ["question"]}) }

AnthropicTool { name: "fail", description: "Report task failure",
    input_schema: json!({"type": "object", "properties": {
        "reason": {"type": "string", "description": "Failure reason"}
    }, "required": ["reason"]}) }
```

### 4. Protocol Implementations

#### Anthropic Protocol

Request:
```rust
let request_body = MessagesRequest {
    model, messages, max_tokens, system, temperature, stream, thinking,
    tools: payload.tools.map(|t| t.iter().map(to_anthropic_tool).collect()),
};
```

Response parsing:
```rust
// content blocks: text, thinking, tool_use
for block in response.content {
    match block.type_ {
        "text" => provider_response.text = Some(block.text),
        "thinking" => provider_response.thinking = Some(block.thinking),
        "tool_use" => provider_response.tool_calls.push(NativeToolCall {
            id: block.id, name: block.name, arguments: block.input,
        }),
    }
}
provider_response.stop_reason = match response.stop_reason {
    "end_turn" => StopReason::EndTurn,
    "tool_use" => StopReason::ToolUse,
    "max_tokens" => StopReason::MaxTokens,
    _ => StopReason::Unknown,
};
```

#### OpenAI Protocol

Request:
```rust
body["tools"] = json!(payload.tools.map(|t| t.iter().map(to_openai_function).collect()));
```

Response parsing:
```rust
let choice = &response.choices[0];
provider_response.text = choice.message.content.clone();
if let Some(tool_calls) = &choice.message.tool_calls {
    for tc in tool_calls {
        provider_response.tool_calls.push(NativeToolCall {
            id: tc.id.clone(),
            name: tc.function.name.clone(),
            arguments: serde_json::from_str(&tc.function.arguments)?,
        });
    }
}
provider_response.stop_reason = match choice.finish_reason.as_str() {
    "stop" => StopReason::EndTurn,
    "tool_calls" => StopReason::ToolUse,
    "length" => StopReason::MaxTokens,
    _ => StopReason::Unknown,
};
```

#### Gemini Protocol

Request:
```rust
body["tools"] = json!([{
    "functionDeclarations": payload.tools.map(|t| t.iter().map(to_gemini_function).collect())
}]);
```

Response parsing:
```rust
for part in candidate.content.parts {
    if let Some(text) = part.text { provider_response.text = Some(text); }
    if let Some(fc) = part.function_call {
        provider_response.tool_calls.push(NativeToolCall {
            id: uuid(), name: fc.name, arguments: fc.args,
        });
    }
}
```

#### ChatGPT / Ollama (Fallback)

```rust
fn supports_native_tools(&self) -> bool { false }

async fn parse_response(&self, response: Response) -> Result<ProviderResponse> {
    let text = self.extract_text(response).await?;
    Ok(ProviderResponse {
        text: Some(text),
        tool_calls: vec![],
        thinking: None,
        stop_reason: StopReason::EndTurn,
        usage: None,
    })
}
```

### 5. Thinker Adaptation

```rust
// Thinker::think_with_level()
let response: ProviderResponse = provider.process_with_overrides(...).await?;

if !response.tool_calls.is_empty() {
    // Native tool_use path — direct mapping
    let tc = &response.tool_calls[0];

    // Map virtual tools to Decision variants
    let decision = match tc.name.as_str() {
        "complete" => Decision::Complete {
            summary: tc.arguments["summary"].as_str().unwrap_or("").to_string(),
        },
        "ask_user" => Decision::AskUser {
            question: tc.arguments["question"].as_str().unwrap_or("").to_string(),
            options: tc.arguments.get("options").and_then(|v| ...),
        },
        "fail" => Decision::Fail {
            reason: tc.arguments["reason"].as_str().unwrap_or("").to_string(),
        },
        _ => Decision::UseTool {
            tool_name: tc.name.clone(),
            arguments: tc.arguments.clone(),
        },
    };

    let reasoning = response.thinking.or(response.text).unwrap_or_default();
    Ok(Thinking { reasoning: Some(reasoning), decision, ... })
} else if let Some(text) = &response.text {
    // Fallback: JSON-in-text parsing (for providers without native tool_use)
    self.decision_parser.parse(text)
} else {
    Err(AlephError::Other { message: "Empty LLM response".into(), ... })
}
```

### 6. Prompt Layer Changes

**ToolsLayer** — conditional:
```rust
if prompt_config.native_tools_enabled {
    // Skip tool schema injection — tools passed via API
    // Optionally inject brief tool usage strategy guidance
    return;
}
// Else: existing Markdown schema injection (fallback)
```

**ResponseFormatLayer** — conditional:
```rust
if prompt_config.native_tools_enabled {
    // Skip JSON format instruction
    // LLM uses tool_use for ALL decisions (including complete/ask_user/fail)
    return;
}
// Else: existing JSON format instruction (fallback)
```

### 7. Tool Result Passback

Tool results must be formatted per Provider's native protocol:

**Anthropic**:
```json
{"role": "user", "content": [
    {"type": "tool_result", "tool_use_id": "toolu_xxx", "content": "search result..."}
]}
```

**OpenAI**:
```json
{"role": "tool", "tool_call_id": "call_xxx", "content": "search result..."}
```

**Gemini**:
```json
{"role": "function", "parts": [
    {"functionResponse": {"name": "search", "response": {"result": "..."}}}
]}
```

PromptBuilder::build_messages() needs protocol-awareness to format tool results correctly. This can be achieved by passing the protocol type or a formatting function.

### 8. Error Degradation

| Scenario | Behavior |
|----------|----------|
| Native tool_use success | Direct Decision mapping |
| Provider doesn't support tools | JSON-in-text fallback (existing path) |
| Native parse failure | Degrade to DecisionParser on text field |
| MaxTokens | Try truncated recovery (existing logic) |
| Empty response | Return error |

## File Changes

| File | Change | Impact |
|------|--------|--------|
| `providers/adapter.rs` | New types + RequestPayload extension | HIGH |
| `providers/mod.rs` | AiProvider returns ProviderResponse | HIGH |
| `providers/http_provider.rs` | Adapt to ProviderResponse | MEDIUM |
| `providers/protocols/anthropic.rs` | Native tool_use request/response | HIGH |
| `providers/protocols/openai.rs` | Native function calling request/response | HIGH |
| `providers/protocols/gemini.rs` | Native functionDeclarations | MEDIUM |
| `providers/protocols/chatgpt.rs` | Return ProviderResponse (text only) | LOW |
| `providers/anthropic/types.rs` | New tool types | MEDIUM |
| `providers/openai/types.rs` | New tool types | MEDIUM |
| `providers/ollama.rs` | Return ProviderResponse (text only) | LOW |
| `thinker/mod.rs` | Dual-path (native vs fallback) | HIGH |
| `thinker/layers/tools.rs` | Conditional injection | LOW |
| `thinker/layers/response_format.rs` | Conditional injection | LOW |
| `thinker/prompt_builder/messages.rs` | Protocol-aware tool result formatting | MEDIUM |
| `thinker/decision_parser.rs` | Retained as fallback only | NONE |
| `agent_loop/agent_loop.rs` | Record tool_call_id | LOW |
| `agent_loop/state.rs` | StepSummary + tool_call_id | LOW |
| `gateway/loop_callback_adapter.rs` | Adapt event types | LOW |

## Out of Scope

- Streaming tool_use (Phase 2 — current implementation is non-streaming)
- Multi-tool parallel execution (LLM returns multiple tool_calls simultaneously)
- Tool choice constraints (force specific tool use)
- Removing DecisionParser entirely (kept as fallback)

## Testing Plan

1. **Unit**: ProviderResponse parsing for each Protocol (mock API responses)
2. **Integration**: End-to-end tool call cycle with real API (Anthropic + OpenAI)
3. **Fallback**: Verify JSON-in-text fallback works for ChatGPT/Ollama
4. **Regression**: Existing agent loop tests pass with new types
5. **Virtual tools**: Complete/AskUser/Fail correctly mapped
6. **Edge cases**: MaxTokens, empty responses, malformed tool_calls

## Architecture Compliance

| Rule | Compliance |
|------|-----------|
| R1 (Brain-Limb Separation) | ✅ Changes within Core only |
| R3 (Core Minimalism) | ✅ No new dependencies |
| P1 (Low Coupling) | ✅ ProtocolAdapter trait preserved |
| P2 (High Cohesion) | ✅ Tool communication in Provider/Protocol layer |
| P3 (Extensibility) | ✅ New Provider auto-gets tool_use support |
| P4 (Dependency Inversion) | ✅ ProtocolAdapter abstracts provider differences |
| P6 (Simplicity) | ✅ Removes 650+ line parser from main path |
| P7 (Defensive Design) | ✅ Native → fallback degradation chain |
