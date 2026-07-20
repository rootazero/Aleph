# Media Attachment Infrastructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable tools to deliver media (images, videos, audio) directly in chat instead of URL text, with core download + graceful fallback.

**Architecture:** Tools declare media via `_media` JSON convention. A shared `PendingMedia` buffer bridges `StreamCallback` (extraction) and `ReplyEmitter` (delivery). `MediaCache` handles downloads with fallback to URL-only on failure.

**Tech Stack:** Rust, serde, reqwest (existing), base64 (existing), tokio, futures

**Spec:** `docs/superpowers/specs/2026-03-24-media-attachment-infrastructure-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `src/gateway/media.rs` | `MediaItem` type, `PendingMedia` type alias, MIME detection helper |
| Create | `src/builtin_tools/media_send.rs` | `media_send` AlephTool — passthrough for LLM-initiated media delivery |
| Modify | `src/media/cache.rs` | Add `download_media_item()`, raise limits (50MB/60s) |
| Modify | `src/builtin_tools/generation/image_generate.rs` | Add `_media` to `ImageGenerateOutput` |
| Modify | `src/builtin_tools/generation/video_generate.rs` | Add `_media` to `VideoGenerateOutput` |
| Modify | `src/builtin_tools/generation/audio_generate.rs` | Add `_media` to `AudioGenerateOutput` |
| Modify | `src/builtin_tools/generation/speech_generate.rs` | Add `_media` to `SpeechGenerateOutput` |
| Modify | `src/gateway/reply_emitter.rs` | Add `pending_media` + `media_cache` fields, `drain_and_send_media()`, `send_media_standalone()` |
| Modify | `src/gateway/execution_engine/run_loop.rs` | Add `pending_media` to `StreamCallback`, extract `_media` in `on_tool_done` |
| Modify | `src/gateway/inbound_router/executor.rs` | Pass `PendingMedia` to ReplyEmitter + StreamCallback construction |
| Modify | `src/gateway/session_scheduler.rs` | Same PendingMedia wiring |
| Modify | `src/agent_loop/prompt_builder.rs` | Add `media_send` guidance to `BASE_BEHAVIOR` |
| Modify | `src/executor/builtin_registry/groups.rs` | Add `media_send` to `content_gen` group |
| Modify | `src/builtin_tools/mod.rs` | Add `pub mod media_send` |
| Modify | `src/gateway/mod.rs` | Add `pub mod media` |

---

### Task 1: Create `MediaItem` type and `PendingMedia` alias

**Files:**
- Create: `src/gateway/media.rs`
- Modify: `src/gateway/mod.rs`

- [ ] **Step 1: Create `src/gateway/media.rs`**

```rust
//! Media attachment types for the `_media` tool output convention.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Media item declared by tool output via `_media` convention.
/// Any tool can include `_media: Vec<MediaItem>` in its output JSON
/// to request media delivery to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    /// Remote URL, local file path, or data URL of the media
    pub url: String,
    /// Media type: "image", "video", "audio", "file"
    pub media_type: String,
    /// MIME type (auto-detected from URL/Content-Type if absent)
    #[serde(default)]
    pub mime_type: Option<String>,
    /// Optional filename
    #[serde(default)]
    pub filename: Option<String>,
}

/// Shared media buffer between StreamCallback and ReplyEmitter.
pub type PendingMedia = Arc<Mutex<Vec<MediaItem>>>;

/// Maximum number of media items allowed per run.
pub const MAX_MEDIA_PER_RUN: usize = 10;

