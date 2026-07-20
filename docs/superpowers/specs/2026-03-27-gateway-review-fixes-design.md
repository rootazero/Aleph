# Gateway Review Fixes — 5 Important Issues

**Date**: 2026-03-27
**Status**: Approved
**Scope**: Fix 5 code review issues in openai_api gateway (agent.rs, passthrough.rs, responses/mod.rs)

## Background

Code review of the OpenAI Gateway implementation (Phase 1 + 2A) found 5 Important issues. Two Critical issues (auth on models, UTF-8 byte slice) were already fixed. These 5 remain:

1. Agent path discards conversation history — only uses last user message
2. Agent path `prompt_tokens`/`completion_tokens` always 0 (misleading)
3. Agent `ToolStart` params may be double-encoded
4. Passthrough `convert_messages` drops assistant `tool_calls` structure
5. Responses API ignores `tools`/`tool_choice` fields

All fixes are local to `openai_api/` — no cross-module type changes needed.

## Fix 1: Agent Session Seeding (agent.rs)

**Problem**: `agent.rs:278-284` extracts only the last user message. Stateless clients (Cursor, Continue) send full conversation history in every request — all prior turns are discarded.

**Fix**: Before building `RunRequest`, seed the agent's session with the conversation history (all messages except the last user message). The existing `AgentInstance.ensure_session()` and `AgentInstance.add_message()` methods handle persistence.

```rust
// Before building RunRequest, if client sent multi-turn history:
if req.messages.len() > 1 {
    agent.ensure_session(&session_key).await;
    for msg in &req.messages[..req.messages.len() - 1] {
        let role = match msg.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" | _ => continue,
        };
        if let Some(content) = &msg.content {
            agent.add_message(&session_key, role, content).await;
        }
    }
}
```

Check `AgentInstance::add_message` actual signature — may need `&SessionKey` or `SessionKey` by value. Adapt accordingly.

**Note**: Session seeding only happens when the session is empty or on first request. The `PerPeer` session key ensures subsequent requests from the same client reuse the session. If the session already has history, the seeding should be skipped to avoid duplicating messages. Check if there's a method like `get_history()` to detect existing messages.

## Fix 2: Usage Omit When No Breakdown (agent.rs)

**Problem**: `agent.rs:133-136` always emits `prompt_tokens: 0, completion_tokens: 0`. `RunSummary` only has `total_tokens: u64` — no prompt/completion split. Emitting 0 misleads clients that use these for cost tracking.

**Fix**: Omit `usage` entirely when only `total_tokens` is available and it's 0. When non-zero, include it with the total but note the split is unavailable:

```rust
// Streaming RunComplete:
usage: if summary.total_tokens > 0 {
    Some(Usage {
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: u32::try_from(summary.total_tokens).unwrap_or(u32::MAX),
    })
} else {
    None
}
```

Same pattern for non-streaming path.

## Fix 3: ToolStart Params Conditional Serialize (agent.rs)

**Problem**: `agent.rs:110` does `serde_json::to_string(&params)` where `params` is `Value`. If `params` is `Value::String` (already serialized JSON), `to_string()` double-encodes it.

**Fix**: Check the value type and handle accordingly:

```rust
let arguments = match &params {
    Value::String(s) => s.clone(),  // already serialized
    other => serde_json::to_string(other).unwrap_or_default(),
};
```

## Fix 4: Passthrough Assistant tool_calls (passthrough.rs)

**Problem**: `passthrough.rs:82-96` maps assistant messages as text-only, silently dropping `tool_calls`. This breaks multi-turn tool use — the provider receives empty assistant turns where tool calls should be.

**Fix**: In `convert_messages()`, when an assistant message has `tool_calls`, parse them into `ContentBlock::ToolCall` (which already exists in the enum):

```rust
"assistant" => {
    let mut content = Vec::new();
    if !content_text.is_empty() {
        content.push(ContentBlock::Text { text: content_text.to_string() });
    }
    if let Some(tool_calls) = &msg.tool_calls {
        for tc in tool_calls {
            if let (Some(id), Some(func)) = (
                tc.get("id").and_then(|v| v.as_str()),
                tc.get("function"),
            ) {
                content.push(ContentBlock::ToolCall {
                    id: id.to_string(),
                    name: func.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                    arguments: func.get("arguments")
                        .and_then(|a| a.as_str())
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(json!({})),
                });
            }
        }
    }
    Some(UnifiedMessage::Assistant { content })
}
```

No changes to `UnifiedMessage` or `ContentBlock` enums — `ContentBlock::ToolCall` already exists.

## Fix 5: Responses tools/tool_choice Forwarding (responses/mod.rs)

**Problem**: `responses/mod.rs:99-107` builds `RequestPayload` with `tools: None, tool_choice: None`, silently discarding client-provided tool definitions.

**Fix**: Reuse the conversion functions from `passthrough.rs`. Make `convert_openai_tools()` and `convert_tool_choice()` public, then call from responses handler:

```rust
// In passthrough.rs — change to pub:
pub fn convert_openai_tools(tools: &[Value]) -> Vec<ToolDefinition> { ... }
pub fn convert_tool_choice(choice: &Value) -> Option<ToolChoice> { ... }

// In responses/mod.rs handle():
use super::completions::passthrough::{convert_openai_tools, convert_tool_choice};

let tool_defs = req.tools.as_ref().map(|t| convert_openai_tools(t));
let tool_choice = req.tool_choice.as_ref().and_then(convert_tool_choice);

// Note: Responses API tools may use flat format { "type": "function", "name": "...", "parameters": {...} }
// while Chat Completions uses nested { "type": "function", "function": { "name": "..." } }.
// convert_openai_tools() handles the nested format. If flat format tools are received,
// they will be silently skipped (filter_map returns None). This is acceptable for Phase 2A —
// if real clients send flat format, add a fallback branch in convert_openai_tools that also
// checks for top-level name/parameters fields.

let payload = RequestPayload {
    tools: tool_defs.as_deref(),
    tool_choice,
    // ... rest unchanged
};
```

## Files Changed

| File | Fixes |
|------|-------|
| `completions/agent.rs` | Fix 1 (session seeding), Fix 2 (usage omit), Fix 3 (params encode) |
| `completions/passthrough.rs` | Fix 4 (assistant tool_calls), make convert functions `pub` |
| `responses/mod.rs` | Fix 5 (tools/tool_choice forwarding) |

## Acceptance Criteria

1. Agent path multi-turn conversations have context (messages seeded into session)
2. Usage field: present with total when available, omitted when zero (no fake 0/0/0)
3. ToolStart params never double-encoded (String values passed through, Objects serialized)
4. Passthrough multi-turn tool calling works (assistant tool_calls → ContentBlock::ToolCall)
5. Responses API forwards tools/tool_choice to provider
6. All 42 existing tests unaffected
