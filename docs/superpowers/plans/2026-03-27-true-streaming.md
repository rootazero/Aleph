# True Streaming Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace fixed-interval streaming throttle with semantic-aware chunking, add delta/full_text protocol fields, and implement lightweight StreamingRenderer for WebChat.

**Architecture:** Three layers — (1) backend StreamingDeltaSink semantic flush, (2) ResponseChunk protocol extension with delta+full_text, (3) frontend StreamingRenderer that skips Markdown during streaming. Changes are strictly ordered: protocol first (types), then backend producer, then frontend consumer.

**Tech Stack:** Rust (core), Leptos/WASM (webchat), pulldown-cmark + syntect (markdown)

**Spec:** `docs/superpowers/specs/2026-03-27-true-streaming-design.md`

---

### Task 0: Dead Code Cleanup (prerequisite)

**Files:**
- Modify: `src/gateway/event_emitter/impls.rs` (lines 40-124)
- Modify: `src/gateway/event_emitter/tests.rs` (lines 87-155)

- [ ] **Step 1: Remove dead throttle code from GatewayEventEmitter**

In `src/gateway/event_emitter/impls.rs`, remove:
- The comment "Throttling state for response chunks (typewriter mode)" (line 31)
- `delta_buffer` and `last_delta_at` fields (lines 32-33)
- `DELTA_THROTTLE_MS` constant (line 42)
- The entire `emit_response_chunk_throttled()` method (lines 76-124)
- Their initialization in `new()` (lines 48-49) and `with_output_mode()` (lines 60-61)

After removal, `GatewayEventEmitter` becomes:

```rust
pub struct GatewayEventEmitter {
    pub(super) event_bus: Arc<GatewayEventBus>,
    pub(super) seq_counter: AtomicU64,
    // Instant mode buffer for accumulating all chunks
    pub(super) instant_buffer: Mutex<String>,
    /// Output mode: typewriter (streaming) or instant (all-at-once)
    pub(super) output_mode: OutputMode,
}
```

Update `new()` and `with_output_mode()` to remove `delta_buffer` and `last_delta_at` initialization.

- [ ] **Step 2: Remove dead throttle tests**

