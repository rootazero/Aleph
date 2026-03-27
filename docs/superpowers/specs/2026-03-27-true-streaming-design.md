# True Streaming Design

**Date**: 2026-03-27
**Status**: Approved
**Scope**: WebChat real-time token streaming

## Problem

Aleph's current streaming has two issues degrading user experience:

1. **Dual throttling** — StreamingDeltaSink (100ms) + GatewayEventEmitter (150ms) stack up to 250ms worst-case delay before a chunk reaches the frontend
2. **Full Markdown re-parse** — Every incoming chunk triggers a complete `render_markdown()` + `inner_html` injection, increasingly expensive on long responses

The frontend's incremental append logic (`push_str`) is correct — the bottleneck is upstream.

## Reference

OpenClaw (~/Github/openclaw) implements true streaming with:
- Smart block chunking (paragraph > sentence > newline > word boundary)
- Both `text` (full) and `delta` (incremental) in each event frame
- Monotonic append guarantee

## Design

Three layers of changes, all focused on WebChat streaming path.

---

### Layer 1: Backend — Merge Throttling + Semantic Chunking

**Files:**
- `core/src/gateway/streaming_sink.rs` — rewrite flush logic
- `core/src/gateway/event_emitter/impls.rs` — remove secondary throttle

**Current:** Two independent throttle layers (100ms + 150ms).

**New:** Single-layer hybrid strategy in StreamingDeltaSink:

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
- `\n` — line break
- `. ` / `。` / `？` / `！` / `! ` / `? ` — sentence end
- ` ``` ` — code fence toggle

**GatewayEventEmitter change:**
- Remove `DELTA_THROTTLE_MS` constant and `emit_response_chunk_throttled()` method
- `emit_response_chunk()` becomes a direct pass-through to `event_bus.publish()`

---

### Layer 2: Protocol — ResponseChunk Delta + Full Text

**Files:**
- `core/src/gateway/event_emitter/types.rs` — add fields to ResponseChunk
- `core/src/gateway/streaming_sink.rs` — maintain accumulated text, pass both
- `core/src/gateway/event_emitter/mod.rs` — extend `emit_response_chunk()` signature
- `core/src/gateway/event_emitter/impls.rs` — serialize new fields

**New ResponseChunk structure:**

```rust
ResponseChunk {
    run_id: String,
    seq: u64,
    delta: String,          // incremental text (NEW)
    full_text: String,      // accumulated complete text (NEW)
    content: String,        // = delta, for backward compat (DEPRECATED)
    chunk_index: u32,
    is_final: bool,
    is_intermediate: bool,
}
```

**Accumulated text maintenance** in StreamingDeltaSink:

```rust
struct StreamingDeltaSink {
    // ... existing fields
    accumulated: String,    // NEW: tracks full text so far
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
```

**Backward compatibility:** `content` field is serialized as alias for `delta`. Old clients continue to work. Remove `content` in a future version.

---

### Layer 3: Frontend — Streaming Renderer

**Files:**
- `interfaces/webchat/src/views/chat/events.rs` — parse `delta` / `full_text` fields
- `interfaces/webchat/src/views/chat/state.rs` — use `delta` for append, store `full_text`
- `interfaces/webchat/src/views/chat/view.rs` — conditional rendering by streaming state
- `interfaces/webchat/src/components/markdown.rs` — new `StreamingRenderer` component

**Rendering state machine:**

```
is_streaming = true  →  StreamingRenderer (lightweight)
is_streaming = false →  MarkdownRenderer  (full parse)
```

**StreamingRenderer behavior:**

```
StreamingRenderer(content: &str):
  1. Scan lines, track whether inside code fence
  2. Outside fence: escape HTML, preserve line breaks, output as text
  3. Inside fence: wrap in <pre><code>, apply syntax highlight class
  4. Append blinking cursor at end
```

Each chunk arrival only processes the new delta, not the entire content.

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
  ├─ Detected: "\n" (semantic boundary)
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
  └─ StreamingRenderer updates (lightweight, no Markdown parse)
```

## Constants

| Constant | Value | Location |
|----------|-------|----------|
| `FLUSH_TIMEOUT_MS` | 50 | `streaming_sink.rs` |
| ~~`THROTTLE_MS`~~ | removed | `streaming_sink.rs` |
| ~~`DELTA_THROTTLE_MS`~~ | removed | `event_emitter/impls.rs` |

## Non-Goals

- ReplyEmitter channel streaming (Telegram, etc.) — separate effort
- Incremental Markdown parsing library — we use streaming/final toggle instead
- WebSocket binary frames — JSON text frames are sufficient
- Tool call streaming — existing intermediate message pattern is adequate

## Risks

1. **Visual flicker on transition** — StreamingRenderer → MarkdownRenderer may cause layout shift if Markdown produces significantly different layout. Mitigated by matching styles.
2. **Incomplete code fence** — If LLM is mid-fence when a chunk fires, StreamingRenderer must track fence state. Handled by line-scanning approach.
3. **full_text memory** — Accumulated string in StreamingDeltaSink grows with response. Acceptable — typical responses are < 10KB.
