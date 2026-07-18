# Media Attachment Infrastructure Design

**Date**: 2026-03-24
**Status**: Approved
**Scope**: Core media delivery infrastructure — generation tools, search/fetch, all channels

## Problem

All generation tools (image/video/audio/speech) currently return URLs as text. Users see a link they must click, not embedded media in the chat. Additionally, when Aleph searches for media (images, videos), there's no way to deliver the found media directly to users.

## Design Decisions

1. **Core-layer `_media` convention** — tools declare media via `_media` field in output JSON (like `_display`)
2. **Core downloads, fallback to URL** — `download_media()` downloads to temp file; on failure, degrades to URL-only
3. **Buffer and attach to final message** — media accumulated during run, delivered with LLM's text response
4. **URL + file both sent by default** — `Attachment` carries both `url` and `path`
5. **`media_send` tool for search scenarios** — LLM autonomously delivers found media
6. **Shared `Arc<Mutex<Vec<MediaItem>>>` for StreamCallback↔ReplyEmitter bridge** — injected at construction time

## Architecture

### Data Flow

```
Tool output JSON (with _media)
    ↓
StreamCallback::on_tool_done() — extract _media from ToolResult::Success { output: Value }
    ↓                             BEFORE output.to_string(), push to shared pending_media Arc
    ↓
LLM generates text response
    ↓
ReplyEmitter::drain_and_send_media() — drain pending_media, download files, populate attachments
    ↓
Channel::send(OutboundMessage { text, attachments }) — existing channel delivery
```

### New Types

```rust
// src/gateway/media.rs

/// Media item declared by tool output via `_media` convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    /// Remote URL, local file path, or data URL of the media
    pub url: String,
    /// Media type: "image", "video", "audio", "file"
    pub media_type: String,
    /// MIME type (auto-detected from URL/Content-Type if absent)
    pub mime_type: Option<String>,
    /// Optional filename
    pub filename: Option<String>,
}

/// Shared media buffer between StreamCallback and ReplyEmitter.
pub type PendingMedia = Arc<Mutex<Vec<MediaItem>>>;
```

### Component Changes

#### 1. Generation Tool Outputs (4 files)

Each generation tool output struct adds `_media: Vec<MediaItem>`:

```rust
pub struct ImageGenerateOutput {
    pub _display: String,
    pub _media: Vec<MediaItem>,  // NEW
    pub image_location: String,
    pub location_type: String,
    // ... rest unchanged
}
```

Populated at output construction:

```rust
_media: vec![MediaItem {
    url: image_location.clone(),
    media_type: "image".into(),
    mime_type: Some(detect_mime(&image_location, "image")),
    filename: None,
}],
```

MIME defaults: image → `image/png`, video → `video/mp4`, audio/speech → `audio/mpeg`.

When `location_type == "file"`, the downloader detects a local path and skips download, using `Attachment { path: Some(...) }` directly.

When `location_type == "data_url"`, the downloader detects `data:` scheme, base64-decodes the payload, writes to temp file, and extracts MIME from the data URL header.

#### 2. `media_send` Built-in Tool (new)

Lightweight tool for LLM-initiated media delivery (search/fetch scenarios):

```rust
/// Tool: media_send
/// Send media files (images, videos, audio) directly to the user.
///
/// Input:  { items: [{ url, media_type, mime_type?, filename? }] }
/// Output: { _media: [...items], _display: "📎 Sending N media files..." }
```

The tool performs no processing — it passes input items as `_media` output. Download and delivery handled by ReplyEmitter.

System prompt addition in `BASE_BEHAVIOR`:
```
When you find media (images, videos, audio) that the user wants,
use the media_send tool to deliver them directly in the chat.
```

#### 3. Core Media Downloader (reuse existing `media/cache.rs`)

**Key insight**: `src/media/cache.rs` already implements HTTP download, inline data handling, local path passthrough, and session-scoped cleanup. We extend `MediaCache` with a `download_media_item()` method rather than building a parallel downloader.

Existing infrastructure:
- `MediaCache::resolve()` — downloads `Attachment` to local file, returns `CachedMedia`
- Session-scoped temp dirs: `<temp_dir>/aleph/media/<session_id>/`
- `MediaCache::cleanup_session()` / `cleanup_stale()` — lifecycle management

New method:

```rust
// src/media/cache.rs — extend MediaCache

impl MediaCache {
    /// Convert a MediaItem (from tool _media output) to a channel Attachment.
    /// Downloads to temp file on success, falls back to URL-only on failure.
    pub async fn download_media_item(
        &self,
        item: &MediaItem,
        session_id: &str,
    ) -> Attachment {
        // IMPORTANT: data: URLs cannot go through resolve()'s URL path (reqwest
        // doesn't support data: scheme). Must be handled BEFORE calling resolve():
        //
        // 1. If item.url starts with "data:" → parse MIME from header, base64 decode
        //    body, build Attachment with data: Some(decoded_bytes), pass to resolve()
        //    via Priority 1 (inline bytes) path
        // 2. If item.url is a local file path → build Attachment with path: Some(...)，
        //    pass to resolve() via Priority 2 (local path) path
        // 3. Otherwise (HTTP/HTTPS URL) → build Attachment with url: Some(...),
        //    pass to resolve() via Priority 3 (URL download) path
        //
        // On Ok(cached) → Attachment { id: uuid, path: Some(cached.local_path),
        //                               url: Some(item.url), mime_type, ... }
        // On Err → Attachment { id: uuid, url: Some(item.url), path: None, ... } (fallback)
    }
}
```

All temp files go to the **same location** as inbound media cache: `<temp_dir>/aleph/media/<session_id>/`. Unified cleanup via existing `cleanup_session()` and `cleanup_stale()`.

Constants to adjust: `MAX_FILE_SIZE` raised from 20MB to 50MB (video files), `DOWNLOAD_TIMEOUT` from 30s to 60s (large media).

MIME detection priority: `MediaItem.mime_type` > HTTP `Content-Type` header > URL extension > `application/octet-stream`.

#### 4. Shared PendingMedia: StreamCallback ↔ ReplyEmitter Bridge

**Problem**: `StreamCallback` holds `Arc<E: EventEmitter>`, which is a trait object — it cannot access `ReplyEmitter`-specific fields like `pending_media`.

**Solution**: Shared `Arc<Mutex<Vec<MediaItem>>>` injected into both `StreamCallback` and `ReplyEmitter` at construction time.

```rust
// run_loop.rs
pub(super) struct StreamCallback<E: EventEmitter + Send + Sync + 'static> {
    emitter: Arc<E>,
    run_id: String,
    seq: u64,
    chunk_index: u32,
    pending_media: PendingMedia,  // NEW — shared with ReplyEmitter
}

// reply_emitter.rs
pub struct ReplyEmitter {
    // ... existing fields ...
    pending_media: PendingMedia,  // NEW — shared with StreamCallback
}
```

Construction in `RunLoop::execute()`:
```rust
let pending_media: PendingMedia = Arc::new(Mutex::new(Vec::new()));
let emitter = ReplyEmitter::new(..., pending_media.clone());
let callback = StreamCallback::new(emitter, run_id, pending_media.clone());
```

#### 5. StreamCallback `_media` Extraction

In `StreamCallback::on_tool_done()` (run_loop.rs), extraction happens on the **original `serde_json::Value`** from `ToolResult::Success { output }`, BEFORE the `output.to_string()` serialization:

```rust
fn on_tool_done(&mut self, name: &str, result: &crate::agent_loop::ToolResult) {
    // NEW: extract _media from raw Value before serialization
    if let crate::agent_loop::ToolResult::Success { output }
        | crate::agent_loop::ToolResult::SuccessAndStopLoop { output } = result
    {
        if let Some(items) = output.get("_media")
            .and_then(|v| serde_json::from_value::<Vec<MediaItem>>(v.clone()).ok())
        {
            let mut pending = self.pending_media.lock().unwrap_or_else(|e| e.into_inner());
            // Enforce max 10 media items per run
            let remaining = 10usize.saturating_sub(pending.len());
            pending.extend(items.into_iter().take(remaining));
        }
    }

    // Existing: emit ToolEnd event (output.to_string() happens here)
    // ...
}
```

#### 6. ReplyEmitter Media Delivery

New fields and method:

```rust
pub struct ReplyEmitter {
    // ... existing fields ...
    pending_media: PendingMedia,        // NEW — shared with StreamCallback
    media_cache: MediaCache,            // NEW — injected at construction, reuses HTTP connection pool
}

impl ReplyEmitter {
    /// Drain pending media, download in parallel via MediaCache, return Attachments.
    async fn drain_and_send_media(&self) -> Vec<Attachment> {
        let media_items = std::mem::take(
            &mut *self.pending_media.lock().unwrap_or_else(|e| e.into_inner())
        );
        if media_items.is_empty() {
            return vec![];
        }

        let session_id = &self.run_id;
        futures::future::join_all(
            media_items.iter().map(|item| self.media_cache.download_media_item(item, session_id))
        ).await
    }

    /// Send media as a separate standalone message (used by voice and AskUser paths).
    async fn send_media_standalone(&self, attachments: Vec<Attachment>) {
        if attachments.is_empty() { return; }
        let message = OutboundMessage {
            conversation_id: self.route.conversation_id.clone(),
            text: String::new(),
            attachments,
            reply_to: None,
            inline_keyboard: None,
            metadata: Default::default(),
        };
        // Send via channel_registry (same pattern as existing send_to_channel)
    }
}
```