In `src/gateway/event_emitter/tests.rs`, remove these 3 tests that reference `emit_response_chunk_throttled` or `DELTA_THROTTLE_MS`:
- `test_throttled_response_chunk_buffering` (lines 87-123)
- `test_throttled_response_chunk_final_always_sends` (lines 125-146)
- `test_throttle_constant` (lines 148-155)

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS (no compilation errors)

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib event_emitter`
Expected: All remaining tests pass

- [ ] **Step 5: Commit**

```bash
git add src/gateway/event_emitter/impls.rs src/gateway/event_emitter/tests.rs
git commit -m "gateway: remove dead emit_response_chunk_throttled code"
```

---

### Task 1: Protocol — Add delta + full_text to ResponseChunk

**Files:**
- Modify: `src/gateway/event_emitter/types.rs` (lines 92-103, StreamEvent::ResponseChunk)
- Modify: `src/gateway/event_emitter/mod.rs` (lines 90-110, emit_response_chunk trait method)
- Modify: `src/gateway/event_emitter/impls.rs` (all 4 implementors + Instant mode)
- Modify: `src/gateway/event_emitter/tests.rs` (update assertions)

- [ ] **Step 1: Add fields to StreamEvent::ResponseChunk**

In `src/gateway/event_emitter/types.rs`, update the `ResponseChunk` variant (lines 92-103):

```rust
/// Response text chunk (streaming output)
ResponseChunk {
    run_id: String,
    seq: u64,
    /// Incremental text for this chunk
    delta: String,
    /// Accumulated complete text so far (within current iteration)
    full_text: String,
    /// Backward-compatible alias for delta (DEPRECATED — use delta instead)
    content: String,
    chunk_index: u32,
    is_final: bool,
    /// When true, send to user immediately as standalone message (intermediate progress).
    /// When false, buffer per existing behavior.
    #[serde(default)]
    is_intermediate: bool,
},
```

- [ ] **Step 2: Update emit_response_chunk trait method signature**

In `src/gateway/event_emitter/mod.rs`, update `emit_response_chunk()` (lines 90-110):

```rust
/// Emit response text chunk
async fn emit_response_chunk(
    &self,
    run_id: &str,
    delta: &str,
    full_text: &str,
    chunk_index: u32,
    is_final: bool,
    is_intermediate: bool,
) {
    let seq = self.next_seq();
    let _ = self
        .emit(StreamEvent::ResponseChunk {
            run_id: run_id.to_string(),
            seq,
            delta: delta.to_string(),
            full_text: full_text.to_string(),
            content: delta.to_string(), // backward compat
            chunk_index,
            is_final,
            is_intermediate,
        })
        .await;
}
```

- [ ] **Step 3: Update Instant mode in GatewayEventEmitter::emit()**

In `src/gateway/event_emitter/impls.rs`, update the `emit()` method's Instant mode handling. The key changes:

In the `StreamEvent::ResponseChunk` destructure (around line 132), add `delta` and `full_text`:
```rust
if let StreamEvent::ResponseChunk {
    ref content,
    ref delta,
    ref full_text,
    is_final,
    is_intermediate,
    ref run_id,
    ..
} = event
```

For the intermediate boundary flush (around line 148), construct with all fields:
```rust
let flush_event = StreamEvent::ResponseChunk {
    run_id: run_id.clone(),
    seq: self.next_seq(),
    delta: accumulated.clone(),
    full_text: accumulated.clone(),
    content: accumulated,
    chunk_index: 0,
    is_final: false,
    is_intermediate: true,
};
```

For non-final buffering (around line 171), buffer `delta` instead of `content`:
```rust
self.instant_buffer.lock().await.push_str(delta);
```

For the final chunk combine (around line 185), build with all fields:
```rust
let full_content = if buffer.is_empty() {
    delta.clone()
} else {
    let buffered = std::mem::take(&mut *buffer);
    format!("{}{}", buffered, delta)
};
drop(buffer);

let final_event = StreamEvent::ResponseChunk {
    run_id: run_id.clone(),
    seq: self.next_seq(),
    delta: full_content.clone(),
    full_text: full_content.clone(),
    content: full_content,
    chunk_index: 0,
    is_final: true,
    is_intermediate: false,
};
```

For the RunComplete flush (around line 209), same pattern — construct with all 3 text fields equal.

- [ ] **Step 4: Fix all compilation errors from new fields**

Search for all `StreamEvent::ResponseChunk { .. }` pattern matches across the codebase and add the new `delta` and `full_text` fields. Key locations:
- `src/gateway/streaming_sink.rs` — test matches (lines 174, 182, 238, 246, 274, 295)
- `src/gateway/event_emitter/tests.rs` — test matches
- Any other files found via: `grep -rn "ResponseChunk" src/`

For test pattern matches, add `delta, full_text, ..` to the destructure or `..` wildcard.

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib event_emitter && cargo test -p alephcore --lib streaming_sink`
Expected: All tests pass

- [ ] **Step 7: Commit**

```bash
git add src/gateway/event_emitter/ src/gateway/streaming_sink.rs
git commit -m "gateway: add delta + full_text fields to ResponseChunk protocol"
```

---

### Task 2: Backend — Semantic Chunking in StreamingDeltaSink

**Files:**
- Modify: `src/gateway/streaming_sink.rs` (full rewrite of flush logic)

- [ ] **Step 1: Write tests for semantic boundary detection**

Add to the `tests` module in `src/gateway/streaming_sink.rs`:

