# Core Multimodal Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the attachment pipeline break so images and audio reach the LLM, with native vision injection and Whisper transcription.

**Architecture:** Bottom-up: first create the media infrastructure (cache, transcription, processor), then fix each pipeline break point from the LLM end (OpenAI adapter) backward to the channel entry (RunRequest). Each task compiles independently.

**Tech Stack:** Rust, reqwest (multipart already enabled), base64 (already in deps), tokio, teloxide

**Spec:** `docs/superpowers/specs/2026-03-22-core-multimodal-enhancement-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/media/mod.rs` | Create | Module declarations |
| `src/media/cache.rs` | Create | MediaCache: download, temp storage, base64, cleanup |
| `src/media/transcription.rs` | Create | TranscriptionService trait + TranscriptionResult |
| `src/media/whisper.rs` | Create | WhisperTranscription: OpenAI Whisper API |
| `src/media/processor.rs` | Create | MediaProcessor: unified attachment → ContentBlock |
| `src/providers/protocols/openai.rs` | Modify | Handle ContentBlock::Image in convert_messages() |
| `src/providers/message.rs` | Modify | Add user_with_content() constructor |
| `src/gateway/execution_engine/mod.rs` | Modify | Add `attachments` to RunRequest |
| `src/gateway/inbound_router/executor.rs` | Modify | Pass attachments to RunRequest |
| `src/gateway/execution_engine/engine.rs` | Modify | Store summary, add MediaProcessor field |
| `src/gateway/execution_engine/run_loop.rs` | Modify | Build multimodal UnifiedMessage |
| `src/agent_loop/loop_core.rs` | Modify | Add run_with_history_messages() |

---

## Task 1: Create media module with MediaCache

**Files:**
- Create: `src/media/mod.rs`
- Create: `src/media/cache.rs`
- Modify: `src/lib.rs` (add `pub mod media;`)

- [ ] **Step 1: Create module skeleton**

`src/media/mod.rs`:
```rust
pub mod cache;
```

