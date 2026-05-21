# Agent Loop Tier 2 — Resilience & Observability

**Date:** 2026-04-01
**Scope:** `src/agent_loop/`
**Depends on:** Tier 1 (CompactionPipeline, PressureSensor, TruncationRecovery, ContextCompactor, StreamingToolBridge)

## Overview

Four features that make the agent loop more resilient to API failures and more observable to users:

| # | Feature | Priority | Batch |
|---|---------|----------|-------|
| 7 | Prompt-too-long (413) Recovery | Highest | 1 |
| 4 | Fallback Model | High | 1 |
| 5 | Stop Hooks | Medium | 2 |
| 6 | Tool Use Summary | Medium | 2 |

Batch 1 features are independent and can be developed in parallel. Batch 2 depends on Batch 1 being stable (shared error classification in `retry.rs`).

---

## Feature 7: Prompt-too-long (413) Reactive Recovery

### Problem

When API returns 413 / "prompt is too long", the loop crashes. The proactive `CompactionPipeline` prevents most cases, but edge cases (large tool outputs, image-heavy conversations) can still exceed the limit.

### Design

**Error classification — `retry.rs`**

Extend `RetryVerdict` with a new variant:

```rust
pub enum RetryVerdict {
    Retry { delay: Duration },
    Fatal,
    CompactAndRetry { token_gap: Option<usize> },  // NEW
}
```

`classify_error()` detects 413 status and "prompt is too long" messages. A helper `parse_token_gap()` extracts the actual-vs-limit difference from error text (e.g., "137500 tokens > 135000 maximum" → gap = 2500).

**Recovery loop — `loop_core.rs`**

Wrap the Think step in an inner retry loop:

```
let mut ptl_attempts = 0;
const MAX_PTL_RETRIES: usize = 3;

let delta_stream = loop {
    match retry_async(|| provider.stream(...), &cancel, 3).await {
        Ok(stream) => break stream,
        Err(e) => {
            let verdict = classify_error(&e);
            if let CompactAndRetry { token_gap } = verdict {
                ptl_attempts += 1;
                if ptl_attempts > MAX_PTL_RETRIES {
                    return Err(e);  // circuit breaker
                }
                emergency_truncate(&mut messages, token_gap, fresh_tail_count);
                continue;
            }
            // ... fallback / fatal handling (see Feature 4)
        }
    }
};
```

**Emergency truncation — `loop_core.rs` (private helper)**

```rust
fn emergency_truncate(
    messages: &mut Vec<UnifiedMessage>,
    token_gap: Option<usize>,
    fresh_tail_count: usize,
)
```

Strategy:
1. Group messages by API round (user → assistant → tool_results = one group)
2. Protect the last `fresh_tail_count` groups
3. If `token_gap` is known: drop oldest groups until estimated tokens freed >= gap × 1.2 (20% safety margin)
4. If `token_gap` is unknown: drop oldest 20% of groups
5. Insert marker at truncation point: `[earlier conversation truncated for recovery]`
6. Return early (no-op) if there aren't enough groups to drop

Token estimation uses the existing `PressureSensor::estimate_tokens()` char-ratio method (no new counting infrastructure needed).

### Constants

```rust
const MAX_PTL_RETRIES: usize = 3;
const PTL_SAFETY_MARGIN: f64 = 1.2;
const PTL_FALLBACK_DROP_RATIO: f64 = 0.20;
const PTL_TRUNCATION_MARKER: &str = "[earlier conversation truncated for recovery]";
```

### Not Changed

- `LoopProvider` trait signature unchanged
- `CompactionPipeline` stages unchanged (413 recovery uses independent group-drop logic)
- `PressureSensor` unchanged (reused for token estimation only)

---

## Feature 4: Fallback Model

### Problem

When the primary model is unavailable (overloaded, down, auth expired), the loop fails. Aleph's multi-provider architecture means a backup provider is often available.

### Design

**Error classification — `retry.rs`**

Add a `Fallback` variant:

