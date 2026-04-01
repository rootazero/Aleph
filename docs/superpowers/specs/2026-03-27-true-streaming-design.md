# True Streaming Design

**Date**: 2026-03-27
**Status**: Approved
**Scope**: WebChat real-time token streaming

## Problem

Aleph's current streaming has two issues degrading user experience:

1. **Coarse fixed-interval throttling** — StreamingDeltaSink uses a 100ms fixed timer to buffer and flush text deltas. This ignores content semantics: chunks may split mid-word or mid-Markdown syntax, and the 100ms interval is not optimized for perceived responsiveness.
2. **Full Markdown re-parse** — Every incoming chunk triggers a complete `render_markdown()` + `inner_html` injection, increasingly expensive on long responses.

Note: `GatewayEventEmitter` has an `emit_response_chunk_throttled()` method (150ms) but it is **dead code** — only called from tests. The production path uses the `EventEmitter` trait's `emit_response_chunk()` which passes through directly. This dead code should be cleaned up separately.

The frontend's incremental append logic (`push_str`) is correct — the bottleneck is upstream throttling and downstream rendering.

## Reference

OpenClaw (~/Github/openclaw) implements true streaming with:
- Smart block chunking (paragraph > sentence > newline > word boundary)
- Both `text` (full) and `delta` (incremental) in each event frame
- Monotonic append guarantee

## Design

Three layers of changes, all focused on WebChat streaming path.

---

### Layer 1: Backend — Semantic Chunking

**Files:**
- `src/gateway/streaming_sink.rs` — rewrite flush logic with semantic boundary detection
- `src/gateway/event_emitter/impls.rs` — remove dead `emit_response_chunk_throttled()` and `DELTA_THROTTLE_MS` (cleanup)

**Current:** Fixed 100ms interval timer in StreamingDeltaSink.

**New:** Hybrid strategy — semantic boundary triggers immediate flush, 50ms timeout as fallback:

```
on_delta(TextDelta(text)):
  buffer.push_str(text)

  if contains_semantic_boundary(&buffer):   // immediate flush
    flush_buffer()
  elif elapsed_since_last_flush > 50ms:     // timeout flush
    flush_buffer()
  // else: keep accumulating

on_delta(Done(_)):
  flush_buffer()                            // drain remaining
```

**Semantic boundaries** (simplified from BlockReplyChunker):
- `\n\n` — paragraph break
- `. ` / `。` / `？` / `！` / `! ` / `? ` — sentence end
- `` ``` `` — code fence toggle

Note: Single `\n` alone does **not** trigger immediate flush (too aggressive for lists/code). It triggers a shorter timeout (30ms) instead of the full 50ms, as a soft boundary.

**Dead code cleanup** (separate prerequisite commit):
- Remove `DELTA_THROTTLE_MS` constant from `impls.rs`
- Remove `emit_response_chunk_throttled()` method from `GatewayEventEmitter`
- Update affected tests

---

### Layer 2: Protocol — ResponseChunk Delta + Full Text

**Files:**
- `src/gateway/event_emitter/types.rs` — add `delta` and `full_text` fields to `StreamEvent::ResponseChunk`
- `src/gateway/streaming_sink.rs` — maintain `accumulated` text, pass both fields; **reset `accumulated` on `Done(ToolUse)`** for intermediate boundaries
- `src/gateway/event_emitter/mod.rs` — extend `emit_response_chunk()` trait method signature (adds `delta: &str`, `full_text: &str`)
- `src/gateway/event_emitter/impls.rs` — update all 4 implementors:
  - `GatewayEventEmitter` — serialize new fields, update `Instant` mode buffering to use `delta`/`full_text`
  - `NoOpEventEmitter` — signature update (no-op body)
  - `CollectingEventEmitter` — store new fields in collected events
  - `DynEventEmitter` — delegate new params
- `src/gateway/event_emitter/tests.rs` — update test assertions

**StreamEvent::ResponseChunk new structure:**

```rust
ResponseChunk {
    run_id: String,
    seq: u64,
    delta: String,          // incremental text this chunk (NEW)
    full_text: String,      // accumulated text so far (NEW)
    content: String,        // = delta, backward compat (DEPRECATED)
    chunk_index: u32,
    is_final: bool,
    is_intermediate: bool,
}
```

JSON serialization emits all three text fields. `content` = `delta` for backward compatibility. Old clients that only read `content` continue to work unchanged.

**Accumulated text maintenance** in StreamingDeltaSink:

```rust
struct StreamingDeltaSink {
    // ... existing fields
    accumulated: String,    // NEW: tracks full text within current iteration
}

fn flush_buffer(&mut self) {
    let delta = self.buffer.drain();
    self.accumulated.push_str(&delta);
    self.emitter.emit_response_chunk(
        delta: &delta,
        full_text: &self.accumulated,
        // ...
    );
}

// On intermediate boundary (tool use), reset accumulated
fn on_delta(Done(ToolUse)):
    flush_buffer()
    self.accumulated.clear()   // reset for next iteration
    // emit intermediate boundary marker as before