`src/media/cache.rs`:
```rust
//! Media file cache for temporary attachment storage.

use std::path::{Path, PathBuf};
use tracing::{debug, warn};

use crate::gateway::channel::Attachment;

/// A resolved media file in the local cache
#[derive(Debug, Clone)]
pub struct CachedMedia {
    pub local_path: PathBuf,
    pub mime_type: String,
    pub size: u64,
}

/// Temporary media file cache. Downloads/copies attachments to a local
/// temp directory for processing. Files are cleaned up per-session.
pub struct MediaCache {
    cache_dir: PathBuf,
    max_file_size: u64,
}

impl Default for MediaCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaCache {
    /// Default max file size: 20MB
    const DEFAULT_MAX_SIZE: u64 = 20 * 1024 * 1024;

    pub fn new() -> Self {
        Self {
            cache_dir: std::env::temp_dir().join("aleph").join("media"),
            max_file_size: Self::DEFAULT_MAX_SIZE,
        }
    }

    /// Resolve an attachment to a local cached file.
    /// Priority: inline data > local path > URL download.
    pub async fn resolve(
        &self,
        attachment: &Attachment,
        session_id: &str,
    ) -> anyhow::Result<CachedMedia> {
        let session_dir = self.cache_dir.join(session_id);
        tokio::fs::create_dir_all(&session_dir).await?;

        // 1. Inline data
        if let Some(data) = &attachment.data {
            if data.len() as u64 > self.max_file_size {
                anyhow::bail!("Attachment too large: {} bytes", data.len());
            }
            let filename = attachment.filename.as_deref().unwrap_or(&attachment.id);
            let path = session_dir.join(filename);
            tokio::fs::write(&path, data).await?;
            return Ok(CachedMedia {
                local_path: path,
                mime_type: attachment.mime_type.clone(),
                size: data.len() as u64,
            });
        }

        // 2. Local path
        if let Some(ref local_path) = attachment.path {
            let p = Path::new(local_path);
            if p.exists() {
                let meta = tokio::fs::metadata(p).await?;
                if meta.len() > self.max_file_size {
                    anyhow::bail!("Attachment too large: {} bytes", meta.len());
                }
                return Ok(CachedMedia {
                    local_path: p.to_path_buf(),
                    mime_type: attachment.mime_type.clone(),
                    size: meta.len(),
                });
            }
        }

        // 3. URL download
        if let Some(ref url) = attachment.url {
            let resp = reqwest::Client::new()
                .get(url)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await?;
            let bytes = resp.bytes().await?;
            if bytes.len() as u64 > self.max_file_size {
                anyhow::bail!("Downloaded attachment too large: {} bytes", bytes.len());
            }
            let filename = attachment.filename.as_deref().unwrap_or(&attachment.id);
            let path = session_dir.join(filename);
            tokio::fs::write(&path, &bytes).await?;
            debug!("Downloaded attachment to {:?} ({} bytes)", path, bytes.len());
            return Ok(CachedMedia {
                local_path: path,
                mime_type: attachment.mime_type.clone(),
                size: bytes.len() as u64,
            });
        }

        anyhow::bail!("Attachment has no data, path, or URL")
    }

    /// Read a cached file and return base64-encoded content
    pub async fn to_base64(&self, cached: &CachedMedia) -> anyhow::Result<String> {
        use base64::Engine;
        let data = tokio::fs::read(&cached.local_path).await?;
        Ok(base64::engine::general_purpose::STANDARD.encode(&data))
    }

    /// Remove all cached files for a session
    pub async fn cleanup_session(&self, session_id: &str) {
        let session_dir = self.cache_dir.join(session_id);
        if session_dir.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&session_dir).await {
                warn!("Failed to cleanup media cache for {}: {}", session_id, e);
            } else {
                debug!("Cleaned up media cache for session {}", session_id);
            }
        }
    }

    /// Clean up stale cache directories (call on startup)
    pub async fn cleanup_stale(&self) {
        if !self.cache_dir.exists() {
            return;
        }
        if let Ok(mut entries) = tokio::fs::read_dir(&self.cache_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Ok(meta) = entry.metadata().await {
                    // Remove dirs older than 1 hour
                    if let Ok(age) = meta.modified().and_then(|m| m.elapsed()) {
                        if age.as_secs() > 3600 {
                            let _ = tokio::fs::remove_dir_all(entry.path()).await;
                        }
                    }
                }
            }
        }
    }
}
```

Add `pub mod media;` to `src/lib.rs`.

- [ ] **Step 2: Write tests**

Add at the bottom of `cache.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resolve_inline_data() {
        let cache = MediaCache::new();
        let attachment = Attachment {
            id: "test1".into(),
            mime_type: "image/png".into(),
            filename: Some("test.png".into()),
            size: Some(4),
            url: None,
            path: None,
            data: Some(vec![1, 2, 3, 4]),
        };
        let result = cache.resolve(&attachment, "test-session").await.unwrap();
        assert_eq!(result.mime_type, "image/png");
        assert_eq!(result.size, 4);
        assert!(result.local_path.exists());
        cache.cleanup_session("test-session").await;
    }

    #[tokio::test]
    async fn test_resolve_no_source_fails() {
        let cache = MediaCache::new();
        let attachment = Attachment {
            id: "empty".into(),
            mime_type: "image/png".into(),
            filename: None,
            size: None,
            url: None,
            path: None,
            data: None,
        };
        assert!(cache.resolve(&attachment, "test-session").await.is_err());
    }

    #[tokio::test]
    async fn test_to_base64() {
        let cache = MediaCache::new();
        let attachment = Attachment {
            id: "b64test".into(),
            mime_type: "image/png".into(),
            filename: Some("b64.png".into()),
            size: Some(3),
            url: None,
            path: None,
            data: Some(vec![0xDE, 0xAD, 0xFF]),
        };
        let cached = cache.resolve(&attachment, "test-b64").await.unwrap();
        let b64 = cache.to_base64(&cached).await.unwrap();
        assert_eq!(b64, "3q3/"); // base64 of [0xDE, 0xAD, 0xFF]
        cache.cleanup_session("test-b64").await;
    }
}
```