```rust
pub enum RetryVerdict {
    Retry { delay: Duration },
    Fatal,
    CompactAndRetry { token_gap: Option<usize> },
    Fallback { reason: String },  // NEW
}
```

Classification rules (applied AFTER retry exhaustion, not on first error):

| Error | Condition | Verdict |
|-------|-----------|---------|
| 529 / overloaded | 3 consecutive | Fallback |
| Network errors | All retries exhausted | Fallback |
| 404 model not found | First occurrence | Fallback |
| 401 / 403 auth | First occurrence | Fallback (with notify flag) |
| 400 bad request | — | Fatal |
| 413 too long | — | CompactAndRetry |

Implementation: `classify_error()` inside `retry_async` continues to handle transient retries as before (Retry/Fatal). A separate function `classify_exhausted_error()` is called **only in the main loop** after `retry_async` returns its final `Err` — it reclassifies network/overload errors as `Fallback`. This keeps the retry layer simple and the fallback decision in the loop where provider switching happens.

**AgentLoop changes — `loop_core.rs`**

New fields:

```rust
pub struct AgentLoop<P: LoopProvider> {
    // ...existing...
    fallback_provider: Option<Box<dyn LoopProvider>>,
    fallback_label: Option<String>,  // e.g., "GPT-4o" for user notification
}
```

Builder method:

```rust
pub fn with_fallback(mut self, provider: Box<dyn LoopProvider>, label: String) -> Self {
    self.fallback_provider = Some(provider);
    self.fallback_label = Some(label);
    self
}
```

Main loop integration (in the Think step error handler, after 413 recovery):

```rust
Err(e) => {
    let verdict = classify_exhausted_error(&e);
    match verdict {
        Fallback { reason } if self.fallback_provider.is_some() && !fallback_active => {
            fallback_active = true;
            let label = self.fallback_label.as_deref().unwrap_or("fallback");
            callback.on_model_fallback(&reason, label);
            // Use fallback_provider for this and all subsequent iterations
            active_provider = self.fallback_provider.as_ref().unwrap().as_ref();
            continue;  // retry this iteration with fallback
        }
        _ => return Err(e),
    }
}
```

Provider selection uses an enum flag rather than a raw reference (avoids lifetime issues between `&self.provider` and `self.fallback_provider.as_ref()`):

```rust
enum ActiveProvider { Primary, Fallback }
let mut active = ActiveProvider::Primary;

// In Think step:
let provider: &dyn LoopProvider = match active {
    ActiveProvider::Primary => &self.provider,
    ActiveProvider::Fallback => self.fallback_provider.as_ref().unwrap().as_ref(),
};
```

**LoopCallback — new event**

```rust
fn on_model_fallback(&mut self, _reason: &str, _fallback_model: &str) {}
```

**Factory — optional injection**

```rust
// Caller assembles:
let loop_instance = LoopFactory::build(primary_provider, tools, soul, config)
    .with_fallback(Box::new(fallback_bridge), "gpt-4o".into());
```

### Key Decisions

- **Single fallback, no chain** — One fallback provider, no cascading. If fallback also fails, the error propagates. (P6 simplicity)
- **One-way switch** — Once fallback activates, it stays for the entire loop run. No switching back.
- **`Box<dyn LoopProvider>` not generic** — Avoids `AgentLoop<P, F>` type parameter explosion.
- **Auth errors fallback + notify** — 401/403 triggers fallback but also fires `on_model_fallback` with a reason indicating the user should fix their API key.

---

## Feature 5: Stop Hooks

### Problem

No mechanism to run external checks before the loop stops. Use cases: validation scripts, CI checks, notification triggers, quality gates.

### Design

**New file: `src/agent_loop/stop_hooks.rs`**

Types:

```rust
pub struct StopHook {
    pub name: String,
    pub command: String,
    pub timeout: Duration,  // default 30s
}

pub struct StopHookContext {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub stop_reason: String,  // "end_turn" | "max_tokens" | "max_iterations" | "tool_stop" | ...
}

pub enum StopHookVerdict {
    Allow,
    Block { reason: String },
    Error { hook_name: String, message: String },
}

pub struct StopHookAggregateResult {
    pub verdicts: Vec<StopHookVerdict>,
}

impl StopHookAggregateResult {
    /// Returns the first blocking reason, if any.
    pub fn blocking_reason(&self) -> Option<&str> { ... }
}
```

