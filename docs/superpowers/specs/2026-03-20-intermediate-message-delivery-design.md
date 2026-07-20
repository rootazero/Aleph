# Intermediate Message Delivery Design

**Date**: 2026-03-20
**Status**: Approved
**Scope**: AgentLoop callback, StreamCallback, ReplyEmitter

## Problem

Aleph currently only sends a single final message to the user after the entire agent loop completes. During multi-step task execution (multiple tool calls across iterations), the user receives no intermediate feedback — they don't know what Aleph is doing, how far along it is, or whether it's still working. This creates a poor user experience, especially for complex tasks that take a long time.

## Root Cause

The `ReplyEmitter` buffers all `ResponseChunk` events (with `is_final: false`) and only flushes the accumulated text when `RunComplete` fires. Even though the AgentLoop correctly calls `callback.on_text()` during intermediate iterations, the text is silently absorbed into the buffer and never delivered until the run ends.

## Design Decision

**Approach A: Distinguish intermediate vs. final text at the LoopCallback level.**

The LLM naturally produces intermediate text when it explains what it's about to do before issuing tool calls. The system's job is simply to deliver that text immediately instead of buffering it. No template-based message generation — this respects the LLM Sovereignty principle (R8) and Intelligence Lives in the Prompt principle (R10).

## Changes

### 1. LoopCallback Trait (loop_core.rs)

Add `on_intermediate_text` method:

```rust
pub trait LoopCallback: Send {
    fn on_text(&mut self, _text: &str) {}
    fn on_intermediate_text(&mut self, _text: &str) {}  // NEW
    fn on_tool_start(&mut self, _name: &str, _input: &Value) {}
    fn on_tool_done(&mut self, _name: &str, _result: &ToolResult) {}
    fn on_safety_block(&mut self, _error: &SafetyError) {}
}
```

### 2. AgentLoop Text Processing (loop_core.rs)

Replace the undifferentiated `on_text` call with:

```rust
if let Some(text) = &response.text {
    if response.has_tool_calls() {
        callback.on_intermediate_text(text);
    } else {
        callback.on_text(text);
    }
    final_text = Some(text.clone());
}
```

- `has_tool_calls() == true` → LLM is still working → intermediate message
- `has_tool_calls() == false` → this is the final response

### 3. ResponseChunk Event (event_emitter/types.rs)

Add `is_intermediate` field to `ResponseChunk`:

```rust
ResponseChunk {
    run_id: String,
    seq: u64,
    content: String,
    chunk_index: u32,
    is_final: bool,
    #[serde(default)]
    is_intermediate: bool,
}
```

Field semantics:
- `is_intermediate: true` + `is_final: false` → send immediately as standalone message
- `is_intermediate: false` + `is_final: false` → buffer (existing behavior)
- `is_final: true` → flush buffer (existing behavior)

`#[serde(default)]` ensures backward compatibility — old clients deserialize `is_intermediate` as `false`.

### 4. StreamCallback (run_loop.rs)

Implement `on_intermediate_text`:

```rust
fn on_intermediate_text(&mut self, text: &str) {
    self.seq += 1;
    let chunk_index = self.chunk_index;
    self.chunk_index += 1;

    let event = StreamEvent::ResponseChunk {
        run_id: self.run_id.clone(),
        seq: self.seq,
        content: text.to_string(),
        chunk_index,
        is_final: false,
        is_intermediate: true,
    };

    let emitter = self.emitter.clone();
    tokio::spawn(async move {
        let _ = emitter.emit(event).await;
    });
}
```

### 5. ReplyEmitter (reply_emitter.rs)

Update `ResponseChunk` handling in `emit()`:

```rust
StreamEvent::ResponseChunk { content, is_final, is_intermediate, .. } => {
    if is_intermediate {
        if !content.is_empty() {
            self.send_to_channel(&content).await;
        }
    } else {
        if !content.is_empty() {
            self.buffer.lock().await.push_str(&content);
        }
        if !self.config.stream_enabled && is_final {
            let mut buffer = self.buffer.lock().await;
            if !buffer.is_empty() {
                let text = std::mem::take(&mut *buffer);
                drop(buffer);
                self.send_to_channel(&text).await;
            }
        }
    }
}
```

### 6. GatewayEventEmitter Instant Mode (event_emitter/impls.rs)

The `GatewayEventEmitter` has its own instant-mode buffering that merges non-final `ResponseChunk` events. Intermediate chunks must bypass this buffer and be emitted immediately to SSE/WebSocket clients, otherwise they would be silently swallowed. Update the instant-mode guard to check `is_intermediate` before buffering.

## What Does NOT Change

- **Session persistence** — Only the final assistant message is stored. Intermediate messages are fire-and-forget to the channel. No changes to `engine.rs` or `AgentInstance.add_message()`.
- **Session history / context window** — Intermediate messages never enter the LLM context window. No changes to `build_loop_history()`.
- **Panel / frontend** — `#[serde(default)]` on `is_intermediate` ensures backward compatibility.
- **send_tool** — Cross-session communication uses the same ExecutionEngine path, automatically gains intermediate message delivery.
- **Typewriter mode** — Still applies to the final message. Intermediate messages are sent instantly as standalone messages (short text, no need for progressive editing).