```rust
#[test]
fn test_semantic_boundary_detection() {
    // Paragraph break
    assert!(has_semantic_boundary("hello\n\n"));
    // Sentence endings
    assert!(has_semantic_boundary("hello. "));
    assert!(has_semantic_boundary("你好。"));
    assert!(has_semantic_boundary("what？"));
    assert!(has_semantic_boundary("wow！"));
    assert!(has_semantic_boundary("hello! "));
    assert!(has_semantic_boundary("what? "));
    // Code fence
    assert!(has_semantic_boundary("```rust\n"));
    assert!(has_semantic_boundary("some code\n```"));
    // Soft boundary (single newline) — NOT a hard boundary
    assert!(!has_semantic_boundary("hello\n"));
    // No boundary
    assert!(!has_semantic_boundary("hello world"));
    assert!(!has_semantic_boundary("hello"));
}

#[test]
fn test_soft_boundary_detection() {
    assert!(has_soft_boundary("hello\n"));
    assert!(!has_soft_boundary("hello world"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib streaming_sink::tests::test_semantic_boundary`
Expected: FAIL (functions not defined)

- [ ] **Step 3: Implement boundary detection functions**

Add to `src/gateway/streaming_sink.rs`, above the `StreamingDeltaSink` struct:

```rust
/// Hard flush timeout (ms) — force flush if no semantic boundary found
const FLUSH_TIMEOUT_MS: u64 = 50;

/// Soft boundary timeout (ms) — shorter timeout after single newline
const SOFT_BOUNDARY_TIMEOUT_MS: u64 = 30;

/// Check if buffer ends with a hard semantic boundary (immediate flush).
fn has_semantic_boundary(s: &str) -> bool {
    s.ends_with("\n\n")
        || s.ends_with(". ")
        || s.ends_with("。")
        || s.ends_with("？")
        || s.ends_with("！")
        || s.ends_with("! ")
        || s.ends_with("? ")
        || s.lines().last().map_or(false, |line| line.trim_start().starts_with("```"))
}

/// Check if buffer ends with a soft boundary (shorter timeout).
fn has_soft_boundary(s: &str) -> bool {
    s.ends_with('\n') && !s.ends_with("\n\n")
}
```

- [ ] **Step 4: Run boundary detection tests**

Run: `cargo test -p alephcore --lib streaming_sink::tests::test_semantic_boundary && cargo test -p alephcore --lib streaming_sink::tests::test_soft_boundary`
Expected: PASS

- [ ] **Step 5: Add accumulated field and update struct**

Update `StreamingDeltaSink` struct to add `accumulated`:

```rust
pub struct StreamingDeltaSink {
    run_id: String,
    emitter: Arc<dyn EventEmitter + Send + Sync>,
    chunk_index: AtomicU32,
    has_emitted_text: Arc<AtomicBool>,
    buffer: Mutex<String>,
    last_emit: Mutex<Instant>,
    /// Accumulated full text within the current iteration (reset on intermediate boundary)
    accumulated: Mutex<String>,
}
```

Update `new()` to initialize: `accumulated: Mutex::new(String::new()),`

- [ ] **Step 6: Rewrite on_delta with semantic flush logic**

Replace the `on_delta` implementation:

```rust
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

                // Determine flush strategy
                let should_flush = if has_semantic_boundary(&buf) {
                    true // Hard boundary: flush immediately
                } else if has_soft_boundary(&buf) {
                    elapsed >= SOFT_BOUNDARY_TIMEOUT_MS // Soft boundary: shorter timeout
                } else {
                    elapsed >= FLUSH_TIMEOUT_MS // No boundary: full timeout
                };

                if should_flush {
                    let content = std::mem::take(&mut *buf);
                    drop(buf);

                    let mut acc = self.accumulated.lock().await;
                    acc.push_str(&content);
                    let full_text = acc.clone();
                    drop(acc);

                    let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                    self.emitter.emit_response_chunk(
                        &self.run_id, &content, &full_text, idx, false, false,
                    ).await;
                    self.has_emitted_text.store(true, Ordering::Release);
                    *self.last_emit.lock().await = Instant::now();
                }
            }
            ProviderDelta::Done(stop_reason) => {
                let is_intermediate = matches!(stop_reason, StopReason::ToolUse);

                // Flush any remaining buffered text
                let mut buf = self.buffer.lock().await;
                let remaining = std::mem::take(&mut *buf);
                drop(buf);

                if !remaining.is_empty() {
                    let mut acc = self.accumulated.lock().await;
                    acc.push_str(&remaining);
                    let full_text = acc.clone();
                    drop(acc);

                    let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                    self.emitter.emit_response_chunk(
                        &self.run_id, &remaining, &full_text, idx, false, false,
                    ).await;
                    self.has_emitted_text.store(true, Ordering::Release);
                }

                let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                if is_intermediate {
                    // Get accumulated for the boundary marker
                    let acc = self.accumulated.lock().await;
                    let full_text = acc.clone();
                    drop(acc);

                    self.emitter.emit_response_chunk(
                        &self.run_id, "", &full_text, idx, false, true,
                    ).await;

                    // Reset accumulated for next iteration
                    self.accumulated.lock().await.clear();
                } else {
                    let acc = self.accumulated.lock().await;
                    let full_text = acc.clone();
                    drop(acc);

                    self.emitter.emit_response_chunk(
                        &self.run_id, "", &full_text, idx, true, false,
                    ).await;
                }
            }
            _ => {}
        }
    }
}
```

- [ ] **Step 7: Update existing tests for new emit_response_chunk signature**

Update test pattern matches in `streaming_sink::tests` to include `delta` and `full_text` fields. The key change is that `content` is now `delta` in the matches:

For `test_done_flushes_buffer_and_emits_final` (and similar tests), update patterns like:
```rust
// Old:
matches!(e, StreamEvent::ResponseChunk { content, is_final, .. } if !content.is_empty() && !*is_final)
// New:
matches!(e, StreamEvent::ResponseChunk { delta, is_final, .. } if !delta.is_empty() && !*is_final)
```

And for final markers:
```rust
// Old:
matches!(last, StreamEvent::ResponseChunk { content, is_final, .. } if *is_final && content.is_empty())
// New:
matches!(last, StreamEvent::ResponseChunk { delta, is_final, .. } if *is_final && delta.is_empty())
```

- [ ] **Step 8: Add test for accumulated reset on intermediate boundary**

```rust
#[tokio::test]
async fn test_accumulated_resets_on_intermediate() {
    let flag = Arc::new(AtomicBool::new(false));
    let (sink, emitter) = make_sink("run-acc", flag.clone());

    // Iteration 1
    sink.on_delta(&ProviderDelta::TextDelta("First part.".to_string())).await;
    sink.on_delta(&ProviderDelta::Done(StopReason::ToolUse)).await;

    // Iteration 2
    sink.on_delta(&ProviderDelta::TextDelta("Second part.".to_string())).await;
    sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn)).await;

    let events = emitter.events().await;

    // Find the text chunks and verify full_text resets between iterations
    let text_chunks: Vec<_> = events.iter().filter(|e| {
        matches!(e, StreamEvent::ResponseChunk { delta, .. } if !delta.is_empty())
    }).collect();

    assert!(text_chunks.len() >= 2, "Should have text chunks from both iterations");

    // First iteration's full_text should contain "First part."
    if let StreamEvent::ResponseChunk { full_text, .. } = &text_chunks[0] {
        assert!(full_text.contains("First part."), "First iteration full_text: {}", full_text);
    }

    // Second iteration's full_text should NOT contain "First part." (accumulated was reset)
    if let StreamEvent::ResponseChunk { full_text, .. } = text_chunks.last().unwrap() {
        assert!(!full_text.contains("First part."),
            "Second iteration full_text should not contain first iteration text: {}", full_text);
        assert!(full_text.contains("Second part."), "Second iteration full_text: {}", full_text);
    }
}
```

- [ ] **Step 9: Verify compilation and run all tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib streaming_sink`
Expected: All tests pass

