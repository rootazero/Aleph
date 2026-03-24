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

## Architecture

### Data Flow

```
Tool output JSON (with _media)
    ↓
StreamCallback::on_tool_done() — extract _media, push to pending_media buffer
    ↓
LLM generates text response
    ↓
ReplyEmitter::send_to_channel() — drain pending_media, download files, populate attachments
    ↓
Channel::send(OutboundMessage { text, attachments }) — existing channel delivery
```

### New Types

```rust
// core/src/gateway/media.rs

/// Media item declared by tool output via `_media` convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    /// Remote URL or local path of the media
    pub url: String,
    /// Media type: "image", "video", "audio", "file"
    pub media_type: String,
    /// MIME type (auto-detected from URL/Content-Type if absent)
    pub mime_type: Option<String>,
    /// Optional filename
    pub filename: Option<String>,
}
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

#### 3. Core Media Downloader (new)

```rust
// core/src/gateway/media.rs

/// Download a MediaItem to a local temp file.
/// Returns Attachment with path+url on success, url-only on failure.
pub async fn download_media(
    item: &MediaItem,
    http_client: &reqwest::Client,
) -> Attachment {
    // 1. If item.url is a local path, skip download
    // 2. Create temp file in ~/.aleph/tmp/media/{uuid}.{ext}
    // 3. Download with 60s timeout, 50MB size limit
    // 4. On success: Attachment { path: Some(temp), url: Some(item.url), mime_type, ... }
    // 5. On failure: Attachment { url: Some(item.url), path: None, ... } (fallback)
}
```

MIME detection priority: `MediaItem.mime_type` > HTTP `Content-Type` header > URL extension > `application/octet-stream`.

#### 4. StreamCallback `_media` Extraction

In `StreamCallback::on_tool_done()` (run_loop.rs):

```rust
fn on_tool_done(&mut self, name: &str, result: &ToolResult) {
    // Existing: emit ToolEnd event ...

    // NEW: extract _media from tool output, push to emitter's pending_media
    if let ToolResult::Success { output } | ToolResult::SuccessAndStopLoop { output } = result {
        if let Some(items) = output.get("_media")
            .and_then(|v| serde_json::from_value::<Vec<MediaItem>>(v.clone()).ok())
        {
            // Push to emitter.pending_media
        }
    }
}
```

#### 5. ReplyEmitter Integration

New field:

```rust
pub struct ReplyEmitter {
    // ... existing fields ...
    pending_media: Mutex<Vec<MediaItem>>,  // NEW
}
```

In `send_to_channel()`:

```rust
async fn send_to_channel(&self, content: &str) {
    // ... existing text chunking logic ...

    // NEW: drain pending media, download in parallel, build attachments
    let media_items = std::mem::take(&mut *self.pending_media.lock().await);
    let attachments = futures::future::join_all(
        media_items.iter().map(|item| download_media(item, &http_client))
    ).await;

    // Attach to the LAST chunk (or first if single chunk)
    let message = OutboundMessage {
        text: chunk,
        attachments,  // was vec![]
        // ...
    };
}
```

Media attaches to the **final message** only, not intermediate messages.

### Temp File Management

- **Location**: `~/.aleph/tmp/media/{uuid}.{ext}`
- **Lifecycle**: download → channel send → immediate delete (fire-and-forget `tokio::fs::remove_file`)
- **Startup cleanup**: server clears `~/.aleph/tmp/media/` on start (handle crash residue)

### Constraints

| Constraint | Value | Reason |
|-----------|-------|--------|
| Max file size | 50MB | Telegram single-file limit |
| Download timeout | 60s | Large video files |
| Max media per response | 10 | Prevent LLM flooding |

### Error Handling

**All errors degrade gracefully, never block message delivery.**

- Download fails → `Attachment { url only }` → channel tries URL
- Channel URL send fails → text message delivered normally, user sees URL in text
- MIME detection fails → `application/octet-stream` → channel sends as file
- Temp file delete fails → warn log, no retry

### YAGNI — Not Building

- No persistent media cache — temp files deleted after use
- No media format conversion — deliver as-is
- No thumbnail generation — platforms handle this
- No media deduplication — frequency too low to optimize

## Files to Create/Modify

| Action | File | Description |
|--------|------|-------------|
| Create | `core/src/gateway/media.rs` | `MediaItem` type + `download_media()` + temp file utils |
| Create | `core/src/builtin_tools/media_send.rs` | `media_send` tool |
| Modify | `core/src/builtin_tools/generation/image_generate.rs` | Add `_media` to output |
| Modify | `core/src/builtin_tools/generation/video_generate.rs` | Add `_media` to output |
| Modify | `core/src/builtin_tools/generation/audio_generate.rs` | Add `_media` to output |
| Modify | `core/src/builtin_tools/generation/speech_generate.rs` | Add `_media` to output |
| Modify | `core/src/gateway/reply_emitter.rs` | Add `pending_media` + inject attachments |
| Modify | `core/src/gateway/execution_engine/run_loop.rs` | Extract `_media` in `on_tool_done` |
| Modify | `core/src/agent_loop/prompt_builder.rs` | Add `media_send` guidance to BASE_BEHAVIOR |
| Modify | `core/src/executor/builtin_registry/groups.rs` | Register `media_send` in tool groups |
| Modify | `core/src/gateway/mod.rs` | Export `media` module |
| Modify | `core/src/bin/aleph-server/commands/start/builder/subsystems.rs` | Startup temp dir cleanup |