```

**Instant mode update** in GatewayEventEmitter:
- Instant mode buffers `delta` values (instead of `content`)
- On flush, concatenates buffered deltas and emits with correct `full_text`
- Intermediate boundary handling unchanged

---

### Layer 3: Frontend — Streaming Renderer

**Files:**
- `interfaces/webchat/src/views/chat/events.rs` — parse `delta` field (fallback to `content` for compat), store `full_text` on message
- `interfaces/webchat/src/views/chat/state.rs` — `append_chunk` uses `delta` for `push_str`; add `full_text: String` to `ChatMessage` for completion rendering
- `interfaces/webchat/src/views/chat/view.rs` — conditional rendering by streaming state
- `interfaces/webchat/src/components/markdown.rs` — new `StreamingRenderer` component

**Rendering state machine:**

```
is_streaming = true  →  StreamingRenderer (lightweight)
is_streaming = false →  MarkdownRenderer  (full parse, uses full_text)
```

**StreamingRenderer — DOM update strategy:**

StreamingRenderer receives the full `content` string (accumulated via `push_str`) but does NOT parse Markdown. Instead:

```rust
#[component]
pub fn StreamingRenderer(content: String) -> impl IntoView {
    // Lightweight: scan lines for code fence state, escape HTML,
    // wrap code blocks in <pre><code>, output everything else as text.
    // This is O(n) string scan, much cheaper than full Markdown parse.
    let html = render_streaming(&content);
    view! {
        <div class="markdown-body text-sm leading-relaxed streaming-content" inner_html=html />
        <span class="inline-block w-1.5 h-4 bg-primary/60 animate-pulse ml-0.5 align-text-bottom"></span>
    }
}
```

The `render_streaming()` function:
1. Scans lines, tracking code fence open/close state
2. Outside fence: escapes HTML entities (`<>&"`), converts `\n` to `<br>`, outputs as plain text spans
3. Inside fence: wraps in `<pre><code class="language-{lang}">`, escapes HTML
4. Cost: simple string scan + escape, no AST construction or tree walking

On each chunk, Leptos re-renders `StreamingRenderer` with the updated full content. This is a full re-render but with a O(n) string scan instead of O(n) Markdown parse + tree construction, which is significantly cheaper.

**Completion transition:**

```rust
// view.rs conditional rendering
if msg.is_streaming {
    view! { <StreamingRenderer content=msg.content.clone() /> }
} else {
    view! { <MarkdownRenderer content=msg.content.clone() /> }
}
```

When `is_final = true` → `is_streaming` becomes `false` → Leptos reactivity triggers one full Markdown render, replacing the streaming view.

**Visual continuity:**
- StreamingRenderer uses identical font, font-size, line-height as MarkdownRenderer
- Code fence blocks share the same background color and monospace font
- No animation on transition — content position stays stable

---

## Data Flow (After)

```
LLM emits TextDelta("hello ")
  ↓
StreamingDeltaSink.on_delta()
  ├─ buffer += "hello "
  ├─ No semantic boundary, timer not expired
  └─ (wait)

LLM emits TextDelta("world.\n")
  ↓
StreamingDeltaSink.on_delta()
  ├─ buffer += "world.\n"
  ├─ Detected: ".\n" (sentence end + newline → semantic boundary)
  ├─ accumulated += "hello world.\n"
  └─ emit_response_chunk(delta="hello world.\n", full_text="hello world.\n")
  ↓
GatewayEventEmitter.emit() — direct pass-through
  ↓
EventBus.publish(JSON-RPC notification)
  ↓
WebSocket → Frontend
  ├─ Parse delta field
  ├─ msg.content.push_str(delta)
  └─ StreamingRenderer re-renders (lightweight string scan, no Markdown parse)
```

## Constants

| Constant | Value | Location |
|----------|-------|----------|
| `FLUSH_TIMEOUT_MS` | 50 | `streaming_sink.rs` |
| `SOFT_BOUNDARY_TIMEOUT_MS` | 30 | `streaming_sink.rs` (for single `\n`) |
| ~~`THROTTLE_MS`~~ | replaced by above | `streaming_sink.rs` |
| ~~`DELTA_THROTTLE_MS`~~ | removed (dead code) | `event_emitter/impls.rs` |

## Affected EventEmitter Implementors

| Implementor | File | Change |
|-------------|------|--------|
| `GatewayEventEmitter` | `impls.rs` | Serialize `delta`+`full_text`, update Instant mode buffering |
| `NoOpEventEmitter` | `impls.rs` | Signature update only |
| `CollectingEventEmitter` | `impls.rs` | Store new fields in collected events |
| `DynEventEmitter` | `impls.rs` | Delegate new params |

## Non-Goals

- ReplyEmitter channel streaming (Telegram, etc.) — separate effort
- Incremental Markdown parsing library — we use streaming/final toggle instead
- WebSocket binary frames — JSON text frames are sufficient
- Tool call streaming — existing intermediate message pattern is adequate

## Risks

1. **Visual flicker on transition** — StreamingRenderer → MarkdownRenderer may cause layout shift if Markdown produces significantly different layout. Mitigated by matching styles.
2. **Incomplete code fence** — If LLM is mid-fence when a chunk fires, StreamingRenderer must track fence state. Handled by line-scanning approach.
3. **full_text memory** — Accumulated string in StreamingDeltaSink grows with response. Acceptable — typical responses are < 10KB. Resets on intermediate boundaries.
4. **Re-render cost** — StreamingRenderer still does full content scan on each chunk. For very long responses (>50KB), this could become noticeable. If needed, a future optimization could cache the scan state and only process the new delta. Not in scope for this iteration.