**Unified media delivery strategy across ALL paths**:

All paths use the same approach: **send media as a separate message after the text/voice message**. This is the simplest strategy that works uniformly — no need to modify `send_to_channel` signature or chunk-splitting logic.

**`send_to_channel()`** — media sent as standalone message after text:
```rust
// After all text chunks are sent:
let media_attachments = self.drain_and_send_media().await;
self.send_media_standalone(media_attachments).await;
```

**`send_as_voice()`** — media sent as standalone message after voice:
```rust
// After voice OutboundMessage is sent:
let media_attachments = self.drain_and_send_media().await;
self.send_media_standalone(media_attachments).await;
```

**`send_typewriter()`** — media sent as standalone message after typewriter completes:
```rust
// After final edit step:
let media_attachments = self.drain_and_send_media().await;
self.send_media_standalone(media_attachments).await;
```

**`AskUser` path** — drain media before sending the question to user:
```rust
// In StreamEvent::AskUser handler, before sending the question:
let media_attachments = self.drain_and_send_media().await;
self.send_media_standalone(media_attachments).await;
// Then send the AskUser prompt as usual
```

This ensures media generated before an `AskUser` interruption is delivered immediately, not lost until `RunComplete`.

### Temp File Management (unified with existing `media/cache.rs`)

- **Location**: `<temp_dir>/aleph/media/<session_id>/` (same as inbound media cache)
- **Lifecycle**: download → channel send → `MediaCache::cleanup_session(session_id)` after run completes
- **Startup cleanup**: existing `MediaCache::cleanup_stale()` handles crash residue (removes dirs older than 1 hour)
- **No new temp directory** — all media temp files share one location, one cleanup mechanism

### Constraints

| Constraint | Value | Reason |
|-----------|-------|--------|
| Max file size | 50MB | Telegram single-file limit |
| Download timeout | 60s | Large video files |
| Max media per run | 10 | Enforced in StreamCallback; excess items silently dropped with warn log |

### Error Handling

**All errors degrade gracefully, never block message delivery.**

- Download fails → `Attachment { url only }` → channel tries URL
- Channel URL send fails → text message delivered normally, user sees URL in text
- MIME detection fails → `application/octet-stream` → channel sends as file
- data_url decode fails → fallback to URL-only Attachment
- Temp file delete fails → warn log, no retry

### YAGNI — Not Building

- No persistent media cache — temp files deleted after use
- No media format conversion — deliver as-is
- No thumbnail generation — platforms handle this
- No media deduplication — frequency too low to optimize

## Files to Create/Modify

| Action | File | Description |
|--------|------|-------------|
| Create | `src/gateway/media.rs` | `MediaItem`, `PendingMedia` type alias |
| Create | `src/builtin_tools/media_send.rs` | `media_send` tool |
| Modify | `src/media/cache.rs` | Add `download_media_item()` method, raise `MAX_FILE_SIZE` to 50MB, timeout to 60s |
| Modify | `src/builtin_tools/generation/image_generate.rs` | Add `_media` to output |
| Modify | `src/builtin_tools/generation/video_generate.rs` | Add `_media` to output |
| Modify | `src/builtin_tools/generation/audio_generate.rs` | Add `_media` to output |
| Modify | `src/builtin_tools/generation/speech_generate.rs` | Add `_media` to output |
| Modify | `src/gateway/reply_emitter.rs` | Add `pending_media` field + `drain_and_send_media()` + inject in all 3 paths |
| Modify | `src/gateway/execution_engine/run_loop.rs` | Add `pending_media` to `StreamCallback`, extract `_media` in `on_tool_done` |
| Modify | `src/agent_loop/prompt_builder.rs` | Add `media_send` guidance to BASE_BEHAVIOR |
| Modify | `src/executor/builtin_registry/groups.rs` | Register `media_send` in tool groups |
| Modify | `src/gateway/mod.rs` | Export `media` module |