/// Detect MIME type from URL extension, with a fallback default based on media_type.
pub fn detect_mime(url: &str, media_type: &str) -> String {
    // Strip query string and fragment before checking extension
    let path = url.split('?').next().unwrap_or(url);
    let path = path.split('#').next().unwrap_or(path);
    // Try URL extension first
    if let Some(ext) = path.rsplit('.').next() {
        match ext.to_lowercase().as_str() {
            "png" => return "image/png".to_string(),
            "jpg" | "jpeg" => return "image/jpeg".to_string(),
            "gif" => return "image/gif".to_string(),
            "webp" => return "image/webp".to_string(),
            "mp4" => return "video/mp4".to_string(),
            "webm" => return "video/webm".to_string(),
            "mp3" => return "audio/mpeg".to_string(),
            "wav" => return "audio/wav".to_string(),
            "ogg" => return "audio/ogg".to_string(),
            "flac" => return "audio/flac".to_string(),
            _ => {}
        }
    }
    // Fallback by media_type
    match media_type {
        "image" => "image/png".to_string(),
        "video" => "video/mp4".to_string(),
        "audio" => "audio/mpeg".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_mime_from_extension() {
        assert_eq!(detect_mime("https://example.com/photo.png", "image"), "image/png");
        assert_eq!(detect_mime("https://example.com/video.mp4", "video"), "video/mp4");
        assert_eq!(detect_mime("https://example.com/song.mp3", "audio"), "audio/mpeg");
    }

    #[test]
    fn test_detect_mime_with_query_params() {
        assert_eq!(detect_mime("https://example.com/img.png?token=abc", "image"), "image/png");
        assert_eq!(detect_mime("https://example.com/video.mp4?v=1.0", "video"), "video/mp4");
        assert_eq!(detect_mime("https://cdn.example.com/file.jpg#frag", "image"), "image/jpeg");
    }

    #[test]
    fn test_detect_mime_fallback() {
        assert_eq!(detect_mime("https://example.com/file?token=abc", "image"), "image/png");
        assert_eq!(detect_mime("https://example.com/file?token=abc", "video"), "video/mp4");
        assert_eq!(detect_mime("https://example.com/file?token=abc", "audio"), "audio/mpeg");
        assert_eq!(detect_mime("https://example.com/file", "file"), "application/octet-stream");
    }

    #[test]
    fn test_media_item_deserialization() {
        let json = r#"{"url":"https://example.com/img.png","media_type":"image"}"#;
        let item: MediaItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.url, "https://example.com/img.png");
        assert_eq!(item.media_type, "image");
        assert!(item.mime_type.is_none());
        assert!(item.filename.is_none());
    }

    #[test]
    fn test_media_item_with_all_fields() {
        let json = r#"{"url":"https://example.com/img.png","media_type":"image","mime_type":"image/png","filename":"photo.png"}"#;
        let item: MediaItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.mime_type.unwrap(), "image/png");
        assert_eq!(item.filename.unwrap(), "photo.png");
    }
}
```

- [ ] **Step 2: Add module to `src/gateway/mod.rs`**

Add after line 88 (`pub mod voice;`):

```rust
pub mod media;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib gateway::media`
Expected: All 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/gateway/media.rs src/gateway/mod.rs
git commit -m "gateway: add MediaItem type and PendingMedia for _media convention"
```

---

### Task 2: Extend `MediaCache` with `download_media_item()`

**Files:**
- Modify: `src/media/cache.rs`

- [ ] **Step 1: Write tests for `download_media_item`**

Add these tests at the end of the `mod tests` block in `src/media/cache.rs`:

```rust
    #[tokio::test]
    async fn test_download_media_item_local_path() {
        use crate::gateway::media::MediaItem;
        let cache = MediaCache::new();

        // Create a temp file to use as "local path"
        let dir = session_dir("test-media-item-local");
        std::fs::create_dir_all(&dir).unwrap();
        let local_file = dir.join("test.png");
        std::fs::write(&local_file, b"fake png data").unwrap();

        let item = MediaItem {
            url: local_file.to_string_lossy().to_string(),
            media_type: "image".to_string(),
            mime_type: Some("image/png".to_string()),
            filename: None,
        };

        let att = cache.download_media_item(&item, "test-media-item-local").await;
        assert_eq!(att.mime_type, "image/png");
        assert!(att.path.is_some());
        assert!(att.url.is_some());

        let _ = MediaCache::cleanup_session("test-media-item-local");
    }

    #[tokio::test]
    async fn test_download_media_item_data_url() {
        use crate::gateway::media::MediaItem;
        let cache = MediaCache::new();

        // "Hello" in base64 = SGVsbG8=
        let item = MediaItem {
            url: "data:text/plain;base64,SGVsbG8=".to_string(),
            media_type: "file".to_string(),
            mime_type: None,
            filename: Some("hello.txt".to_string()),
        };

        let att = cache.download_media_item(&item, "test-media-item-data").await;
        assert_eq!(att.mime_type, "text/plain");
        assert!(att.path.is_some(), "data URL should be decoded to file");

        // Verify content
        let content = std::fs::read_to_string(att.path.as_ref().unwrap()).unwrap();
        assert_eq!(content, "Hello");

        let _ = MediaCache::cleanup_session("test-media-item-data");
    }

    #[tokio::test]
    async fn test_download_media_item_invalid_url_fallback() {
        use crate::gateway::media::MediaItem;
        let cache = MediaCache::new();

        let item = MediaItem {
            url: "https://invalid.example.com/does-not-exist.png".to_string(),
            media_type: "image".to_string(),
            mime_type: Some("image/png".to_string()),
            filename: None,
        };

        let att = cache.download_media_item(&item, "test-media-item-fallback").await;
        // Should fallback to URL-only
        assert!(att.url.is_some());
        assert!(att.path.is_none());
        assert_eq!(att.mime_type, "image/png");

        let _ = MediaCache::cleanup_session("test-media-item-fallback");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib media::cache::tests::test_download_media_item -- 2>&1 | head -20`