## Prompt Guidance (In Scope)

The code changes open the pipe for intermediate messages, but the LLM must actually produce text during intermediate iterations for anything to flow through. Without prompt guidance, most LLMs return only tool calls (no text) during intermediate iterations. This is addressed at two levels:

### Level 1: BASE_BEHAVIOR (prompt_builder.rs)

Add a behavioral rule to `BASE_BEHAVIOR` in `src/agent_loop/prompt_builder.rs`. This is hard-coded and loaded for ALL agents unconditionally:

```
- **KEEP THE USER INFORMED.** When you need to execute multiple steps, briefly tell the user
  what you're about to do before each tool call. Use natural, conversational language — do NOT
  expose raw tool names, parameters, or JSON. For example: "Let me check your calendar..." or
  "I'll search for that file now." This helps the user understand your progress and know you're
  still working.
```

### Level 2: SOUL.md Template and Existing Agents

Add a "Communication" section to the SOUL.md template (`default_soul()` in `src/config/agent_resolver.rs`) so new agents inherit this guidance from their identity:

```markdown
## Communication

**Show your work.** When tackling multi-step tasks, narrate your progress naturally — what you're
doing, what you found, what's next. Don't go silent for long stretches. The user should feel like
they're working _with_ you, not waiting _for_ you.
```

Update all existing agent SOUL.md files (`~/.aleph/agents/{main,codev,coding,cowork,agent-73f36847}/SOUL.md`) with the same section.

## Edge Cases

### SuccessAndStopLoop

When a tool returns `SuccessAndStopLoop`, the loop breaks immediately after the tool call. The text from that iteration's LLM response was already dispatched as `on_intermediate_text()` (because `has_tool_calls()` was true). Since no further LLM call happens, this "intermediate" text is effectively the last user-facing text. This works correctly — the text is delivered immediately via `send_to_channel()`, and the subsequent `RunComplete` with `final_response` handles persistence. No special handling needed.

### Persistence Nudge

When the persistence nudge fires (LLM returns text + EndTurn after errors), `on_text()` is called (final path) but the loop `continue`s. The pre-nudge text gets buffered, and post-nudge text also gets buffered/delivered. This is pre-existing behavior, not introduced by this design.

### Empty Text with Tool Calls

When LLM returns `text = Some("")` with tool calls, `on_intermediate_text("")` fires but the `if !content.is_empty()` guard in `ReplyEmitter` prevents sending an empty message. Handled correctly.

### Text is None with Tool Calls

The most common case — LLM returns only tool calls with no text. The `if let Some(text)` block doesn't execute, so no intermediate message is sent. This is expected. Prompt guidance is required to make LLMs produce text alongside tool calls.

### Typewriter Mode

Intermediate messages are always sent instantly via `send_to_channel()`, bypassing the typewriter progressive-edit path. Only the final message uses typewriter mode (if enabled). This is intentional — intermediate messages are typically short progress updates that don't benefit from progressive editing.

## Message Flow (After)

```
User sends message
    ↓
AgentLoop iteration 1:
    LLM returns: text="Let me look into that..." + tool_calls=[search_files]
    → on_intermediate_text("Let me look into that...")
    → StreamCallback emits ResponseChunk { is_intermediate: true }
    → ReplyEmitter.emit() → send_to_channel() immediately
    → User sees: "Let me look into that..."
    ↓
AgentLoop iteration 2:
    LLM returns: text="Found some results, analyzing..." + tool_calls=[read_file]
    → on_intermediate_text("Found some results, analyzing...")
    → User sees: "Found some results, analyzing..."
    ↓
AgentLoop iteration 3 (final):
    LLM returns: text="Here's what I found: ..." + no tool_calls (EndTurn)
    → on_text("Here's what I found: ...")
    → StreamCallback emits ResponseChunk { is_intermediate: false }
    → ReplyEmitter buffers, then flushes on RunComplete
    → User sees: "Here's what I found: ..."
```

## Files Modified

| File | Change |
|------|--------|
| `src/agent_loop/loop_core.rs` | Add `on_intermediate_text` to trait, change text dispatch logic |
| `src/gateway/event_emitter/types.rs` | Add `is_intermediate` field to `ResponseChunk` |
| `src/gateway/execution_engine/run_loop.rs` | Implement `on_intermediate_text` in `StreamCallback` |
| `src/gateway/reply_emitter.rs` | Handle `is_intermediate` in `emit()` |
| `src/gateway/event_emitter/impls.rs` | Bypass instant-mode buffering for `is_intermediate` chunks |
| `src/gateway/event_emitter/mod.rs` | Add `is_intermediate: false` to `emit_response_chunk()` helper |
| `src/gateway/execution_engine/simple.rs` | Add `is_intermediate: false` to constructions |
| `src/gateway/execution_engine/slash_command.rs` | Add `is_intermediate: false` to constructions |
| `src/agent_loop/prompt_builder.rs` | Add intermediate message guidance to `BASE_BEHAVIOR` |
| `src/config/agent_resolver.rs` | Add "Communication" section to `default_soul()` template |
| `~/.aleph/agents/*/SOUL.md` (5 files) | Add "Communication" section to existing agents |