- [ ] **Step 3: Compile and test**

Run: `cargo test -p alephcore --lib media::cache`
Expected: ALL PASS

- [ ] **Step 4: Commit**

```bash
git add src/media/ src/lib.rs && git commit -m "media: add MediaCache for attachment download and temp storage"
```

---

## Task 2: TranscriptionService trait and WhisperTranscription

**Files:**
- Create: `src/media/transcription.rs`
- Create: `src/media/whisper.rs`
- Modify: `src/media/mod.rs`

- [ ] **Step 1: Create TranscriptionService trait**

`src/media/transcription.rs`:
```rust
//! Transcription service abstraction for speech-to-text.

use async_trait::async_trait;
use super::cache::CachedMedia;

/// Result of audio transcription
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    pub text: String,
    pub language: Option<String>,
}

/// Trait for audio transcription providers
#[async_trait]
pub trait TranscriptionService: Send + Sync {
    /// Transcribe an audio file to text
    async fn transcribe(&self, audio: &CachedMedia) -> anyhow::Result<TranscriptionResult>;
}
```

- [ ] **Step 2: Create WhisperTranscription**

`src/media/whisper.rs`:
```rust
//! OpenAI Whisper API transcription provider.

use async_trait::async_trait;
use reqwest::multipart;
use tracing::{debug, warn};

use super::cache::CachedMedia;
use super::transcription::{TranscriptionResult, TranscriptionService};

/// Whisper API transcription via OpenAI-compatible endpoints
pub struct WhisperTranscription {
    api_key: String,
    base_url: String,
    model: String,
}

impl WhisperTranscription {
    pub fn new(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        Self {
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            model: model.unwrap_or_else(|| "whisper-1".to_string()),
        }
    }
}

#[async_trait]
impl TranscriptionService for WhisperTranscription {
    async fn transcribe(&self, audio: &CachedMedia) -> anyhow::Result<TranscriptionResult> {
        let url = format!("{}/audio/transcriptions", self.base_url);

        let file_bytes = tokio::fs::read(&audio.local_path).await?;
        let filename = audio
            .local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.ogg")
            .to_string();

        let file_part = multipart::Part::bytes(file_bytes)
            .file_name(filename)
            .mime_str(&audio.mime_type)?;

        let form = multipart::Form::new()
            .part("file", file_part)
            .text("model", self.model.clone())
            .text("response_format", "json");

        let resp = reqwest::Client::new()
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Whisper API error {}: {}", status, body);
        }

        #[derive(serde::Deserialize)]
        struct WhisperResponse {
            text: String,
            #[serde(default)]
            language: Option<String>,
        }

        let result: WhisperResponse = resp.json().await?;
        debug!("Whisper transcription: {} chars", result.text.len());

        Ok(TranscriptionResult {
            text: result.text,
            language: result.language,
        })
    }
}
```

- [ ] **Step 3: Update mod.rs**

```rust
pub mod cache;
pub mod transcription;
pub mod whisper;
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/media/ && git commit -m "media: add TranscriptionService trait and WhisperTranscription impl"
```

---

## Task 3: MediaProcessor — unified entry point

**Files:**
- Create: `src/media/processor.rs`
- Modify: `src/media/mod.rs`

- [ ] **Step 1: Create MediaProcessor**