Expected: FAIL — `download_media_item` does not exist yet.

- [ ] **Step 3: Raise limits and implement `download_media_item`**

In `src/media/cache.rs`, change constants:

```rust
/// Maximum file size allowed (50 MB — for video files).
const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;

/// HTTP download timeout (60s — large media files).
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
```

Update error message in `CacheError::TooLarge`:
```rust
#[error("File exceeds 50 MB limit: {size} bytes")]
TooLarge { size: u64 },
```

Add import at top of file:
```rust
use crate::gateway::media::{MediaItem, detect_mime};
```

Add method to `impl MediaCache` block (after `cleanup_stale()`):

```rust
    /// Convert a [`MediaItem`] (from tool `_media` output) to a channel [`Attachment`].
    ///
    /// Downloads to temp file on success, falls back to URL-only on failure.
    pub async fn download_media_item(
        &self,
        item: &MediaItem,
        session_id: &str,
    ) -> Attachment {
        let id = uuid::Uuid::new_v4().to_string();
        let mime = item
            .mime_type
            .clone()
            .unwrap_or_else(|| detect_mime(&item.url, &item.media_type));

        // Build a temporary Attachment to pass through resolve()
        let temp_attachment = if item.url.starts_with("data:") {
            // Parse data URL: data:[<mediatype>][;base64],<data>
            match Self::decode_data_url(&item.url) {
                Ok((decoded_mime, bytes)) => Attachment {
                    id: id.clone(),
                    mime_type: decoded_mime.unwrap_or_else(|| mime.clone()),
                    filename: item.filename.clone().or_else(|| Some(format!("{}.bin", &id[..8]))),
                    size: Some(bytes.len() as u64),
                    url: None,
                    path: None,
                    data: Some(bytes),
                },
                Err(e) => {
                    warn!(url_prefix = &item.url[..item.url.len().min(30)], error = %e, "Failed to decode data URL, falling back to URL-only");
                    return Self::url_only_attachment(&id, &item.url, &mime, &item.filename);
                }
            }
        } else if item.url.starts_with('/') || item.url.starts_with("./") || item.url.starts_with("~/") {
            // Local file path
            Attachment {
                id: id.clone(),
                mime_type: mime.clone(),
                filename: item.filename.clone(),
                size: None,
                url: None,
                path: Some(item.url.clone()),
                data: None,
            }
        } else {
            // HTTP/HTTPS URL
            Attachment {
                id: id.clone(),
                mime_type: mime.clone(),
                filename: item.filename.clone().or_else(|| Some(format!("{}.bin", &id[..8]))),
                size: None,
                url: Some(item.url.clone()),
                path: None,
                data: None,
            }
        };

        match self.resolve(&temp_attachment, session_id).await {
            Ok(cached) => Attachment {
                id,
                mime_type: cached.mime_type,
                filename: item.filename.clone(),
                size: Some(cached.size),
                url: Some(item.url.clone()),
                path: Some(cached.local_path.to_string_lossy().to_string()),
                data: None,
            },
            Err(e) => {
                warn!(url = %item.url, error = %e, "Media download failed, falling back to URL-only");
                Self::url_only_attachment(&id, &item.url, &mime, &item.filename)
            }
        }
    }

    /// Parse a data URL and decode its content.
    fn decode_data_url(url: &str) -> Result<(Option<String>, Vec<u8>), CacheError> {
        // Format: data:[<mediatype>][;base64],<data>
        let rest = url.strip_prefix("data:").ok_or_else(|| {
            CacheError::Download("Not a data URL".to_string())
        })?;
        let (header, data) = rest.split_once(',').ok_or_else(|| {
            CacheError::Download("Invalid data URL: no comma separator".to_string())
        })?;
        let mime = if header.contains(';') {
            let mime_part = header.split(';').next().unwrap_or("");
            if mime_part.is_empty() { None } else { Some(mime_part.to_string()) }
        } else if !header.is_empty() && !header.contains("base64") {
            Some(header.to_string())
        } else {
            None
        };

        let bytes = if header.contains("base64") {
            BASE64.decode(data).map_err(|e| CacheError::Download(format!("Base64 decode failed: {}", e)))?
        } else {
            data.as_bytes().to_vec()
        };

        Ok((mime, bytes))
    }

    /// Create a URL-only fallback Attachment (no local file).
    fn url_only_attachment(id: &str, url: &str, mime: &str, filename: &Option<String>) -> Attachment {
        Attachment {
            id: id.to_string(),
            mime_type: mime.to_string(),
            filename: filename.clone(),
            size: None,
            url: Some(url.to_string()),
            path: None,
            data: None,
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib media::cache`
Expected: All tests pass (existing + 3 new).