- [ ] **Step 10: Commit**

```bash
git add src/gateway/streaming_sink.rs
git commit -m "gateway: semantic chunking in StreamingDeltaSink with accumulated text"
```

---

### Task 3: Fix All Callers of emit_response_chunk

**Files:**
- Modify: `src/gateway/handlers/agent.rs:242` — the only external caller

- [ ] **Step 1: Update handlers/agent.rs**

The only caller outside event_emitter and streaming_sink is in `src/gateway/handlers/agent.rs` at line 242. This is a one-shot message replay (non-streaming), so `delta == full_text`:

```rust
// Old (line 242):
.emit_response_chunk(&run_id, chunk, i as u32, is_final, false)
// New:
.emit_response_chunk(&run_id, chunk, chunk, i as u32, is_final, false)
```

Verify no other callers exist:
Run: `grep -rn "emit_response_chunk\b" src/ --include="*.rs" | grep -v event_emitter | grep -v streaming_sink | grep -v tests.rs`
Expected: Only `handlers/agent.rs:242`

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass (ignore pre-existing `markdown_skill::loader` failures)

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/agent.rs
git commit -m "gateway: update emit_response_chunk caller in agent handler"
```

---

### Task 4: Frontend — StreamingRenderer Component

**Files:**
- Modify: `interfaces/webchat/src/components/markdown.rs` — add `StreamingRenderer` component + `render_streaming()` function

- [ ] **Step 1: Add render_streaming function**

In `interfaces/webchat/src/components/markdown.rs`, add after `html_escape()`:

```rust
/// Lightweight streaming renderer — escapes HTML, tracks code fences, no Markdown parse.
///
/// Much cheaper than full Markdown: O(n) string scan with HTML escape only.
/// Used during streaming; replaced by full MarkdownRenderer on completion.
fn render_streaming(content: &str) -> String {
    let mut html = String::with_capacity(content.len() * 2);
    let mut in_fence = false;
    let mut fence_lang = String::new();

    for line in content.split('\n') {
        if line.starts_with("```") {
            if in_fence {
                // Close fence
                html.push_str("</code></pre></div>");
                in_fence = false;
            } else {
                // Open fence
                fence_lang = line.trim_start_matches('`').trim().to_string();
                let lang_label = if fence_lang.is_empty() { "code" } else { &fence_lang };
                html.push_str(&format!(
                    r#"<div class="code-block-wrapper relative group my-3"><div class="flex items-center justify-between px-3 py-1.5 bg-surface-sunken/50 rounded-t-lg border border-b-0 border-border text-xs text-text-tertiary"><span>{lang_label}</span></div><pre class="rounded-b-lg border border-border bg-surface-sunken overflow-x-auto p-3 text-sm leading-relaxed"><code>"#,
                ));
                in_fence = true;
            }
        } else if in_fence {
            html.push_str(&html_escape(line));
            html.push('\n');
        } else {
            // Plain text: escape and convert newlines
            html.push_str(&html_escape(line));
            html.push_str("<br>");
        }
    }

    // If still in fence (incomplete), close it
    if in_fence {
        html.push_str("</code></pre></div>");
    }

    html
}
```

- [ ] **Step 2: Add StreamingRenderer component**

```rust
/// A Leptos component for streaming content — lightweight rendering without full Markdown parse.
///
/// Tracks code fences and escapes HTML, but does not process Markdown syntax.
/// Switches to MarkdownRenderer on completion for full formatting.
#[component]
pub fn StreamingRenderer(content: String) -> impl IntoView {
    let html = render_streaming(&content);

    view! {
        <div class="markdown-body text-sm leading-relaxed streaming-content" inner_html=html />
    }
}
```

- [ ] **Step 3: Verify WASM compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown` (or the project's WASM build command)
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/markdown.rs
git commit -m "webchat: add StreamingRenderer component for lightweight streaming display"
```

---

### Task 5: Frontend — Wire Up Streaming Renderer

**Files:**
- Modify: `interfaces/webchat/src/views/chat/events.rs` — parse `delta` field
- Modify: `interfaces/webchat/src/views/chat/view.rs` — conditional rendering

**Note on state.rs:** The spec mentions adding `full_text: String` to `ChatMessage`. This is unnecessary — the frontend already accumulates text via `push_str(delta)` in `append_chunk()`, which produces the same result. The `full_text` from the protocol is a recovery mechanism (if delta was lost/reordered), but in WebSocket delivery order is guaranteed. No changes to `state.rs` needed.

**Note on is_final:** The spec says `is_final` triggers `is_streaming = false`. In practice, this is handled by the `run_complete` event (which calls `complete_run()`), not by `is_final` directly. This is correct — `run_complete` fires after all chunks including the final marker, providing a clean transition point.

- [ ] **Step 1: Update events.rs to use delta field**

In `interfaces/webchat/src/views/chat/events.rs`, update the `response_chunk` handler (lines 55-72):

```rust
"response_chunk" => {
    let is_intermediate = data.get("is_intermediate")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Prefer "delta" field, fall back to "content" for backward compat
    let chunk_text = data.get("delta")
        .or_else(|| data.get("content"))
        .and_then(|c| c.as_str());

    if is_intermediate {
        if let Some(text) = chunk_text {
            if !text.is_empty() {
                chat.append_chunk(run_id, text);
            }
        }
        chat.finalize_intermediate(run_id);
    } else if let Some(text) = chunk_text {
        chat.append_chunk(run_id, text);
    }
}
```

- [ ] **Step 2: Update view.rs MessageBubble to use conditional rendering**

In `interfaces/webchat/src/views/chat/view.rs`, add `StreamingRenderer` import at the top:

```rust
use crate::components::markdown::{MarkdownRenderer, StreamingRenderer};
```

Then update the content rendering section in `MessageBubble` (around lines 179-190):

```rust
// Message content — Markdown for assistant (full render when done, streaming render when streaming), plain text for user
{if is_user {
    view! {
        <div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
            {content.clone()}
        </div>
    }.into_any()
} else if is_streaming {
    view! {
        <StreamingRenderer content=content.clone() />
    }.into_any()
} else {
    view! {
        <MarkdownRenderer content=content.clone() />
    }.into_any()
}}
```

Also remove the separate `streaming_cursor` logic (lines 162-168) since `StreamingRenderer` is now the visual indicator. Replace the `{streaming_cursor}` usage with an inline cursor shown only during streaming outside the renderer:

```rust
// Streaming cursor (after StreamingRenderer)
{if is_streaming {
    Some(view! {
        <span class="inline-block w-1.5 h-4 bg-primary/60 animate-pulse ml-0.5 align-text-bottom"></span>
    })
} else {
    None
}}
```

- [ ] **Step 3: Verify WASM compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/chat/events.rs interfaces/webchat/src/views/chat/view.rs
git commit -m "webchat: wire StreamingRenderer for real-time streaming display"
```

---

### Task 6: Integration Verification

- [ ] **Step 1: Full backend test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass (ignore pre-existing `markdown_skill::loader` failures)

- [ ] **Step 2: Build WASM**

Run: `just build` (or the project's full build command that includes WASM)
Expected: PASS

- [ ] **Step 3: Manual verification**

Start the server and open WebChat. Send a message and observe:
1. Text appears incrementally (not all at once)
2. During streaming: plain text rendering, no Markdown formatting
3. After completion: full Markdown formatting appears (code blocks highlighted, lists formatted)
4. Blinking cursor visible during streaming
5. No visual jump/flicker on transition

- [ ] **Step 4: Final commit (if any fixups needed)**

```bash
git add -u
git commit -m "streaming: integration fixups"
```