Execution function:

```rust
pub async fn execute_stop_hooks(
    hooks: &[StopHook],
    context: &StopHookContext,
    cancel: &CancellationToken,
) -> StopHookAggregateResult
```

Implementation:
1. Serialize `StopHookContext` to JSON
2. Execute all hooks in parallel via `tokio::process::Command` with stdin JSON
3. Each hook has independent timeout (`tokio::time::timeout`)
4. Collect exit codes: 0 = Allow, 2 = Block (stdout = reason), other = Error
5. Return aggregate result

**Exit code protocol:**

| Exit Code | Meaning | Loop Behavior |
|-----------|---------|---------------|
| 0 | Allow stop | Proceed to break |
| 2 | Block stop | Inject reason into messages, continue loop |
| Other | Hook error | Log warning, do not block |

**Integration — `loop_core.rs`**

New fields:

```rust
pub struct AgentLoop<P: LoopProvider> {
    // ...existing...
    stop_hooks: Vec<StopHook>,
}
```

At each `break` exit point (extracted to a helper to avoid duplication):

```rust
const MAX_STOP_HOOK_BLOCKS: usize = 3;

// Before breaking:
if !self.stop_hooks.is_empty() && stop_hook_blocks < MAX_STOP_HOOK_BLOCKS {
    let ctx = StopHookContext { final_text, iterations, tool_calls_made, stop_reason };
    let result = execute_stop_hooks(&self.stop_hooks, &ctx, &self.cancel_token).await;

    for verdict in &result.verdicts {
        if let StopHookVerdict::Error { hook_name, message } = verdict {
            callback.on_stop_hook_error(hook_name, message);
        }
    }

    if let Some(reason) = result.blocking_reason() {
        stop_hook_blocks += 1;
        callback.on_stop_hook_block(reason);
        messages.push(UnifiedMessage::user(&format!(
            "[SYSTEM] Stop hook blocked exit: {reason}. Address the issue and try again."
        )));
        continue;  // resume loop
    }
}
break;
```

**LoopCallback — new events**

```rust
fn on_stop_hook_block(&mut self, _reason: &str) {}
fn on_stop_hook_error(&mut self, _hook_name: &str, _error: &str) {}
```

### Key Decisions

- **Parallel execution** — All hooks run concurrently, first block wins
- **3-block limit** — Prevents hook ↔ LLM infinite loop
- **Only at break points** — Not every iteration, only when loop is about to stop
- **stdin/stdout protocol** — Simple, language-agnostic, no SDK needed for hook authors
- **Hook source is user config only** — `~/.aleph/hooks/` or injected via `with_stop_hooks()`, never from LLM output

---

## Feature 6: Tool Use Summary

### Problem

After tool execution, users see raw tool names but no human-readable summary of what was accomplished. Summaries enable better UX in chat interfaces (Telegram, Panel).

### Design

**New file: `src/agent_loop/tool_summary.rs`**

Types and function:

```rust
pub struct ToolSummaryInput {
    pub tool_name: String,
    pub tool_input: String,   // truncated to 300 chars
    pub tool_output: String,  // truncated to 300 chars
}

const SUMMARY_SYSTEM_PROMPT: &str = "\
You are a tool-use summarizer. Write a short summary (~30 chars) of what \
the tools accomplished. Use past tense, be specific. Examples:\n\
- \"Searched auth/ for login bugs\"\n\
- \"Read 3 config files\"\n\
- \"Created signup API endpoint\"";

const SUMMARY_INPUT_TRUNCATE: usize = 300;

pub async fn generate_tool_summary(
    provider: &dyn AiProvider,
    tools: &[ToolSummaryInput],
    last_assistant_text: Option<&str>,
) -> Option<String> {
    // 1. Build user prompt: tool entries + last_assistant_text (first 200 chars)
    // 2. Call provider.process() with system prompt
    // 3. Extract text from response
    // 4. Return Some(text) or None on any error (log with tracing::warn)
}

/// Truncate string to max chars at a char boundary (UTF-8 safe).
fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    // Find the last char boundary at or before `max`
    let end = s.char_indices()
        .take_while(|(i, _)| *i <= max)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0);
    &s[..end]
}
```