- [ ] **Step 5: Commit**

```bash
git add src/media/cache.rs
git commit -m "media: add download_media_item() with data URL support and 50MB limit"
```

---

### Task 3: Add `_media` field to generation tool outputs

**Files:**
- Modify: `src/builtin_tools/generation/image_generate.rs`
- Modify: `src/builtin_tools/generation/video_generate.rs`
- Modify: `src/builtin_tools/generation/audio_generate.rs`
- Modify: `src/builtin_tools/generation/speech_generate.rs`

- [ ] **Step 1: Add `_media` to `ImageGenerateOutput`**

In `src/builtin_tools/generation/image_generate.rs`:

Add import at top:
```rust
use crate::gateway::media::{MediaItem, detect_mime};
```

Add field after `_display` in `ImageGenerateOutput` (line ~51):
```rust
    /// Media items for channel delivery
    pub _media: Vec<MediaItem>,
```

At the output construction site (line ~225), add `_media` field:
```rust
        Ok(ImageGenerateOutput {
            _display: display,
            _media: vec![MediaItem {
                url: image_location.clone(),
                media_type: "image".into(),
                mime_type: Some(detect_mime(&image_location, "image")),
                filename: None,
            }],
            image_location,
            // ... rest unchanged
        })
```

Update test `test_output_serialization` to include `_media` field in the test output construction.

- [ ] **Step 2: Add `_media` to `VideoGenerateOutput`**

Same pattern in `src/builtin_tools/generation/video_generate.rs`:

Add import, add `pub _media: Vec<MediaItem>` field, populate at construction (line ~155):
```rust
            _media: vec![MediaItem {
                url: video_location.to_string(),
                media_type: "video".into(),
                mime_type: Some(detect_mime(&video_location, "video")),
                filename: None,
            }],
```

- [ ] **Step 3: Add `_media` to `AudioGenerateOutput`**

Same pattern in `src/builtin_tools/generation/audio_generate.rs`:

Add import, add `pub _media: Vec<MediaItem>` field, populate at construction (line ~126):
```rust
            _media: vec![MediaItem {
                url: audio_location.to_string(),
                media_type: "audio".into(),
                mime_type: Some(detect_mime(&audio_location, "audio")),
                filename: None,
            }],
```

- [ ] **Step 4: Add `_media` to `SpeechGenerateOutput`**

Same pattern in `src/builtin_tools/generation/speech_generate.rs`:

Add import, add `pub _media: Vec<MediaItem>` field, populate at construction (line ~233):
```rust
            _media: vec![MediaItem {
                url: audio_location.clone(),
                media_type: "audio".into(),
                mime_type: Some(detect_mime(&audio_location, "audio")),
                filename: None,
            }],
```

