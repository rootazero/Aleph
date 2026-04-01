# End-to-End Streaming Phase 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable real-time token-by-token text delivery from LLM providers to WebSocket clients by connecting Phase 1's DeltaSink to the Gateway event system.

**Architecture:** `StreamingDeltaSink` implements `DeltaSink`, bridges `ProviderDelta::TextDelta` → `EventEmitter::emit_response_chunk()` with 100ms throttle buffering. A shared `Arc<AtomicBool>` coordinates with `StreamCallback` to prevent text duplication. ExecutionEngine creates both and injects into AgentLoop.

**Tech Stack:** Rust, async-trait, tokio (Mutex, AtomicBool, AtomicU32, Instant), futures

**Spec:** `docs/superpowers/specs/2026-03-25-streaming-phase2-design.md`

---

## File Structure

### New Files
| File | Purpose |
|------|---------|
| `src/gateway/streaming_sink.rs` | `StreamingDeltaSink` — bridges ProviderDelta to EventEmitter with throttle |

### Modified Files
| File | Changes |
|------|---------|
| `src/gateway/mod.rs` | Add `pub mod streaming_sink;` after line 89 |
| `src/gateway/execution_engine/run_loop.rs` | Add `streaming_active` + `has_emitted_text` to `StreamCallback`; create + inject `StreamingDeltaSink` in `run_agent_loop()` |

---

## Task 1: StreamingDeltaSink Implementation

**Files:**
- Create: `src/gateway/streaming_sink.rs`
- Modify: `src/gateway/mod.rs` — add `pub mod streaming_sink;`

- [ ] **Step 1: Write tests for StreamingDeltaSink**

Create `src/gateway/streaming_sink.rs` with tests at the bottom. Use a `CollectingEventEmitter` (or build a minimal mock) to verify emit behavior.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::delta::ProviderDelta;
    use crate::providers::adapter::StopReason;
    use std::sync::atomic::AtomicBool;

    // Test 1: TextDelta within throttle window is buffered, not emitted immediately
    #[tokio::test]
    async fn test_text_delta_buffered_within_throttle() {
        let has_emitted = Arc::new(AtomicBool::new(false));
        let emitter = /* CollectingEventEmitter or mock */;
        let sink = StreamingDeltaSink::new(
            "run_1".into(),
            Arc::new(emitter),
            has_emitted.clone(),
        );
        // Send a single small TextDelta
        sink.on_delta(&ProviderDelta::TextDelta("Hi".into())).await;
        // Within 100ms throttle — should be buffered, not emitted yet
        // (verify emitter has 0 events OR check buffer state)
    }

    // Test 2: Done flushes buffer and emits final chunk
    #[tokio::test]
    async fn test_done_flushes_buffer_and_emits_final() {
        let has_emitted = Arc::new(AtomicBool::new(false));
        let emitter = /* CollectingEventEmitter */;
        let sink = StreamingDeltaSink::new("run_1".into(), Arc::new(emitter.clone()), has_emitted.clone());
        sink.on_delta(&ProviderDelta::TextDelta("Hello ".into())).await;
        sink.on_delta(&ProviderDelta::TextDelta("world".into())).await;
        sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn)).await;
        // Verify: emitter received at least one chunk with "Hello world" content
        // Verify: last chunk has is_final=true and empty content
        // Verify: has_emitted is true
    }

    // Test 3: has_emitted_text flag is set on first TextDelta emit
    #[tokio::test]
    async fn test_has_emitted_text_flag_set() {
        let has_emitted = Arc::new(AtomicBool::new(false));
        let emitter = /* CollectingEventEmitter */;
        let sink = StreamingDeltaSink::new("run_1".into(), Arc::new(emitter), has_emitted.clone());
        assert!(!has_emitted.load(Ordering::Acquire));
        // Force a flush by waiting past throttle or sending Done
        sink.on_delta(&ProviderDelta::TextDelta("test".into())).await;
        sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn)).await;
        assert!(has_emitted.load(Ordering::Acquire));
    }

    // Test 4: Non-text deltas are ignored
    #[tokio::test]
    async fn test_non_text_deltas_ignored() {
        let has_emitted = Arc::new(AtomicBool::new(false));
        let emitter = /* CollectingEventEmitter */;
        let sink = StreamingDeltaSink::new("run_1".into(), Arc::new(emitter.clone()), has_emitted.clone());
        sink.on_delta(&ProviderDelta::ThinkingDelta("thought".into())).await;
        sink.on_delta(&ProviderDelta::ToolCallStart { id: "c1".into(), name: "search".into() }).await;
        // Verify: emitter has 0 events, has_emitted still false
    }

    // Test 5: chunk_index increments correctly
    #[tokio::test]
    async fn test_chunk_index_increments() {
        // Send multiple TextDeltas with sleep(>100ms) between to force flushes
        // Verify chunk indices are 0, 1, 2, ...
    }
}
```

Note: If `CollectingEventEmitter` exists (check `src/gateway/event_emitter/`), use it. Otherwise create a minimal test helper.

- [ ] **Step 2: Implement StreamingDeltaSink**

```rust
//! Real-time streaming bridge: ProviderDelta → EventEmitter
//!
//! Created per-run by ExecutionEngine, injected into AgentLoop via with_delta_sink().
//! Buffers text deltas within a 100ms throttle window for smooth delivery.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::gateway::event_emitter::EventEmitter;
use crate::providers::delta::{DeltaSink, ProviderDelta};
use crate::sync_primitives::Arc;