**Integration — `loop_core.rs`**

New field:

```rust
pub struct AgentLoop<P: LoopProvider> {
    // ...existing...
    summary_provider: Option<Arc<dyn AiProvider>>,
}
```

Fire-and-forget after tool execution:

```rust
// After tool batch execution completes:
let pending_summary: Option<JoinHandle<Option<String>>> =
    if let Some(ref sp) = self.summary_provider {
        let inputs = build_summary_inputs(&outcomes);
        let sp = sp.clone();
        let last_text = intermediate_texts.last().cloned();
        Some(tokio::spawn(async move {
            generate_tool_summary(&*sp, &inputs, last_text.as_deref()).await
        }))
    } else {
        None
    };

// At the start of next iteration, before Think step:
if let Some(handle) = pending_summary.take() {
    if let Ok(Some(summary)) = handle.await {
        callback.on_tool_summary(&summary);
    }
}
```

**LoopCallback — new event**

```rust
fn on_tool_summary(&mut self, _summary: &str) {}
```

### Key Decisions

- **Silent failure** — Any error in summary generation returns `None`, logged via `tracing::warn`. Never blocks the main loop.
- **Not injected into messages** — Summary is for UI display only, not added to conversation history (avoids context pollution).
- **`tokio::spawn` without cancellation** — Summary is a lightweight fire-and-forget task. Worst case: one wasted Haiku call on cancellation.
- **`Arc<dyn AiProvider>` not `Box<dyn LoopProvider>`** — Summary uses `process()` directly, not the streaming `stream()` interface.
- **Truncation before call** — Tool I/O capped at 300 chars each, last_assistant_text at 200 chars. Keeps summary request well under any model's context limit.

---

## New Files Summary

| File | Lines (est.) | Purpose |
|------|-------------|---------|
| `src/agent_loop/stop_hooks.rs` | ~150 | Stop hook execution and protocol |
| `src/agent_loop/tool_summary.rs` | ~100 | Tool use summary generation |

## Modified Files Summary

| File | Changes |
|------|---------|
| `src/agent_loop/retry.rs` | +`CompactAndRetry`, +`Fallback` variants, +`classify_exhausted_error()`, +`parse_token_gap()` |
| `src/agent_loop/loop_core.rs` | +413 recovery loop, +fallback switching, +stop hook checks, +summary fire-and-forget, +new AgentLoop fields |
| `src/agent_loop/mod.rs` | +`pub mod stop_hooks; pub mod tool_summary;` + re-exports |
| `src/agent_loop/factory.rs` | +optional fallback/summary provider injection |

## LoopCallback Additions

```rust
pub trait LoopCallback: Send {
    // ...existing 5 methods...
    fn on_model_fallback(&mut self, _reason: &str, _fallback_model: &str) {}
    fn on_stop_hook_block(&mut self, _reason: &str) {}
    fn on_stop_hook_error(&mut self, _hook_name: &str, _error: &str) {}
    fn on_tool_summary(&mut self, _summary: &str) {}
}
```

## Implementation Batches

**Batch 1 (parallel, no dependencies):**
- Feature 7: 413 Recovery — `retry.rs` + `loop_core.rs`
- Feature 4: Fallback Model — `retry.rs` + `loop_core.rs` + `factory.rs`

**Batch 2 (after Batch 1 stable):**
- Feature 5: Stop Hooks — `stop_hooks.rs` + `loop_core.rs`
- Feature 6: Tool Use Summary — `tool_summary.rs` + `loop_core.rs` + `factory.rs`