Update test `test_output_serialization` to include `_media` field.

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore`
Expected: Success.

- [ ] **Step 6: Run existing generation tests**

Run: `cargo test -p alephcore --lib builtin_tools::generation`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/builtin_tools/generation/
git commit -m "generation: add _media field to all generation tool outputs"
```

---

### Task 4: Create `media_send` built-in tool

**Files:**
- Create: `src/builtin_tools/media_send.rs`
- Modify: `src/builtin_tools/mod.rs`
- Modify: `src/executor/builtin_registry/groups.rs`

- [ ] **Step 1: Create `src/builtin_tools/media_send.rs`**

```rust
//! media_send — LLM-invoked tool for delivering media to the user.
//!
//! This tool does NO processing. It simply passes the input items as `_media`
//! output, which the ReplyEmitter picks up for download and channel delivery.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::gateway::media::MediaItem;
use crate::tools::traits::AlephTool;

/// Input arguments for media_send tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MediaSendArgs {
    /// List of media items to send to the user.
    /// Each item needs a `url` and `media_type` ("image", "video", "audio", "file").
    pub items: Vec<MediaSendItem>,
}

/// A single media item to send.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MediaSendItem {
    /// URL of the media file
    pub url: String,
    /// Type: "image", "video", "audio", or "file"
    pub media_type: String,
    /// Optional MIME type (auto-detected if absent)
    #[serde(default)]
    pub mime_type: Option<String>,
    /// Optional filename
    #[serde(default)]
    pub filename: Option<String>,
}

/// Output from media_send tool.
#[derive(Debug, Clone, Serialize)]
pub struct MediaSendOutput {
    /// Human-readable display text
    pub _display: String,
    /// Media items for channel delivery (consumed by ReplyEmitter)
    pub _media: Vec<MediaItem>,
}

/// The media_send tool — passes media URLs through for channel delivery.
#[derive(Clone)]
pub struct MediaSendTool;

impl MediaSendTool {
    pub fn new() -> Self {
        Self
    }
}

impl AlephTool for MediaSendTool {
    const NAME: &'static str = "media_send";
    const DESCRIPTION: &'static str =
        "Send media files (images, videos, audio) directly to the user in the chat. \
         Use this when you find or have media URLs that the user wants to see or hear.";
    type Args = MediaSendArgs;
    type Output = MediaSendOutput;

    async fn call_impl(&self, args: Self::Args) -> Result<Self::Output, crate::tools::error::ToolError> {
        let count = args.items.len();
        let media: Vec<MediaItem> = args
            .items
            .into_iter()
            .map(|item| MediaItem {
                url: item.url,
                media_type: item.media_type,
                mime_type: item.mime_type,
                filename: item.filename,
            })
            .collect();

        Ok(MediaSendOutput {
            _display: format!("📎 Sending {} media file{}...", count, if count == 1 { "" } else { "s" }),
            _media: media,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_media_send_passthrough() {
        let tool = MediaSendTool::new();
        let args = MediaSendArgs {
            items: vec![
                MediaSendItem {
                    url: "https://example.com/cat.jpg".to_string(),
                    media_type: "image".to_string(),
                    mime_type: Some("image/jpeg".to_string()),
                    filename: Some("cat.jpg".to_string()),
                },
                MediaSendItem {
                    url: "https://example.com/song.mp3".to_string(),
                    media_type: "audio".to_string(),
                    mime_type: None,
                    filename: None,
                },
            ],
        };

        let output = tool.call_impl(args).await.unwrap();
        assert_eq!(output._media.len(), 2);
        assert_eq!(output._media[0].url, "https://example.com/cat.jpg");
        assert_eq!(output._media[0].media_type, "image");
        assert_eq!(output._media[1].url, "https://example.com/song.mp3");
        assert!(output._display.contains("2"));
    }

    #[tokio::test]
    async fn test_media_send_single() {
        let tool = MediaSendTool::new();
        let args = MediaSendArgs {
            items: vec![MediaSendItem {
                url: "https://example.com/photo.png".to_string(),
                media_type: "image".to_string(),
                mime_type: None,
                filename: None,
            }],
        };

        let output = tool.call_impl(args).await.unwrap();
        assert_eq!(output._media.len(), 1);
        assert!(output._display.contains("1"));
        assert!(!output._display.contains("files"));  // singular "file"
    }
}
```

- [ ] **Step 2: Register module in `src/builtin_tools/mod.rs`**

