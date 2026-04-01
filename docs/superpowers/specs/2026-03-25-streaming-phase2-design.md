# End-to-End Streaming Phase 2 — Real-Time Token Delivery

**Date**: 2026-03-25
**Status**: Approved
**Scope**: StreamingDeltaSink + StreamCallback coordination + ExecutionEngine injection
**Depends on**: Provider Protocol Refactor Phase 1 (completed)

## Summary

Connect the `DeltaSink` pre-wire from Phase 1 to the existing Gateway event system, enabling real-time token-by-token text delivery from LLM providers to WebSocket clients.

**Phase 1** (done): `ProviderDelta` + `DeltaCollector` + `DeltaSink` trait + `NoopSink` default in AgentLoop
**Phase 2** (this spec): `StreamingDeltaSink` implementation + `StreamCallback` coordination

## Goals

1. **Real-time text streaming** — users see LLM text appear token-by-token instead of waiting for the complete response
2. **Zero duplication** — `StreamingDeltaSink` handles text deltas; `StreamCallback.on_text()` is silenced to avoid sending the same text twice
3. **Minimal invasion** — only 3 files changed (1 new, 2 modified); no changes to AgentLoop, Gateway, WebSocket handler, or frontend

## Non-Goals

- Thinking/reasoning real-time streaming (future work — needs conflict resolution with message system)
- Tool call real-time streaming (DeltaSink ignores ToolCall* events; existing `on_tool_start`/`on_tool_done` callbacks handle tools)
- Frontend/panel changes (existing `stream.response_chunk` handling already works)
- Cancellation or error recovery improvements

---

## Section 1: StreamingDeltaSink

**File**: `src/gateway/streaming_sink.rs` (new)

Bridges `ProviderDelta` events to the Gateway `EventEmitter` for real-time streaming. Created per-run by ExecutionEngine, injected into AgentLoop via `with_delta_sink()`.

```rust
use std::sync::atomic::{AtomicU32, Ordering};
use async_trait::async_trait;
use crate::providers::delta::{DeltaSink, ProviderDelta};
use crate::gateway::event_emitter::EventEmitter;
use crate::sync_primitives::Arc;

pub struct StreamingDeltaSink {
    run_id: String,
    emitter: Arc<dyn EventEmitter + Send + Sync>,
    chunk_index: AtomicU32,
}

impl StreamingDeltaSink {
    pub fn new(run_id: String, emitter: Arc<dyn EventEmitter + Send + Sync>) -> Self {
        Self {
            run_id,
            emitter,
            chunk_index: AtomicU32::new(0),
        }
    }
}

#[async_trait]
impl DeltaSink for StreamingDeltaSink {
    async fn on_delta(&self, delta: &ProviderDelta) {
        match delta {
            ProviderDelta::TextDelta(text) => {
                let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                let _ = self.emitter.emit_response_chunk(
                    &self.run_id,
                    text,
                    idx,
                    false, // is_final
                ).await;
            }
            ProviderDelta::Done(_) => {
                let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                let _ = self.emitter.emit_response_chunk(
                    &self.run_id,
                    "",
                    idx,
                    true, // is_final — marks this Think round's text as complete
                ).await;
            }
            // ThinkingDelta, ToolCall*, Usage, Error — not handled in Phase 2
            _ => {}
        }
    }
}
```

### Design Decisions

- **Location in gateway layer** — depends on `EventEmitter` (a gateway concept), not in providers
- **`let _ =` on emit** — broadcast channel full → drop is acceptable; built-in throttle controls flow
- **`chunk_index` never resets** — monotonically increases across all Think rounds within a run; client uses it for ordering
- **`Done` always emits final empty chunk** — even when there was no text (tool-call-only response); client ignores empty final chunks
- **Direct await, no spawn** — `DeltaSink::on_delta()` is async; emit is microsecond-level channel publish; no backpressure concern

---

## Section 2: StreamCallback Coordination

**File**: `src/gateway/execution_engine/run_loop.rs` (modify)

When `StreamingDeltaSink` is active, `StreamCallback` must not re-emit text that was already streamed token-by-token.

### Changes to StreamCallback