`src/media/processor.rs`:
```rust
//! MediaProcessor — unified attachment processing entry point.
//!
//! Converts `Vec<Attachment>` into `Vec<ContentBlock>` for LLM consumption.
//! Images are either natively injected (vision-capable models) or described
//! via VisionPipeline. Audio is transcribed via TranscriptionService.

use crate::sync_primitives::Arc;
use tracing::{debug, warn};

use crate::gateway::channel::Attachment;
use crate::providers::message::ContentBlock;
use crate::vision::VisionPipeline;
use crate::vision::types::{ImageInput, ImageFormat};

use super::cache::{MediaCache, CachedMedia};
use super::transcription::TranscriptionService;

/// Classifies media by MIME type prefix
enum MediaType {
    Image,
    Audio,
    Other,
}

fn classify_media(mime: &str) -> MediaType {
    if mime.starts_with("image/") {
        MediaType::Image
    } else if mime.starts_with("audio/") {
        MediaType::Audio
    } else {
        MediaType::Other
    }
}

fn mime_to_image_format(mime: &str) -> ImageFormat {
    match mime {
        "image/png" => ImageFormat::Png,
        "image/webp" => ImageFormat::WebP,
        _ => ImageFormat::Jpeg, // Default for jpeg, gif, etc.
    }
}

/// Unified media processing service
pub struct MediaProcessor {
    cache: MediaCache,
    transcription: Option<Box<dyn TranscriptionService>>,
    vision: Option<Arc<VisionPipeline>>,
}

impl MediaProcessor {
    pub fn new(
        transcription: Option<Box<dyn TranscriptionService>>,
        vision: Option<Arc<VisionPipeline>>,
    ) -> Self {
        Self {
            cache: MediaCache::new(),
            transcription,
            vision,
        }
    }

    /// Process all attachments and return ContentBlocks for LLM context.
    ///
    /// - Images: native `ContentBlock::Image` if `supports_vision`, else description
    /// - Audio: transcription if available, else placeholder
    /// - Other: filename/type placeholder
    pub async fn process(
        &self,
        attachments: &[Attachment],
        supports_vision: bool,
        session_id: &str,
    ) -> Vec<ContentBlock> {
        let mut blocks = Vec::new();

        for attachment in attachments {
            match classify_media(&attachment.mime_type) {
                MediaType::Image => {
                    match self.process_image(attachment, supports_vision, session_id).await {
                        Ok(block) => blocks.push(block),
                        Err(e) => {
                            warn!("Failed to process image attachment: {}", e);
                            blocks.push(ContentBlock::Text {
                                text: "[Image: processing failed]".to_string(),
                            });
                        }
                    }
                }
                MediaType::Audio => {
                    match self.process_audio(attachment, session_id).await {
                        Ok(block) => blocks.push(block),
                        Err(e) => {
                            warn!("Failed to process audio attachment: {}", e);
                            blocks.push(ContentBlock::Text {
                                text: format!("[Audio: {}]", attachment.mime_type),
                            });
                        }
                    }
                }
                MediaType::Other => {
                    let label = attachment.filename.as_deref().unwrap_or("file");
                    blocks.push(ContentBlock::Text {
                        text: format!("[Attachment: {} ({})]", label, attachment.mime_type),
                    });
                }
            }
        }

        blocks
    }

    async fn process_image(
        &self,
        attachment: &Attachment,
        supports_vision: bool,
        session_id: &str,
    ) -> anyhow::Result<ContentBlock> {
        let cached = self.cache.resolve(attachment, session_id).await?;
        let b64 = self.cache.to_base64(&cached).await?;

        if supports_vision {
            // Native injection — LLM sees the raw image
            Ok(ContentBlock::Image {
                data: b64,
                mime_type: cached.mime_type,
            })
        } else if let Some(ref vision) = self.vision {
            // Fallback — describe via VisionPipeline
            let format = mime_to_image_format(&cached.mime_type);
            let input = ImageInput::Base64 { data: b64, format };
            match vision.understand_image(&input, "Describe this image concisely.").await {
                Ok(result) => Ok(ContentBlock::Text {
                    text: format!("[Image: {}]", result.description),
                }),
                Err(e) => {
                    warn!("Vision fallback failed: {}", e);
                    Ok(ContentBlock::Text {
                        text: "[Image: description unavailable]".to_string(),
                    })
                }
            }
        } else {
            Ok(ContentBlock::Text {
                text: "[Image: vision not configured]".to_string(),
            })
        }
    }

    async fn process_audio(
        &self,
        attachment: &Attachment,
        session_id: &str,
    ) -> anyhow::Result<ContentBlock> {
        if let Some(ref stt) = self.transcription {
            let cached = self.cache.resolve(attachment, session_id).await?;
            let result = stt.transcribe(&cached).await?;
            Ok(ContentBlock::Text {
                text: format!("[Voice message transcript]:\n\"{}\"", result.text),
            })
        } else {
            Ok(ContentBlock::Text {
                text: format!("[Audio: {}]", attachment.mime_type),
            })
        }
    }

    /// Cleanup cached files for a session
    pub async fn cleanup(&self, session_id: &str) {
        self.cache.cleanup_session(session_id).await;
    }

    /// Cleanup stale cache on startup
    pub async fn cleanup_stale(&self) {
        self.cache.cleanup_stale().await;
    }
}
```