Add after existing module declarations (e.g., after `pub mod voice_tools;` line 75):
```rust
pub mod media_send;
```

- [ ] **Step 3: Add `media_send` to `content_gen` group in `src/executor/builtin_registry/groups.rs`**

Change line ~39:
```rust
        tools: &["image_generate", "video_generate", "audio_generate", "speech_generate"],
```
to:
```rust
        tools: &["image_generate", "video_generate", "audio_generate", "speech_generate", "media_send"],
```

- [ ] **Step 4: Register `MediaSendTool` in the builtin registry**

Find where other generation tools are registered (search for `ImageGenerateTool` registration in `src/executor/builtin_registry/`). Add `MediaSendTool` registration alongside them:

```rust
registry.register(MediaSendTool::new());
```

Add import:
```rust
use crate::builtin_tools::media_send::MediaSendTool;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib builtin_tools::media_send`
Expected: All 2 tests pass.

- [ ] **Step 6: Compile check**

Run: `cargo check -p alephcore`
Expected: Success.

- [ ] **Step 7: Commit**

```bash
git add src/builtin_tools/media_send.rs src/builtin_tools/mod.rs src/executor/builtin_registry/
git commit -m "tools: add media_send tool for LLM-initiated media delivery"
```

---

### Task 5: Wire `PendingMedia` into `StreamCallback` for `_media` extraction

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs`

- [ ] **Step 1: Add `pending_media` field to `StreamCallback`**

In `src/gateway/execution_engine/run_loop.rs`, add import at top:
```rust
use crate::gateway::media::{MediaItem, PendingMedia, MAX_MEDIA_PER_RUN};
```

Modify `StreamCallback` struct (line ~312):
```rust
pub(super) struct StreamCallback<E: EventEmitter + Send + Sync + 'static> {
    emitter: Arc<E>,
    run_id: String,
    seq: u64,
    chunk_index: u32,
    pending_media: PendingMedia,  // NEW
}
```

Update `StreamCallback::new()` (line ~320):
```rust
    pub(super) fn new(emitter: Arc<E>, run_id: String, pending_media: PendingMedia) -> Self {
        Self {
            emitter,
            run_id,
            seq: 0,
            chunk_index: 0,
            pending_media,
        }
    }
```

- [ ] **Step 2: Extract `_media` in `on_tool_done`**

In `on_tool_done` method (line ~394), add media extraction BEFORE the existing code:

```rust
    fn on_tool_done(&mut self, name: &str, result: &crate::agent_loop::ToolResult) {
        // Extract _media from raw Value before serialization
        match result {
            crate::agent_loop::ToolResult::Success { output }
            | crate::agent_loop::ToolResult::SuccessAndStopLoop { output } => {
                if let Some(media_val) = output.get("_media") {
                    if let Ok(items) = serde_json::from_value::<Vec<MediaItem>>(media_val.clone()) {
                        let mut pending = self.pending_media.lock().unwrap_or_else(|e| e.into_inner());
                        let remaining = MAX_MEDIA_PER_RUN.saturating_sub(pending.len());
                        if remaining < items.len() {
                            tracing::warn!(
                                tool = %name,
                                total = items.len(),
                                accepted = remaining,
                                "Media items exceed per-run limit, dropping excess"
                            );
                        }
                        pending.extend(items.into_iter().take(remaining));
                    }
                }
            }
            _ => {}
        }

        // Existing code: emit ToolEnd event
        use crate::gateway::event_emitter::ToolResult as EmitterToolResult;
        self.seq += 1;
        // ... rest unchanged ...
    }
```

- [ ] **Step 3: Add `pending_media` to `RunRequest`**

In `src/gateway/execution_engine/mod.rs`, add import and field to `RunRequest` (line ~52):

```rust
use crate::gateway::media::PendingMedia;
```

Add field after `attachments`:
```rust
    /// Shared pending media buffer (for media attachment delivery)
    #[allow(dead_code)]
    pub pending_media: PendingMedia,