```rust
pub struct StreamCallback<E: EventEmitter> {
    // ... existing fields ...
    streaming_active: bool, // NEW
}
```

**Silenced methods when `streaming_active == true`**:

| Method | Behavior when streaming_active |
|--------|-------------------------------|
| `on_text()` | **Skip** ResponseChunk emission. Session persistence is handled by ExecutionEngine after loop completes (unchanged). |
| `on_intermediate_text()` | **Skip** — intermediate text was also streamed via DeltaSink. |
| `on_tool_start()` | **Unchanged** — DeltaSink does not handle tool events in Phase 2. |
| `on_tool_done()` | **Unchanged** |
| `on_safety_block()` | **Unchanged** |

### Constructor Change

```rust
impl<E: EventEmitter> StreamCallback<E> {
    pub fn new(
        emitter: Arc<E>,
        run_id: String,
        streaming_active: bool, // NEW parameter
    ) -> Self {
        Self {
            // ... existing fields ...
            streaming_active,
        }
    }
}
```

---

## Section 3: ExecutionEngine Injection

**File**: `src/gateway/execution_engine/run_loop.rs` (modify, same file as Section 2)

In `run_agent_loop()`, create `StreamingDeltaSink` and inject into AgentLoop:

```rust
// 1. Create streaming sink
let streaming_sink = StreamingDeltaSink::new(
    run_id.clone(),
    emitter.clone(),
);

// 2. Create callback with streaming_active = true
let mut callback = StreamCallback::new(
    emitter.clone(),
    run_id.clone(),
    true, // streaming_active
);

// 3. Inject into AgentLoop
let agent_loop = AgentLoop::new(provider, tools, safety)
    .with_delta_sink(Box::new(streaming_sink))
    // ... other existing config ...
    ;

// 4. Run as before (unchanged)
agent_loop.run_with_history(&input, &mut callback).await
```

---

## Section 4: Edge Cases

| Scenario | Handling |
|----------|----------|
| **auto-continue (MaxTokens retry)** | Each Think round produces its own delta stream. First `Done(MaxTokens)` emits `is_final:true`, second round's `TextDelta` continues with incrementing `chunk_index`. Client sees text append seamlessly. |
| **nudge (task-complete check fail)** | Nudge inserts user message, then re-enters Think. New delta stream pushes normally. |
| **Empty text (tool-call-only response)** | No `TextDelta` emitted. `Done` sends empty `is_final:true` chunk. Client ignores. |
| **Provider error (stream breaks)** | `ProviderDelta::Error` ignored by DeltaSink. `delta?` in AgentLoop propagates error. ExecutionEngine emits `RunError`. |
| **emit failure (channel full)** | `let _ =` discards error. Typewriter throttle already controls flow. |
| **Multiple Think rounds (tool → think → tool → think)** | `chunk_index` increments across all rounds. Each `Done` emits a final chunk. Client renders continuously. |

## Section 5: Files Changed

| File | Change |
|------|--------|
| `src/gateway/streaming_sink.rs` | **NEW** — `StreamingDeltaSink` impl |
| `src/gateway/mod.rs` | Add `pub mod streaming_sink;` |
| `src/gateway/execution_engine/run_loop.rs` | Add `streaming_active` to `StreamCallback`; create + inject `StreamingDeltaSink` in `run_agent_loop()` |

## Section 6: What Does NOT Change

- **AgentLoop** — already has `DeltaSink` support from Phase 1
- **ProviderDelta / DeltaCollector** — unchanged
- **Gateway WebSocket handler** — already pushes `stream.response_chunk` notifications
- **GatewayEventEmitter** — already has Typewriter mode with 150ms throttle
- **Frontend/Panel** — already handles `stream.response_chunk` events
- **Session persistence** — still happens in ExecutionEngine after loop completes
- **Tool events** — `on_tool_start` / `on_tool_done` callbacks unchanged

---

## Section 7: Review Fixes

Addressed from spec review feedback.

### Fix 1: System-generated truncation notice must not be silenced

`loop_core.rs:495` calls `callback.on_text(notice)` with a system-generated truncation warning (⚠️ 输出因 token 限制被截断). This text was never streamed via DeltaSink and must NOT be silenced.