/// Throttle interval for text delta emission (100ms)
const THROTTLE_MS: u64 = 100;

pub struct StreamingDeltaSink {
    run_id: String,
    emitter: Arc<dyn EventEmitter + Send + Sync>,
    chunk_index: AtomicU32,
    /// Shared with StreamCallback to coordinate text deduplication.
    /// Set to true when at least one TextDelta has been emitted.
    /// StreamCallback reads and resets this via swap(false).
    has_emitted_text: Arc<AtomicBool>,
    /// Buffer for accumulating text within throttle window
    buffer: Mutex<String>,
    /// Last time a chunk was actually emitted
    last_emit: Mutex<Instant>,
}

impl StreamingDeltaSink {
    pub fn new(
        run_id: String,
        emitter: Arc<dyn EventEmitter + Send + Sync>,
        has_emitted_text: Arc<AtomicBool>,
    ) -> Self {
        Self {
            run_id,
            emitter,
            chunk_index: AtomicU32::new(0),
            has_emitted_text,
            buffer: Mutex::new(String::new()),
            last_emit: Mutex::new(Instant::now()),
        }
    }

    /// Flush the buffer and emit a response chunk
    async fn flush_buffer(&self, is_final: bool) {
        let mut buf = self.buffer.lock().await;
        let content = std::mem::take(&mut *buf);
        drop(buf);

        if !content.is_empty() {
            let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
            let _ = self.emitter.emit_response_chunk(&self.run_id, &content, idx, false).await;
            self.has_emitted_text.store(true, Ordering::Release);
            *self.last_emit.lock().await = Instant::now();
        }

        if is_final {
            let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
            let _ = self.emitter.emit_response_chunk(&self.run_id, "", idx, true).await;
        }
    }
}

#[async_trait]
impl DeltaSink for StreamingDeltaSink {
    async fn on_delta(&self, delta: &ProviderDelta) {
        match delta {
            ProviderDelta::TextDelta(text) => {
                let mut buf = self.buffer.lock().await;
                buf.push_str(text);

                let last = self.last_emit.lock().await;
                let elapsed = last.elapsed().as_millis() as u64;
                drop(last);

                if elapsed >= THROTTLE_MS {
                    let content = std::mem::take(&mut *buf);
                    drop(buf);

                    let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                    let _ = self.emitter.emit_response_chunk(&self.run_id, &content, idx, false).await;
                    self.has_emitted_text.store(true, Ordering::Release);
                    *self.last_emit.lock().await = Instant::now();
                }
                // Within throttle window: buffer accumulates
            }
            ProviderDelta::Done(_) => {
                self.flush_buffer(true).await;
            }
            // ThinkingDelta, ToolCall*, Usage, Error — not handled in Phase 2
            _ => {}
        }
    }
}
```

- [ ] **Step 3: Add module to gateway/mod.rs**

In `src/gateway/mod.rs`, add after line 89 (`pub mod media;`):
```rust
pub mod streaming_sink;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib streaming_sink -- --nocapture`
Expected: All tests pass.

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore`
Expected: Compiles (new module, no existing code changed).

- [ ] **Step 6: Commit**

```bash
git add src/gateway/streaming_sink.rs src/gateway/mod.rs
git commit -m "gateway: add StreamingDeltaSink with throttle buffering"
```

---