```

Update all `RunRequest` construction sites to include `pending_media: std::sync::Arc::new(std::sync::Mutex::new(Vec::new()))` — these will be replaced with the shared Arc in Task 6. Search for `RunRequest {` in:
- `src/gateway/execution_engine/tests.rs`
- `src/gateway/inbound_router/executor.rs`
- `src/gateway/session_scheduler.rs`

- [ ] **Step 4: Update `StreamCallback::new()` call site to use `request.pending_media`**

In `run_loop.rs`, find the call site (line ~191):
```rust
let mut callback = StreamCallback::new(emitter.clone(), run_id.to_string());
```

Change to:
```rust
let mut callback = StreamCallback::new(emitter.clone(), run_id.to_string(), request.pending_media.clone());
```

This ensures `StreamCallback` shares the SAME Arc that `ReplyEmitter` will use (wired in Task 6).

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: Success.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs
git commit -m "gateway: extract _media from tool output in StreamCallback"
```

---

### Task 6: Add media delivery to `ReplyEmitter`

**Files:**
- Modify: `src/gateway/reply_emitter.rs`

- [ ] **Step 1: Add `pending_media` and `media_cache` fields**

Add imports at top of `src/gateway/reply_emitter.rs`:
```rust
use crate::gateway::media::PendingMedia;
use crate::media::cache::MediaCache;
```

Add fields to `ReplyEmitter` struct (after `voice_state` line ~101):
```rust
    /// Pending media items from tool outputs (shared with StreamCallback)
    pending_media: PendingMedia,
    /// Media cache for downloading media items
    media_cache: MediaCache,
```

- [ ] **Step 2: Update constructors**

In `ReplyEmitter::new()` (line ~109), add parameter and initialize:
```rust
    pub fn new(
        channel_registry: Arc<ChannelRegistry>,
        route: ReplyRoute,
        run_id: String,
        pending_media: PendingMedia,
    ) -> Self {
        Self {
            // ... existing fields ...
            pending_media,
            media_cache: MediaCache::new(),
        }
    }
```

Same for `with_config()` (line ~130):
```rust
    pub fn with_config(
        channel_registry: Arc<ChannelRegistry>,
        route: ReplyRoute,
        run_id: String,
        config: ReplyEmitterConfig,
        pending_media: PendingMedia,
    ) -> Self {
        Self {
            // ... existing fields ...
            pending_media,
            media_cache: MediaCache::new(),
        }
    }
```

- [ ] **Step 3: Implement `drain_and_send_media()` and `send_media_standalone()`**

Add methods to `impl ReplyEmitter` block:

```rust
    /// Drain pending media, download in parallel via MediaCache, return Attachments.
    async fn drain_and_send_media(&self) -> Vec<crate::gateway::channel::Attachment> {
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

    /// Send media as a separate standalone message.
    async fn send_media_standalone(&self, attachments: Vec<crate::gateway::channel::Attachment>) {
        if attachments.is_empty() { return; }
        let message = OutboundMessage {
            conversation_id: self.route.conversation_id.clone(),
            text: String::new(),
            attachments,
            reply_to: None,
            inline_keyboard: None,
            metadata: Default::default(),
        };
        if let Err(e) = self.channel_registry.send(&self.route.channel_id, message).await {
            tracing::warn!(error = %e, "Failed to send media standalone message");
        }
    }
```

- [ ] **Step 4: Inject media delivery into `send_to_channel()`**

At the END of `send_to_channel()` (after all text chunks are sent, before the method returns), add:

```rust
        // Deliver any pending media as standalone message
        let media_attachments = self.drain_and_send_media().await;
        self.send_media_standalone(media_attachments).await;
```

- [ ] **Step 5: Inject media delivery into `send_as_voice()`**

At the END of `send_as_voice()` (after voice message is sent), add the same two lines:

```rust
        // Deliver any pending media as standalone message
        let media_attachments = self.drain_and_send_media().await;
        self.send_media_standalone(media_attachments).await;
```

- [ ] **Step 6: Inject media delivery into `send_typewriter()`**

At the END of `send_typewriter()` (after typewriter sequence completes), add the same two lines.

- [ ] **Step 6.5: Inject media delivery into `AskUser` handler in `emit()`**

In the `StreamEvent::AskUser` arm of `emit()` (reply_emitter.rs line ~786), add media drain BEFORE the buffer flush:

```rust
            StreamEvent::AskUser { question, .. } => {
                // Drain any pending media before sending the question
                let media_attachments = self.drain_and_send_media().await;
                self.send_media_standalone(media_attachments).await;

                // Flush buffer first (existing code)
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };
                // ... rest unchanged ...
            }
```

- [ ] **Step 7: Fix all call sites of `ReplyEmitter::new()` and `with_config()`**

These constructors now require `pending_media: PendingMedia`. The key wiring pattern is: **create `PendingMedia` Arc, pass to `ReplyEmitter`, AND store it in `RunRequest` so `StreamCallback` gets the same Arc.**

**`src/gateway/inbound_router/executor.rs`** (lines ~124, ~134, ~226):
Create `pending_media` before building `ReplyEmitter`, pass to `with_config()`. Then when building `RunRequest`, set `pending_media: pending_media.clone()`. This ensures `StreamCallback` (in `run_loop.rs`) uses the same Arc via `request.pending_media`.

```rust
let pending_media: PendingMedia = Arc::new(Mutex::new(Vec::new()));
let re = ReplyEmitter::with_config(
    self.channel_registry.clone(),
    ctx.reply_route.clone(),
    run_id.clone(),
    reply_config,
    pending_media.clone(),
);
// ... later in RunRequest construction:
// pending_media: pending_media.clone(),
```

**`src/gateway/session_scheduler.rs`** (lines ~176, ~392):
Same pattern — create `PendingMedia`, pass to `ReplyEmitter::with_config(...)`, then pass to `RunRequest`.

**`src/gateway/reply_emitter.rs` tests** (lines ~857, ~879, ~898, ~913):
Pass `Arc::new(Mutex::new(Vec::new()))` as the last argument.

- [ ] **Step 8: Compile check**

Run: `cargo check -p alephcore`
Expected: Success.

- [ ] **Step 9: Run tests**

Run: `cargo test -p alephcore --lib gateway::reply_emitter`
Expected: All existing tests pass.

- [ ] **Step 10: Commit**

```bash
git add src/gateway/reply_emitter.rs src/gateway/inbound_router/executor.rs src/gateway/session_scheduler.rs src/gateway/execution_engine/run_loop.rs
git commit -m "gateway: wire PendingMedia through ReplyEmitter for media delivery"
```

---

### Task 7: Add `media_send` prompt guidance

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs`

- [ ] **Step 1: Add media_send guidance to `BASE_BEHAVIOR`**

In `src/agent_loop/prompt_builder.rs`, find the last line of `BASE_BEHAVIOR` (line ~57, ending with `...use them.";`). Change the closing to continue with a new rule:

Replace:
```rust
...use them.";
```
With:
```rust
...use them.\n\
- **MEDIA DELIVERY.** When you find media (images, videos, audio) that the user wants to see or hear, use the `media_send` tool to deliver them directly in the chat. Do not just paste URLs — use the tool so media appears inline.";
```

Note: `BASE_BEHAVIOR` uses `\n\` line-continuation style throughout — follow the same pattern.

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`
Expected: Success.

- [ ] **Step 3: Commit**

```bash
git add src/agent_loop/prompt_builder.rs
git commit -m "prompt: add media_send guidance to BASE_BEHAVIOR"
```

---

### Task 8: Add temp file cleanup at RunComplete

**Files:**
- Modify: `src/gateway/reply_emitter.rs`

The spec requires `MediaCache::cleanup_session(session_id)` after run completes. The existing `cleanup_stale()` handles crash residue (1-hour TTL), but we should eagerly clean up after each run.

- [ ] **Step 1: Add cleanup in `RunComplete` handler**

In `reply_emitter.rs`, in the `StreamEvent::RunComplete` arm (line ~716), after all text/media has been sent, add:

```rust
                // Clean up media temp files for this run
                if let Err(e) = MediaCache::cleanup_session(&self.run_id) {
                    tracing::warn!(error = %e, "Failed to cleanup media session");
                }
```

- [ ] **Step 2: Commit**

```bash
git add src/gateway/reply_emitter.rs
git commit -m "gateway: cleanup media temp files after run completes"
```

---

### Task 9: Integration verification

- [ ] **Step 1: Full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass (pre-existing failures in `tools::markdown_skill::loader::tests` are known and unrelated).

- [ ] **Step 2: Clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`
Expected: No new warnings.

- [ ] **Step 3: Final commit (if any fixes needed)**

```bash
git add -A
git commit -m "media: fix clippy warnings and test issues"
```