**Solution**: `on_text()` silencing is conditional, not blanket. The `StreamCallback` tracks whether DeltaSink has emitted any text for the current Think round via a `streamed_text: bool` flag (set to true when `StreamingDeltaSink` emits at least one `TextDelta`).

```rust
fn on_text(&mut self, text: &str) {
    if self.streaming_active && self.streamed_text {
        // DeltaSink already delivered this text token-by-token.
        // Reset flag for next Think round.
        self.streamed_text = false;
        return;
    }
    // System-generated notice or non-streamed text: emit normally
    // ... existing ResponseChunk emission ...
}
```

To coordinate, `StreamingDeltaSink` needs to signal the callback. Implementation: use a shared `Arc<AtomicBool>` between `StreamingDeltaSink` and `StreamCallback`:

```rust
pub struct StreamingDeltaSink {
    run_id: String,
    emitter: Arc<dyn EventEmitter + Send + Sync>,
    chunk_index: AtomicU32,
    has_emitted_text: Arc<AtomicBool>,  // shared with StreamCallback
}

// In on_delta(TextDelta):
self.has_emitted_text.store(true, Ordering::Release);

// StreamCallback reads:
fn on_text(&mut self, text: &str) {
    if self.streaming_active && self.has_emitted_text.swap(false, Ordering::Acquire) {
        return; // Already streamed, skip
    }
    // Emit normally (system notice or fallback)
}
```

This way truncation notices and other system-generated `on_text()` calls are always delivered.

### Fix 2: Token-level throttling in StreamingDeltaSink

`emit_response_chunk()` goes through the `EventEmitter` trait, which does NOT throttle. The 150ms Typewriter throttle lives in `GatewayEventEmitter::emit_response_chunk_throttled()`, a concrete method not on the trait.

**Solution**: Add throttle logic directly in `StreamingDeltaSink`. Simple approach using `Instant` + buffer:

```rust
pub struct StreamingDeltaSink {
    // ... existing fields ...
    last_emit: Mutex<Instant>,
    buffer: Mutex<String>,
}

const THROTTLE_MS: u64 = 100; // slightly less than Typewriter's 150ms

async fn on_delta(&self, delta: &ProviderDelta) {
    match delta {
        ProviderDelta::TextDelta(text) => {
            let mut buf = self.buffer.lock().await;
            buf.push_str(text);

            let mut last = self.last_emit.lock().await;
            let elapsed = last.elapsed().as_millis() as u64;

            if elapsed >= THROTTLE_MS {
                // Flush buffer
                let content = std::mem::take(&mut *buf);
                drop(buf);
                *last = Instant::now();
                drop(last);

                let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                let _ = self.emitter.emit_response_chunk(&self.run_id, &content, idx, false).await;
                self.has_emitted_text.store(true, Ordering::Release);
            }
            // Within throttle window: buffer accumulates, flushed on next delta or Done
        }
        ProviderDelta::Done(_) => {
            // Flush remaining buffer
            let mut buf = self.buffer.lock().await;
            let remaining = std::mem::take(&mut *buf);
            drop(buf);

            if !remaining.is_empty() {
                let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                let _ = self.emitter.emit_response_chunk(&self.run_id, &remaining, idx, false).await;
                self.has_emitted_text.store(true, Ordering::Release);
            }

            // Emit final marker
            let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
            let _ = self.emitter.emit_response_chunk(&self.run_id, "", idx, true).await;
        }
        _ => {}
    }
}
```

**Use `tokio::sync::Mutex`** (not `std::sync::Mutex`) since this is in an async context.

### Fix 3: Multiple `is_final: true` per run

The spec's edge case table already documents this. The existing frontend (`interfaces/webchat/`) handles `stream.response_chunk` by appending content to the current message. An `is_final: true` with empty content is benign — it signals "this round done" but the `stream.run_complete` event signals "entire run done". No code change needed; adding explicit documentation for frontend developers:

**Convention**: `is_final: true` on `ResponseChunk` means "this LLM output segment is complete". Multiple segments may exist in a single run (tool calls between them). `stream.run_complete` means "entire agent run finished".