- [ ] **Step 2: Update mod.rs**

```rust
pub mod cache;
pub mod processor;
pub mod transcription;
pub mod whisper;
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS (may need to adjust imports based on actual vision module paths)

- [ ] **Step 4: Commit**

```bash
git add src/media/ && git commit -m "media: add MediaProcessor for unified attachment processing"
```

---

## Task 4: Fix OpenAI protocol adapter for vision

**Files:**
- Modify: `src/providers/protocols/openai.rs:109-119`

- [ ] **Step 1: Fix convert_messages User handler**

In `openai.rs`, replace the `UnifiedMessage::User { content }` match arm (lines ~109-119):

```rust
UnifiedMessage::User { content } => {
    let has_images = content
        .iter()
        .any(|b| matches!(b, crate::providers::message::ContentBlock::Image { .. }));

    if has_images {
        // Multimodal: text + images
        use crate::providers::openai::types::{
            ContentBlock as OaiBlock, ImageUrl,
        };
        let blocks: Vec<OaiBlock> = content
            .iter()
            .filter_map(|b| match b {
                crate::providers::message::ContentBlock::Text { text } => {
                    Some(OaiBlock::Text { text: text.clone() })
                }
                crate::providers::message::ContentBlock::Image { data, mime_type } => {
                    Some(OaiBlock::ImageUrl {
                        image_url: ImageUrl {
                            url: format!("data:{};base64,{}", mime_type, data),
                            detail: Some("auto".to_string()),
                        },
                    })
                }
                _ => None,
            })
            .collect();
        result.push(Message {
            role: "user".to_string(),
            content: MessageContent::Multimodal { content: blocks },
        });
    } else {
        // Text-only (existing behavior, unchanged)
        let text = content
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join("\n");
        result.push(Message {
            role: "user".to_string(),
            content: MessageContent::Text { content: text },
        });
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS — verify the import paths match the actual codebase.

- [ ] **Step 3: Commit**

```bash
git add src/providers/protocols/openai.rs && git commit -m "openai: support ContentBlock::Image in convert_messages (vision)"
```

---

## Task 5: Add UnifiedMessage multimodal constructor

**Files:**
- Modify: `src/providers/message.rs`

- [ ] **Step 1: Add user_with_content() constructor**

In `message.rs`, in the `impl UnifiedMessage` block (after the existing `user()` method):

```rust
/// Create a user message with pre-built content blocks (for multimodal)
pub fn user_with_content(content: Vec<ContentBlock>) -> Self {
    Self::User { content }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/providers/message.rs && git commit -m "providers: add UnifiedMessage::user_with_content() for multimodal"
```

---

## Task 6: Add attachments to RunRequest

**Files:**
- Modify: `src/gateway/execution_engine/mod.rs:50-63` (RunRequest)
- Modify: `src/gateway/inbound_router/executor.rs:130-136` (construction)

- [ ] **Step 1: Add attachments field to RunRequest**

In `mod.rs`, add to the `RunRequest` struct:

```rust
pub struct RunRequest {
    pub run_id: String,
    pub input: String,
    pub session_key: SessionKey,
    pub timeout_secs: Option<u64>,
    pub metadata: HashMap<String, String>,
    /// Attachments from inbound message (images, audio, documents)
    pub attachments: Vec<crate::gateway::channel::Attachment>,
}
```

- [ ] **Step 2: Fix all RunRequest construction sites**

In `executor.rs:130-136`, add the attachments field:
```rust
let request = RunRequest {
    run_id: run_id.clone(),
    input: ctx.message.text.clone(),
    session_key: ctx.session_key.clone(),
    timeout_secs: None,
    metadata,
    attachments: ctx.message.attachments.clone(),
};
```

Run `cargo check -p alephcore` to find any other construction sites that need updating. Fix each one by adding `attachments: Vec::new()` for non-channel callers (e.g., fast-path, panel, tests).

- [ ] **Step 3: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "gateway: add attachments field to RunRequest"
```

---

## Task 7: Wire MediaProcessor into ExecutionEngine

**Files:**
- Modify: `src/gateway/execution_engine/engine.rs` (add field, store summary)
- Modify: `src/gateway/execution_engine/run_loop.rs` (call MediaProcessor, build multimodal message)
- Modify: `src/agent_loop/loop_core.rs` (add run_with_history_messages)

- [ ] **Step 1: Add MediaProcessor to ExecutionEngine**

In `engine.rs`, add to the struct (~line 55):
```rust
pub(super) media_processor: Option<Arc<crate::media::processor::MediaProcessor>>,
```

Initialize as `None` in the constructor. Add a setter method:
```rust
pub fn set_media_processor(&mut self, processor: Arc<crate::media::processor::MediaProcessor>) {
    self.media_processor = Some(processor);
}
```

- [ ] **Step 2: Store session summary with attachment markers**

In `engine.rs`, before the `add_message` call (~line 248), build a summary:

```rust
// Build session text with attachment markers
let mut session_text = request.input.clone();
for att in &request.attachments {
    let label = att.filename.as_deref().unwrap_or("file");
    if att.mime_type.starts_with("image/") {
        session_text.push_str(&format!("\n[Image attached: {}]", att.mime_type));
    } else if att.mime_type.starts_with("audio/") {
        session_text.push_str(&format!("\n[Audio attached: {}]", att.mime_type));
    } else {
        session_text.push_str(&format!("\n[Attachment: {} ({})]", label, att.mime_type));
    }
}
agent
    .add_message(&request.session_key, MessageRole::User, &session_text)
    .await;
```

- [ ] **Step 3: Add run_with_history_messages to AgentLoop**

In `loop_core.rs`, add a new method after `run_with_history`:

```rust
/// Run with pre-built messages (for multimodal — caller builds the first user message).
pub async fn run_with_history_messages(
    &self,
    messages: Vec<UnifiedMessage>,
    callback: &mut dyn LoopCallback,
) -> anyhow::Result<LoopRunResult> {
    let tool_infos: Vec<ToolInfo> = self
        .tool_registry
        .tool_definitions()
        .iter()
        .map(|td| ToolInfo {
            name: td.name.clone(),
            description: td.description.clone(),
            parameters_schema: Some(td.parameters.clone()),
        })
        .collect();
    let system_prompt = self.prompt_builder.build(&tool_infos, None);
    let tool_defs = self.tool_registry.tool_definitions();

    // Messages already include the current user message (potentially multimodal)
    // Continue with the existing loop logic...
    self.run_loop(messages, &system_prompt, &tool_defs, callback).await
}
```

Note: This assumes the existing loop logic can be extracted into a `run_loop` helper that both `run_with_history` and `run_with_history_messages` call. If the loop logic is inlined in `run_with_history`, extract it first.

- [ ] **Step 4: Process attachments in run_loop.rs**

In `run_loop.rs`, in the `run_agent_loop` function, after building history and before calling `agent_loop.run_with_history`:

```rust
// Process multimodal attachments
let has_attachments = !request.attachments.is_empty();
if has_attachments {
    if let Some(ref media_processor) = self.media_processor {
        // Determine if current model supports vision
        let supports_vision = /* query from agent/provider config */;

        let media_blocks = media_processor
            .process(&request.attachments, supports_vision, &request.session_key.to_string())
            .await;

        // Build multimodal user message
        let mut content = vec![ContentBlock::Text { text: request.input.clone() }];
        content.extend(media_blocks);
        history.push(UnifiedMessage::user_with_content(content));

        let result = agent_loop.run_with_history_messages(history, &mut callback).await?;

        // Cleanup media cache
        media_processor.cleanup(&request.session_key.to_string()).await;

        return Ok(result);
    }
}

// Existing non-multimodal path (unchanged)
let result = agent_loop.run_with_history(&request.input, history, &mut callback).await?;
```

- [ ] **Step 5: Resolve supports_vision**

The `supports_vision` flag needs to come from the model info. Look up how the provider/model is resolved in `run_loop.rs` and query `ProviderModelInfo.supports_vision`. If the model info is not directly available, default to `true` for known vision-capable providers (OpenAI, Anthropic) and `false` otherwise. This can be refined later.

- [ ] **Step 6: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "gateway: wire MediaProcessor into ExecutionEngine for multimodal support"
```

---

## Task 8: Wire MediaProcessor at server startup

**Files:**
- Modify: `src/bin/aleph-server/commands/start/` (server init code)

- [ ] **Step 1: Find server initialization**

Search for where `ExecutionEngine` is constructed during server startup. This is likely in the `start` command builder.

- [ ] **Step 2: Create and inject MediaProcessor**

After ExecutionEngine is created, create a MediaProcessor and inject it:

```rust
// Create MediaProcessor
let transcription: Option<Box<dyn TranscriptionService>> = {
    // Try to get OpenAI API key for Whisper
    if let Some(api_key) = provider_registry.get_api_key("openai") {
        Some(Box::new(WhisperTranscription::new(api_key, None, None)))
    } else {
        None
    }
};

let vision_pipeline = /* get existing VisionPipeline if available */;

let media_processor = Arc::new(MediaProcessor::new(transcription, vision_pipeline));
media_processor.cleanup_stale().await;
execution_engine.set_media_processor(media_processor);
```

Note: The exact wiring depends on how the server builder exposes the provider registry and vision pipeline. Follow the existing pattern for injecting optional services (like `session_compactor`).

- [ ] **Step 3: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "server: create and inject MediaProcessor at startup"
```

---

## Task 9: Final cleanup and verification

- [ ] **Step 1: Update media module docs**

In `src/media/mod.rs`, add module documentation:
```rust
//! Media processing pipeline for multimodal LLM interaction.
//!
//! Handles attachment download, caching, image injection, and audio transcription.
//! MediaProcessor is the unified entry point, owned by ExecutionEngine.
//!
//! # Data Flow
//!
//! ```text
//! Channel → InboundMessage.attachments
//!     → RunRequest.attachments
//!         → MediaProcessor.process()
//!             ├─ Image + vision → ContentBlock::Image (native)
//!             ├─ Image - vision → VisionPipeline → ContentBlock::Text
//!             ├─ Audio + STT    → WhisperAPI → ContentBlock::Text
//!             └─ Other          → ContentBlock::Text (placeholder)
//!         → UnifiedMessage::User { content: [Text, Image, ...] }
//!             → Provider adapter → LLM API call
//! ```

pub mod cache;
pub mod processor;
pub mod transcription;
pub mod whisper;
```

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: ALL PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore`
Expected: No new warnings from our code

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "media: final docs and cleanup for multimodal pipeline"
```