## Task 2: StreamCallback Coordination

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs`
  - `StreamCallback` struct (line ~313): add `streaming_active: bool`, `has_emitted_text: Arc<AtomicBool>`
  - `StreamCallback::new()` (line ~322): add parameters
  - `on_text()` (line ~336): conditional skip
  - `on_intermediate_text()` (line ~359): conditional skip

- [ ] **Step 1: Add fields to StreamCallback struct**

At `run_loop.rs` line ~313, add two fields:

```rust
pub(super) struct StreamCallback<E: EventEmitter + Send + Sync + 'static> {
    emitter: Arc<E>,
    run_id: String,
    seq: u64,
    chunk_index: u32,
    pending_media: PendingMedia,
    streaming_active: bool,                    // NEW
    has_emitted_text: Arc<AtomicBool>,         // NEW — shared with StreamingDeltaSink
}
```

Add import at top of file:
```rust
use std::sync::atomic::{AtomicBool, Ordering};
```

- [ ] **Step 2: Update constructor**

At `StreamCallback::new()` (line ~322), add parameters:

```rust
pub(super) fn new(
    emitter: Arc<E>,
    run_id: String,
    pending_media: PendingMedia,
    streaming_active: bool,                      // NEW
    has_emitted_text: Arc<AtomicBool>,           // NEW
) -> Self {
    Self {
        emitter,
        run_id,
        seq: 0,
        chunk_index: 0,
        pending_media,
        streaming_active,
        has_emitted_text,
    }
}
```

- [ ] **Step 3: Update on_text() with conditional skip**

At `on_text()` (line ~336), add early return at the top:

```rust
fn on_text(&mut self, text: &str) {
    // If streaming is active and DeltaSink already delivered this text,
    // skip to avoid duplication. System-generated notices (truncation warning)
    // were never sent through DeltaSink, so has_emitted_text will be false for them.
    if self.streaming_active && self.has_emitted_text.swap(false, Ordering::Acquire) {
        return;
    }
    // ... existing ResponseChunk emission code unchanged ...
}
```

- [ ] **Step 4: Update on_intermediate_text() with conditional skip**

At `on_intermediate_text()` (line ~359), add the same early return:

```rust
fn on_intermediate_text(&mut self, text: &str) {
    if self.streaming_active && self.has_emitted_text.swap(false, Ordering::Acquire) {
        return;
    }
    // ... existing code unchanged ...
}
```

- [ ] **Step 5: Fix the StreamCallback creation site**

At line ~192 where `StreamCallback::new()` is called, update to pass the new parameters. For now, pass `false` and a dummy `Arc<AtomicBool>` to keep it compiling (Task 3 will wire the real values):

```rust
let has_emitted_text = Arc::new(AtomicBool::new(false));
let mut callback = StreamCallback::new(
    emitter.clone(),
    run_id.to_string(),
    request.pending_media.clone(),
    false,                    // streaming_active — wired in Task 3
    has_emitted_text.clone(), // shared with DeltaSink — wired in Task 3
);
```

- [ ] **Step 6: Compile and test**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`
Expected: All tests pass. Behavior unchanged (streaming_active=false preserves original path).

- [ ] **Step 7: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs
git commit -m "gateway: add streaming_active flag to StreamCallback for dedup coordination"
```

---

## Task 3: ExecutionEngine Injection — Wire Everything Together

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs`
  - Where AgentLoop is constructed (line ~169) and callback created (line ~192)

- [ ] **Step 1: Import StreamingDeltaSink**

At the top of `run_loop.rs`, add:
```rust
use crate::gateway::streaming_sink::StreamingDeltaSink;
```

- [ ] **Step 2: Create shared AtomicBool and StreamingDeltaSink**

At the `run_agent_loop()` function, before the callback creation (line ~192), create the shared flag and sink:

```rust
// Shared flag for DeltaSink ↔ StreamCallback coordination
let has_emitted_text = Arc::new(AtomicBool::new(false));

// Real-time streaming sink: ProviderDelta → EventEmitter
let streaming_sink = StreamingDeltaSink::new(
    run_id.to_string(),
    emitter.clone(),
    has_emitted_text.clone(),
);
```

- [ ] **Step 3: Update StreamCallback creation**

Change the callback creation to enable streaming:

```rust
let mut callback = StreamCallback::new(
    emitter.clone(),
    run_id.to_string(),
    request.pending_media.clone(),
    true,                     // streaming_active — DeltaSink handles text delivery
    has_emitted_text.clone(), // shared with StreamingDeltaSink
);
```

- [ ] **Step 4: Inject DeltaSink into AgentLoop**

Find where `AgentLoop` is constructed (line ~169). Add `.with_delta_sink()`:

```rust
let agent_loop = AgentLoop::new(/* existing args */)
    .with_delta_sink(Box::new(streaming_sink))
    // ... other existing config (with_tool_compactor, etc.) ...
    ;
```

**IMPORTANT**: The `.with_delta_sink()` call must come BEFORE the builder is consumed by `.run_with_history()`. Check the existing builder chain and insert appropriately.

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore`
Expected: Compiles.

- [ ] **Step 6: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass.

- [ ] **Step 7: Build release**

Run: `cargo build --release --bin aleph-server 2>&1 | tail -3`
Expected: Builds successfully.

- [ ] **Step 8: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs
git commit -m "gateway: wire StreamingDeltaSink into ExecutionEngine for real-time token streaming"
```

---

## Summary

| Task | Description | Files | Complexity |
|------|-------------|-------|------------|
| 1 | StreamingDeltaSink implementation | 2 (1 new, 1 mod) | Medium |
| 2 | StreamCallback dedup coordination | 1 | Low |
| 3 | ExecutionEngine wiring | 1 | Low |

### Task Dependencies

```
1 → 2 → 3
```

Strictly sequential — each depends on the previous.
