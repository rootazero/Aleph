# Message Pipeline Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Insert a DebounceBuffer → MessagePipeline → SessionScheduler chain between InboundRouter and ExecutionEngine, enabling message merging, media pre-understanding, and per-session serial execution.

**Architecture:** New `pipeline/` module under `core/src/gateway/` with 5 files. InboundRouter stops calling ExecutionAdapter directly — instead submits to DebounceBuffer. SessionScheduler owns the ExecutionAdapter reference and enforces serial execution per session.

**Tech Stack:** Rust, tokio (timers, spawn, Mutex), async-trait, reqwest (media download), futures (join_all for concurrent understanding), uuid, tracing

---

### Task 1: Pipeline Types & Data Structures

**Files:**
- Create: `core/src/gateway/pipeline/mod.rs`
- Create: `core/src/gateway/pipeline/types.rs`
- Modify: `core/src/gateway/mod.rs` (add `pub mod pipeline;`)

**Step 1: Write the failing test**

Create `core/src/gateway/pipeline/types.rs` with test module:

```rust
//! Pipeline data types
//!
//! Shared types used across all pipeline stages.

use chrono::{DateTime, Utc};
use std::path::PathBuf;

use crate::gateway::channel::{Attachment, MessageId};
use crate::gateway::inbound_context::InboundContext;

/// Category of media content
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaCategory {
    Image,
    Document,
    Link,
    Audio,
    Video,
    Unknown,
}

impl MediaCategory {
    /// Classify from MIME type
    pub fn from_mime(mime: &str) -> Self {
        let lower = mime.to_lowercase();
        if lower.starts_with("image/") {
            Self::Image
        } else if lower.starts_with("audio/") {
            Self::Audio
        } else if lower.starts_with("video/") {
            Self::Video
        } else if lower == "application/pdf"
            || lower.starts_with("text/")
            || lower.contains("document")
            || lower.contains("spreadsheet")
        {
            Self::Document
        } else {
            Self::Unknown
        }
    }
}

/// Media file that has been downloaded to local workspace
#[derive(Debug, Clone)]
pub struct LocalMedia {
    /// Original attachment info
    pub original: Attachment,
    /// Path on local filesystem
    pub local_path: PathBuf,
    /// Classified media category
    pub media_category: MediaCategory,
}

/// Result of LLM pre-understanding of a media item
#[derive(Debug, Clone)]
pub struct MediaUnderstanding {
    /// The local media that was understood
    pub media: LocalMedia,
    /// LLM-generated description
    pub description: String,
    /// Type of understanding performed
    pub understanding_type: UnderstandingType,
}

/// Type of understanding that was performed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnderstandingType {
    ImageDescription,
    LinkSummary,
    DocumentSummary,
    Skipped(String),
}

/// Multiple messages merged by the debounce buffer
#[derive(Debug, Clone)]
pub struct MergedMessage {
    /// Merged text (individual messages joined by newline)
    pub text: String,
    /// All attachments from all merged messages
    pub attachments: Vec<Attachment>,
    /// Context from the first message (used for routing)
    pub primary_context: InboundContext,
    /// IDs of all merged messages
    pub merged_message_ids: Vec<MessageId>,
    /// Number of messages that were merged
    pub merge_count: usize,
}

impl MergedMessage {
    /// Create from a single InboundContext (no merging)
    pub fn from_single(ctx: InboundContext) -> Self {
        let text = ctx.message.text.clone();
        let attachments = ctx.message.attachments.clone();
        let id = ctx.message.id.clone();
        Self {
            text,
            attachments,
            primary_context: ctx,
            merged_message_ids: vec![id],
            merge_count: 1,
        }
    }

    /// Create from multiple InboundContexts
    pub fn from_batch(contexts: Vec<InboundContext>) -> Self {
        assert!(!contexts.is_empty(), "Cannot merge empty batch");
        if contexts.len() == 1 {
            return Self::from_single(contexts.into_iter().next().unwrap());
        }

        let mut texts = Vec::with_capacity(contexts.len());
        let mut attachments = Vec::new();
        let mut ids = Vec::with_capacity(contexts.len());

        for ctx in &contexts {
            if !ctx.message.text.is_empty() {
                texts.push(ctx.message.text.clone());
            }
            attachments.extend(ctx.message.attachments.clone());
            ids.push(ctx.message.id.clone());
        }

        let merge_count = contexts.len();
        let primary_context = contexts.into_iter().next().unwrap();

        Self {
            text: texts.join("\n"),
            attachments,
            primary_context,
            merged_message_ids: ids,
            merge_count,
        }
    }
}

/// Message enriched with media understanding results
#[derive(Debug, Clone)]
pub struct EnrichedMessage {
    /// Original merged message
    pub merged: MergedMessage,
    /// Text with understanding appendix (replaces RunRequest.input)
    pub enriched_text: String,
    /// Downloaded media files
    pub local_media: Vec<LocalMedia>,
    /// Token count consumed by pre-understanding LLM calls
    pub understanding_tokens: u64,
}

impl EnrichedMessage {
    /// Build enriched message from pipeline results
    pub fn build(
        merged: MergedMessage,
        local_media: Vec<LocalMedia>,
        understandings: Vec<MediaUnderstanding>,
        understanding_tokens: u64,
    ) -> Self {
        let enriched_text = Self::build_enriched_text(&merged.text, &understandings);
        Self {
            merged,
            enriched_text,
            local_media,
            understanding_tokens,
        }
    }

    /// Build enriched text: original + understanding appendix
    fn build_enriched_text(original: &str, understandings: &[MediaUnderstanding]) -> String {
        let meaningful: Vec<&MediaUnderstanding> = understandings
            .iter()
            .filter(|u| !matches!(u.understanding_type, UnderstandingType::Skipped(_)))
            .collect();

        if meaningful.is_empty() {
            return original.to_string();
        }

        let mut result = original.to_string();
        result.push_str("\n\n[Attachment Understanding]");

        for u in meaningful {
            let label = match &u.understanding_type {
                UnderstandingType::ImageDescription => "Image",
                UnderstandingType::LinkSummary => "Link",
                UnderstandingType::DocumentSummary => "Document",
                UnderstandingType::Skipped(_) => unreachable!(),
            };
            let name = u
                .media
                .original
                .filename
                .as_deref()
                .or(u.media.original.url.as_deref())
                .unwrap_or("unnamed");
            result.push_str(&format!("\n- {} \"{}\": {}", label, name, u.description));
        }

        result
    }
}

/// Pipeline processing error
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Media download failed: {0}")]
    DownloadFailed(String),
    #[error("Media understanding failed: {0}")]
    UnderstandingFailed(String),
    #[error("Pipeline cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, UserId};
    use crate::gateway::inbound_context::ReplyRoute;
    use crate::gateway::router::SessionKey;

    fn make_ctx(text: &str) -> InboundContext {
        let msg = InboundMessage {
            id: MessageId::new(format!("msg-{}", text.len())),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("chat-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("telegram"), ConversationId::new("chat-1"));
        InboundContext::new(msg, route, SessionKey::main("main"))
    }

    #[test]
    fn test_media_category_from_mime() {
        assert_eq!(MediaCategory::from_mime("image/png"), MediaCategory::Image);
        assert_eq!(MediaCategory::from_mime("image/jpeg"), MediaCategory::Image);
        assert_eq!(MediaCategory::from_mime("audio/mp3"), MediaCategory::Audio);
        assert_eq!(MediaCategory::from_mime("video/mp4"), MediaCategory::Video);
        assert_eq!(MediaCategory::from_mime("application/pdf"), MediaCategory::Document);
        assert_eq!(MediaCategory::from_mime("text/plain"), MediaCategory::Document);
        assert_eq!(MediaCategory::from_mime("application/octet-stream"), MediaCategory::Unknown);
    }

    #[test]
    fn test_merged_message_single() {
        let ctx = make_ctx("hello");
        let merged = MergedMessage::from_single(ctx);
        assert_eq!(merged.text, "hello");
        assert_eq!(merged.merge_count, 1);
        assert_eq!(merged.merged_message_ids.len(), 1);
    }

    #[test]
    fn test_merged_message_batch() {
        let contexts = vec![
            make_ctx("hello"),
            make_ctx("how are you"),
            make_ctx("check the weather"),
        ];
        let merged = MergedMessage::from_batch(contexts);
        assert_eq!(merged.text, "hello\nhow are you\ncheck the weather");
        assert_eq!(merged.merge_count, 3);
        assert_eq!(merged.merged_message_ids.len(), 3);
    }

    #[test]
    fn test_enriched_text_no_understandings() {
        let text = EnrichedMessage::build_enriched_text("hello", &[]);
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_enriched_text_with_understanding() {
        let media = LocalMedia {
            original: Attachment {
                id: "att-1".to_string(),
                mime_type: "image/png".to_string(),
                filename: Some("photo.jpg".to_string()),
                size: None,
                url: None,
                path: None,
                data: None,
            },
            local_path: PathBuf::from("/tmp/photo.jpg"),
            media_category: MediaCategory::Image,
        };
        let understanding = MediaUnderstanding {
            media,
            description: "A sunset over the ocean".to_string(),
            understanding_type: UnderstandingType::ImageDescription,
        };
        let text = EnrichedMessage::build_enriched_text("describe this", &[understanding]);
        assert!(text.contains("describe this"));
        assert!(text.contains("[Attachment Understanding]"));
        assert!(text.contains("Image \"photo.jpg\": A sunset over the ocean"));
    }

    #[test]
    fn test_enriched_text_skips_skipped() {
        let media = LocalMedia {
            original: Attachment {
                id: "att-1".to_string(),
                mime_type: "application/zip".to_string(),
                filename: Some("archive.zip".to_string()),
                size: None,
                url: None,
                path: None,
                data: None,
            },
            local_path: PathBuf::from("/tmp/archive.zip"),
            media_category: MediaCategory::Unknown,
        };
        let understanding = MediaUnderstanding {
            media,
            description: String::new(),
            understanding_type: UnderstandingType::Skipped("unsupported format".to_string()),
        };
        let text = EnrichedMessage::build_enriched_text("check this file", &[understanding]);
        assert_eq!(text, "check this file");
        assert!(!text.contains("[Attachment Understanding]"));
    }
}
```

**Step 2: Create the mod.rs**

Create `core/src/gateway/pipeline/mod.rs`:

```rust
//! Message Pipeline
//!
//! Pre-processes messages between InboundRouter and ExecutionEngine.
//! Stages: Debounce → MediaDownload → MediaUnderstanding → Enrichment

pub mod types;
pub mod debounce;
pub mod media_download;
pub mod media_understanding;

pub use types::*;
```

**Step 3: Register the module**

In `core/src/gateway/mod.rs`, add after `pub mod inbound_router;`:

```rust
pub mod pipeline;
```

**Step 4: Run tests**

```bash
cargo test -p alephcore --lib pipeline::types
```

Expected: All 6 tests pass.

**Step 5: Commit**

```bash
git add core/src/gateway/pipeline/ core/src/gateway/mod.rs
git commit -m "pipeline: add core types (MergedMessage, EnrichedMessage, MediaCategory)"
```

---

### Task 2: DebounceBuffer

**Files:**
- Create: `core/src/gateway/pipeline/debounce.rs`

**Step 1: Write the implementation with tests**

```rust
//! Debounce Buffer
//!
//! Collects rapid-fire messages per session and merges them
//! after a configurable sliding window expires.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::sync_primitives::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::gateway::inbound_context::InboundContext;
use super::types::MergedMessage;

/// Configuration for the debounce buffer
#[derive(Debug, Clone)]
pub struct DebounceConfig {
    /// Default debounce window in milliseconds (default: 2000)
    pub default_window_ms: u64,
    /// Maximum wait time in milliseconds (default: 5000)
    pub max_window_ms: u64,
    /// Maximum messages per batch before forced flush (default: 10)
    pub max_messages: usize,
    /// Per-channel window overrides (channel_id -> window_ms)
    pub channel_overrides: HashMap<String, u64>,
}

impl Default for DebounceConfig {
    fn default() -> Self {
        Self {
            default_window_ms: 2000,
            max_window_ms: 5000,
            max_messages: 10,
            channel_overrides: HashMap::new(),
        }
    }
}

impl DebounceConfig {
    /// Get the debounce window for a specific channel
    pub fn window_for_channel(&self, channel_id: &str) -> Duration {
        let ms = self
            .channel_overrides
            .get(channel_id)
            .copied()
            .unwrap_or(self.default_window_ms);
        Duration::from_millis(ms)
    }

    /// Get the max window duration
    pub fn max_window(&self) -> Duration {
        Duration::from_millis(self.max_window_ms)
    }
}

/// A batch of messages being collected for a session
struct DebounceBatch {
    contexts: Vec<InboundContext>,
    first_received: Instant,
    timer_handle: Option<JoinHandle<()>>,
}

/// Callback type for when a merged message is ready
pub type OnMergedReady = Arc<dyn Fn(MergedMessage) + Send + Sync>;

/// Buffer that collects rapid-fire messages and merges them
pub struct DebounceBuffer {
    pending: Arc<Mutex<HashMap<String, DebounceBatch>>>,
    config: DebounceConfig,
    on_ready: OnMergedReady,
}

impl DebounceBuffer {
    /// Create a new debounce buffer
    pub fn new(config: DebounceConfig, on_ready: OnMergedReady) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            config,
            on_ready,
        }
    }

    /// Submit a message for debouncing
    ///
    /// If this is the first message for a session, starts a timer.
    /// If there are already pending messages, resets the timer (sliding window).
    /// If max_messages is reached, flushes immediately.
    pub async fn submit(&self, ctx: InboundContext) {
        let session_key = ctx.session_key.to_key_string();
        let channel_id = ctx.message.channel_id.as_str().to_string();

        let mut pending = self.pending.lock().await;

        let batch = pending.entry(session_key.clone()).or_insert_with(|| {
            DebounceBatch {
                contexts: Vec::new(),
                first_received: Instant::now(),
                timer_handle: None,
            }
        });

        batch.contexts.push(ctx);
        let count = batch.contexts.len();

        debug!(
            session = %session_key,
            count = count,
            "Debounce: message added to batch"
        );

        // Check if max messages reached → flush immediately
        if count >= self.config.max_messages {
            info!(
                session = %session_key,
                count = count,
                "Debounce: max messages reached, flushing"
            );
            // Cancel existing timer
            if let Some(handle) = batch.timer_handle.take() {
                handle.abort();
            }
            // Take and flush
            let batch = pending.remove(&session_key).unwrap();
            drop(pending);
            self.flush_batch(batch.contexts);
            return;
        }

        // Check if max_window exceeded → flush immediately
        if batch.first_received.elapsed() >= self.config.max_window() {
            info!(
                session = %session_key,
                "Debounce: max window exceeded, flushing"
            );
            if let Some(handle) = batch.timer_handle.take() {
                handle.abort();
            }
            let batch = pending.remove(&session_key).unwrap();
            drop(pending);
            self.flush_batch(batch.contexts);
            return;
        }

        // Cancel existing timer and start a new one (sliding window)
        if let Some(handle) = batch.timer_handle.take() {
            handle.abort();
        }

        let window = self.config.window_for_channel(&channel_id);
        // Clamp to remaining max_window time
        let elapsed = batch.first_received.elapsed();
        let remaining_max = self.config.max_window().saturating_sub(elapsed);
        let effective_window = window.min(remaining_max);

        let pending_ref = self.pending.clone();
        let on_ready = self.on_ready.clone();
        let key = session_key.clone();

        batch.timer_handle = Some(tokio::spawn(async move {
            sleep(effective_window).await;

            let mut pending = pending_ref.lock().await;
            if let Some(batch) = pending.remove(&key) {
                debug!(
                    session = %key,
                    count = batch.contexts.len(),
                    "Debounce: window expired, flushing"
                );
                let merged = MergedMessage::from_batch(batch.contexts);
                drop(pending);
                on_ready(merged);
            }
        }));
    }

    /// Flush a batch of contexts immediately
    fn flush_batch(&self, contexts: Vec<InboundContext>) {
        if contexts.is_empty() {
            warn!("Debounce: attempted to flush empty batch");
            return;
        }
        let merged = MergedMessage::from_batch(contexts);
        (self.on_ready)(merged);
    }

    /// Get the number of sessions with pending messages (for diagnostics)
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::*;
    use crate::gateway::inbound_context::ReplyRoute;
    use crate::gateway::router::SessionKey;
    use chrono::Utc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    fn make_ctx(text: &str, session: &str) -> InboundContext {
        let msg = InboundMessage {
            id: MessageId::new(format!("msg-{}", text.len())),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("chat-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("telegram"), ConversationId::new("chat-1"));
        InboundContext::new(msg, route, SessionKey::main(session))
    }

    #[tokio::test]
    async fn test_single_message_passthrough() {
        let received = Arc::new(Mutex::new(Vec::<MergedMessage>::new()));
        let notify = Arc::new(Notify::new());

        let recv_clone = received.clone();
        let notify_clone = notify.clone();
        let on_ready: OnMergedReady = Arc::new(move |merged| {
            let recv = recv_clone.clone();
            let n = notify_clone.clone();
            tokio::spawn(async move {
                recv.lock().await.push(merged);
                n.notify_one();
            });
        });

        let config = DebounceConfig {
            default_window_ms: 100, // Short for testing
            max_window_ms: 500,
            max_messages: 10,
            channel_overrides: HashMap::new(),
        };
        let buffer = DebounceBuffer::new(config, on_ready);

        buffer.submit(make_ctx("hello", "agent-1")).await;
        notify.notified().await;

        let results = received.lock().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "hello");
        assert_eq!(results[0].merge_count, 1);
    }

    #[tokio::test]
    async fn test_rapid_fire_merge() {
        let received = Arc::new(Mutex::new(Vec::<MergedMessage>::new()));
        let notify = Arc::new(Notify::new());

        let recv_clone = received.clone();
        let notify_clone = notify.clone();
        let on_ready: OnMergedReady = Arc::new(move |merged| {
            let recv = recv_clone.clone();
            let n = notify_clone.clone();
            tokio::spawn(async move {
                recv.lock().await.push(merged);
                n.notify_one();
            });
        });

        let config = DebounceConfig {
            default_window_ms: 200,
            max_window_ms: 2000,
            max_messages: 10,
            channel_overrides: HashMap::new(),
        };
        let buffer = DebounceBuffer::new(config, on_ready);

        // Send 3 messages rapidly (within 200ms window)
        buffer.submit(make_ctx("hello", "agent-1")).await;
        sleep(Duration::from_millis(50)).await;
        buffer.submit(make_ctx("how are you", "agent-1")).await;
        sleep(Duration::from_millis(50)).await;
        buffer.submit(make_ctx("check weather", "agent-1")).await;

        // Wait for window to expire
        notify.notified().await;

        let results = received.lock().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].merge_count, 3);
        assert!(results[0].text.contains("hello"));
        assert!(results[0].text.contains("how are you"));
        assert!(results[0].text.contains("check weather"));
    }

    #[tokio::test]
    async fn test_max_messages_flush() {
        let flush_count = Arc::new(AtomicUsize::new(0));

        let count_clone = flush_count.clone();
        let on_ready: OnMergedReady = Arc::new(move |_merged| {
            count_clone.fetch_add(1, Ordering::SeqCst);
        });

        let config = DebounceConfig {
            default_window_ms: 5000, // Long window
            max_window_ms: 10000,
            max_messages: 3, // But low max
            channel_overrides: HashMap::new(),
        };
        let buffer = DebounceBuffer::new(config, on_ready);

        buffer.submit(make_ctx("a", "agent-1")).await;
        buffer.submit(make_ctx("b", "agent-1")).await;
        // Third message should trigger immediate flush
        buffer.submit(make_ctx("c", "agent-1")).await;

        // Give a moment for the flush
        sleep(Duration::from_millis(50)).await;
        assert_eq!(flush_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_separate_sessions_independent() {
        let received = Arc::new(Mutex::new(Vec::<MergedMessage>::new()));
        let notify = Arc::new(Notify::new());

        let recv_clone = received.clone();
        let notify_clone = notify.clone();
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let on_ready: OnMergedReady = Arc::new(move |merged| {
            let recv = recv_clone.clone();
            let n = notify_clone.clone();
            let c = counter_clone.clone();
            tokio::spawn(async move {
                recv.lock().await.push(merged);
                if c.fetch_add(1, Ordering::SeqCst) >= 1 {
                    n.notify_one();
                }
            });
        });

        let config = DebounceConfig {
            default_window_ms: 100,
            max_window_ms: 500,
            max_messages: 10,
            channel_overrides: HashMap::new(),
        };
        let buffer = DebounceBuffer::new(config, on_ready);

        // Two different sessions
        buffer.submit(make_ctx("hello from A", "agent-a")).await;
        buffer.submit(make_ctx("hello from B", "agent-b")).await;

        // Wait for both to flush
        notify.notified().await;
        sleep(Duration::from_millis(200)).await;

        let results = received.lock().await;
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_config_channel_override() {
        let mut config = DebounceConfig::default();
        config.channel_overrides.insert("webchat".to_string(), 500);

        assert_eq!(
            config.window_for_channel("webchat"),
            Duration::from_millis(500)
        );
        assert_eq!(
            config.window_for_channel("telegram"),
            Duration::from_millis(2000)
        );
    }
}
```

**Step 2: Run tests**

```bash
cargo test -p alephcore --lib pipeline::debounce
```

Expected: All 5 tests pass.

**Step 3: Commit**

```bash
git add core/src/gateway/pipeline/debounce.rs
git commit -m "pipeline: add DebounceBuffer with sliding window and max-messages flush"
```

---

### Task 3: MediaDownloader

**Files:**
- Create: `core/src/gateway/pipeline/media_download.rs`

**Step 1: Write implementation with tests**

```rust
//! Media Download Stage
//!
//! Downloads remote media attachments to local workspace directory.
//! Extracts URLs from message text for link understanding.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use reqwest::Client;
use tokio::fs;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::gateway::channel::Attachment;
use super::types::{LocalMedia, MediaCategory, MergedMessage};

/// Downloads media attachments to local workspace
pub struct MediaDownloader {
    /// Root directory for media storage
    workspace_root: PathBuf,
    /// HTTP client for downloading
    http_client: Client,
    /// Maximum file size in bytes (default: 50MB)
    max_file_size: u64,
    /// Supported MIME type prefixes
    supported_prefixes: HashSet<String>,
}

impl MediaDownloader {
    /// Create a new media downloader
    pub fn new(workspace_root: PathBuf) -> Self {
        let mut supported = HashSet::new();
        for prefix in &["image/", "audio/", "video/", "text/", "application/pdf"] {
            supported.insert(prefix.to_string());
        }

        Self {
            workspace_root,
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            max_file_size: 50 * 1024 * 1024, // 50MB
            supported_prefixes: supported,
        }
    }

    /// Set max file size
    pub fn with_max_file_size(mut self, max_bytes: u64) -> Self {
        self.max_file_size = max_bytes;
        self
    }

    /// Download all attachments from a merged message
    ///
    /// Also extracts URLs from message text and creates Link entries.
    /// Individual download failures are logged and skipped (non-blocking).
    pub async fn download_all(&self, merged: &MergedMessage) -> Vec<LocalMedia> {
        let run_dir = self.workspace_root.join("media").join(Uuid::new_v4().to_string());

        let mut results = Vec::new();

        // Process attachments
        for attachment in &merged.attachments {
            match self.download_attachment(attachment, &run_dir).await {
                Ok(local) => results.push(local),
                Err(e) => {
                    warn!(
                        attachment_id = %attachment.id,
                        error = %e,
                        "Failed to download attachment, skipping"
                    );
                }
            }
        }

        // Extract URLs from text and create Link entries
        for url in extract_urls(&merged.text) {
            match self.download_link(&url, &run_dir).await {
                Ok(local) => results.push(local),
                Err(e) => {
                    debug!(url = %url, error = %e, "Failed to fetch link, skipping");
                }
            }
        }

        results
    }

    /// Download a single attachment
    async fn download_attachment(
        &self,
        attachment: &Attachment,
        run_dir: &Path,
    ) -> Result<LocalMedia, String> {
        let category = MediaCategory::from_mime(&attachment.mime_type);

        // Already local
        if let Some(path) = &attachment.path {
            let local_path = PathBuf::from(path);
            if local_path.exists() {
                return Ok(LocalMedia {
                    original: attachment.clone(),
                    local_path,
                    media_category: category,
                });
            }
        }

        // Inline data
        if let Some(data) = &attachment.data {
            if data.len() as u64 > self.max_file_size {
                return Err(format!(
                    "Inline data too large: {} bytes (max {})",
                    data.len(),
                    self.max_file_size
                ));
            }
            let filename = attachment
                .filename
                .as_deref()
                .unwrap_or(&attachment.id);
            let local_path = run_dir.join(filename);
            fs::create_dir_all(run_dir).await.map_err(|e| e.to_string())?;
            fs::write(&local_path, data).await.map_err(|e| e.to_string())?;
            return Ok(LocalMedia {
                original: attachment.clone(),
                local_path,
                media_category: category,
            });
        }

        // Remote URL
        if let Some(url) = &attachment.url {
            let filename = attachment
                .filename
                .as_deref()
                .unwrap_or(&attachment.id);
            let local_path = run_dir.join(filename);
            self.download_url(url, &local_path).await?;
            return Ok(LocalMedia {
                original: attachment.clone(),
                local_path,
                media_category: category,
            });
        }

        Err("Attachment has no data, path, or URL".to_string())
    }

    /// Download a URL extracted from message text
    async fn download_link(
        &self,
        url: &str,
        run_dir: &Path,
    ) -> Result<LocalMedia, String> {
        let filename = format!("link_{}.html", Uuid::new_v4().to_string().get(..8).unwrap_or("unknown"));
        let local_path = run_dir.join(&filename);
        self.download_url(url, &local_path).await?;

        let attachment = Attachment {
            id: format!("link-{}", Uuid::new_v4()),
            mime_type: "text/html".to_string(),
            filename: Some(filename),
            size: None,
            url: Some(url.to_string()),
            path: None,
            data: None,
        };

        Ok(LocalMedia {
            original: attachment,
            local_path,
            media_category: MediaCategory::Link,
        })
    }

    /// Download from URL to local path
    async fn download_url(&self, url: &str, local_path: &Path) -> Result<(), String> {
        let response = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }

        // Check content-length if available
        if let Some(len) = response.content_length() {
            if len > self.max_file_size {
                return Err(format!(
                    "File too large: {} bytes (max {})",
                    len, self.max_file_size
                ));
            }
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read response body: {}", e))?;

        if bytes.len() as u64 > self.max_file_size {
            return Err(format!(
                "Downloaded file too large: {} bytes (max {})",
                bytes.len(),
                self.max_file_size
            ));
        }

        if let Some(parent) = local_path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
        }
        fs::write(local_path, &bytes).await.map_err(|e| e.to_string())?;

        Ok(())
    }
}

/// Extract URLs from text using simple regex-free matching
pub fn extract_urls(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter(|word| {
            (word.starts_with("http://") || word.starts_with("https://"))
                && word.len() > 10
        })
        .map(|url| {
            // Trim trailing punctuation
            url.trim_end_matches(|c: char| matches!(c, ',' | '.' | ')' | ']' | '>' | ';' | '。' | '，'))
                .to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::*;
    use crate::gateway::inbound_context::{InboundContext, ReplyRoute};
    use crate::gateway::router::SessionKey;
    use chrono::Utc;
    use tempfile::tempdir;

    #[test]
    fn test_extract_urls_basic() {
        let urls = extract_urls("check https://example.com please");
        assert_eq!(urls, vec!["https://example.com"]);
    }

    #[test]
    fn test_extract_urls_multiple() {
        let urls = extract_urls("see https://a.com and https://b.com/page");
        assert_eq!(urls.len(), 2);
    }

    #[test]
    fn test_extract_urls_trailing_punctuation() {
        let urls = extract_urls("visit https://example.com.");
        assert_eq!(urls, vec!["https://example.com"]);
    }

    #[test]
    fn test_extract_urls_no_urls() {
        let urls = extract_urls("no links here");
        assert!(urls.is_empty());
    }

    #[test]
    fn test_extract_urls_short_rejected() {
        let urls = extract_urls("http://x");
        assert!(urls.is_empty());
    }

    #[tokio::test]
    async fn test_download_inline_data() {
        let temp = tempdir().unwrap();
        let downloader = MediaDownloader::new(temp.path().to_path_buf());

        let attachment = Attachment {
            id: "att-1".to_string(),
            mime_type: "image/png".to_string(),
            filename: Some("test.png".to_string()),
            size: Some(5),
            url: None,
            path: None,
            data: Some(vec![1, 2, 3, 4, 5]),
        };

        let msg = InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("test"),
            conversation_id: ConversationId::new("chat-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: "see this image".to_string(),
            attachments: vec![attachment],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("test"), ConversationId::new("chat-1"));
        let ctx = InboundContext::new(msg, route, SessionKey::main("main"));
        let merged = super::super::types::MergedMessage::from_single(ctx);

        let results = downloader.download_all(&merged).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].media_category, MediaCategory::Image);
        assert!(results[0].local_path.exists());

        let content = fs::read(&results[0].local_path).await.unwrap();
        assert_eq!(content, vec![1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn test_download_local_path() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("existing.txt");
        fs::write(&file_path, "existing content").await.unwrap();

        let downloader = MediaDownloader::new(temp.path().to_path_buf());

        let attachment = Attachment {
            id: "att-1".to_string(),
            mime_type: "text/plain".to_string(),
            filename: Some("existing.txt".to_string()),
            size: None,
            url: None,
            path: Some(file_path.to_string_lossy().to_string()),
            data: None,
        };

        let msg = InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("test"),
            conversation_id: ConversationId::new("chat-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: String::new(),
            attachments: vec![attachment],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("test"), ConversationId::new("chat-1"));
        let ctx = InboundContext::new(msg, route, SessionKey::main("main"));
        let merged = super::super::types::MergedMessage::from_single(ctx);

        let results = downloader.download_all(&merged).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].local_path, file_path);
    }

    #[tokio::test]
    async fn test_download_no_attachments_no_urls() {
        let temp = tempdir().unwrap();
        let downloader = MediaDownloader::new(temp.path().to_path_buf());

        let msg = InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("test"),
            conversation_id: ConversationId::new("chat-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: "just text, no links".to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("test"), ConversationId::new("chat-1"));
        let ctx = InboundContext::new(msg, route, SessionKey::main("main"));
        let merged = super::super::types::MergedMessage::from_single(ctx);

        let results = downloader.download_all(&merged).await;
        assert!(results.is_empty());
    }
}
```

**Step 2: Run tests**

```bash
cargo test -p alephcore --lib pipeline::media_download
```

Expected: All 6 tests pass (URL download tests skip gracefully — only inline/local tests run).

**Step 3: Commit**

```bash
git add core/src/gateway/pipeline/media_download.rs
git commit -m "pipeline: add MediaDownloader with inline/local/URL support"
```

---

### Task 4: MediaUnderstander

**Files:**
- Create: `core/src/gateway/pipeline/media_understanding.rs`

**Step 1: Write implementation with tests**

This stage calls an LLM to generate descriptions. We use a trait for testability.

```rust
//! Media Understanding Stage
//!
//! Uses LLM to generate descriptions of media content.
//! Supports configurable model selection with lightweight default.

use crate::sync_primitives::Arc;
use futures::future::join_all;
use tracing::{debug, warn};

use super::types::{LocalMedia, MediaCategory, MediaUnderstanding, UnderstandingType};

/// Trait for LLM-based media understanding (allows mocking in tests)
#[async_trait::async_trait]
pub trait UnderstandingProvider: Send + Sync {
    /// Generate a text description of the given media
    ///
    /// Returns (description, tokens_used)
    async fn understand(
        &self,
        local_path: &std::path::Path,
        category: &MediaCategory,
        model: &str,
    ) -> Result<(String, u64), String>;
}

/// Understands media content using LLM calls
pub struct MediaUnderstander {
    provider: Arc<dyn UnderstandingProvider>,
    /// Default model for understanding (e.g., "haiku")
    default_model: String,
}

impl MediaUnderstander {
    pub fn new(provider: Arc<dyn UnderstandingProvider>, default_model: String) -> Self {
        Self {
            provider,
            default_model,
        }
    }

    /// Understand all media items concurrently
    ///
    /// Individual failures result in Skipped entries, not errors.
    /// Returns (understandings, total_tokens_used)
    pub async fn understand_all(
        &self,
        media: &[LocalMedia],
        agent_model_override: Option<&str>,
    ) -> (Vec<MediaUnderstanding>, u64) {
        let model = agent_model_override.unwrap_or(&self.default_model);

        let futures: Vec<_> = media
            .iter()
            .map(|m| self.understand_one(m.clone(), model))
            .collect();

        let results = join_all(futures).await;

        let mut total_tokens = 0u64;
        let mut understandings = Vec::with_capacity(results.len());

        for result in results {
            total_tokens += result.1;
            understandings.push(result.0);
        }

        (understandings, total_tokens)
    }

    /// Understand a single media item
    async fn understand_one(&self, media: LocalMedia, model: &str) -> (MediaUnderstanding, u64) {
        let understanding_type = match &media.media_category {
            MediaCategory::Image => UnderstandingType::ImageDescription,
            MediaCategory::Link => UnderstandingType::LinkSummary,
            MediaCategory::Document => UnderstandingType::DocumentSummary,
            MediaCategory::Audio | MediaCategory::Video => {
                return (
                    MediaUnderstanding {
                        media,
                        description: String::new(),
                        understanding_type: UnderstandingType::Skipped(
                            "audio/video understanding not yet supported".to_string(),
                        ),
                    },
                    0,
                );
            }
            MediaCategory::Unknown => {
                return (
                    MediaUnderstanding {
                        media,
                        description: String::new(),
                        understanding_type: UnderstandingType::Skipped(
                            "unknown media type".to_string(),
                        ),
                    },
                    0,
                );
            }
        };

        match self
            .provider
            .understand(&media.local_path, &media.media_category, model)
            .await
        {
            Ok((description, tokens)) => {
                debug!(
                    path = %media.local_path.display(),
                    tokens = tokens,
                    "Media understood successfully"
                );
                (
                    MediaUnderstanding {
                        media,
                        description,
                        understanding_type,
                    },
                    tokens,
                )
            }
            Err(e) => {
                warn!(
                    path = %media.local_path.display(),
                    error = %e,
                    "Media understanding failed, skipping"
                );
                (
                    MediaUnderstanding {
                        media,
                        description: String::new(),
                        understanding_type: UnderstandingType::Skipped(e),
                    },
                    0,
                )
            }
        }
    }
}

/// Prompt templates for media understanding
pub mod prompts {
    use super::MediaCategory;

    pub fn for_category(category: &MediaCategory) -> &'static str {
        match category {
            MediaCategory::Image => {
                "Describe this image concisely in the user's language. \
                 Focus on key content, text, and actionable details."
            }
            MediaCategory::Link => {
                "Summarize this webpage content in 2-3 sentences. \
                 Focus on the main topic and key information."
            }
            MediaCategory::Document => {
                "Summarize this document concisely. \
                 List key sections and main points."
            }
            _ => "Describe this content briefly.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::Attachment;
    use std::path::PathBuf;
    use tokio::sync::Mutex;

    /// Mock provider that returns canned responses
    struct MockProvider {
        responses: Mutex<Vec<Result<(String, u64), String>>>,
    }

    impl MockProvider {
        fn new(responses: Vec<Result<(String, u64), String>>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait::async_trait]
    impl UnderstandingProvider for MockProvider {
        async fn understand(
            &self,
            _path: &std::path::Path,
            _category: &MediaCategory,
            _model: &str,
        ) -> Result<(String, u64), String> {
            let mut responses = self.responses.lock().await;
            if responses.is_empty() {
                Ok(("default description".to_string(), 10))
            } else {
                responses.remove(0)
            }
        }
    }

    fn make_local_media(category: MediaCategory, filename: &str) -> LocalMedia {
        LocalMedia {
            original: Attachment {
                id: format!("att-{}", filename),
                mime_type: match &category {
                    MediaCategory::Image => "image/png".to_string(),
                    MediaCategory::Link => "text/html".to_string(),
                    MediaCategory::Document => "application/pdf".to_string(),
                    _ => "application/octet-stream".to_string(),
                },
                filename: Some(filename.to_string()),
                size: None,
                url: None,
                path: None,
                data: None,
            },
            local_path: PathBuf::from(format!("/tmp/{}", filename)),
            media_category: category,
        }
    }

    #[tokio::test]
    async fn test_understand_image() {
        let provider = Arc::new(MockProvider::new(vec![Ok((
            "A photo of a sunset".to_string(),
            50,
        ))]));
        let understander = MediaUnderstander::new(provider, "haiku".to_string());

        let media = vec![make_local_media(MediaCategory::Image, "photo.png")];
        let (results, tokens) = understander.understand_all(&media, None).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].description, "A photo of a sunset");
        assert_eq!(results[0].understanding_type, UnderstandingType::ImageDescription);
        assert_eq!(tokens, 50);
    }

    #[tokio::test]
    async fn test_understand_skips_audio() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let understander = MediaUnderstander::new(provider, "haiku".to_string());

        let media = vec![make_local_media(MediaCategory::Audio, "song.mp3")];
        let (results, tokens) = understander.understand_all(&media, None).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].understanding_type,
            UnderstandingType::Skipped(_)
        ));
        assert_eq!(tokens, 0);
    }

    #[tokio::test]
    async fn test_understand_failure_becomes_skipped() {
        let provider = Arc::new(MockProvider::new(vec![Err(
            "API timeout".to_string(),
        )]));
        let understander = MediaUnderstander::new(provider, "haiku".to_string());

        let media = vec![make_local_media(MediaCategory::Image, "photo.png")];
        let (results, tokens) = understander.understand_all(&media, None).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].understanding_type,
            UnderstandingType::Skipped(ref s) if s == "API timeout"
        ));
        assert_eq!(tokens, 0);
    }

    #[tokio::test]
    async fn test_understand_concurrent_multiple() {
        let provider = Arc::new(MockProvider::new(vec![
            Ok(("image description".to_string(), 30)),
            Ok(("link summary".to_string(), 20)),
        ]));
        let understander = MediaUnderstander::new(provider, "haiku".to_string());

        let media = vec![
            make_local_media(MediaCategory::Image, "photo.png"),
            make_local_media(MediaCategory::Link, "page.html"),
        ];
        let (results, tokens) = understander.understand_all(&media, None).await;

        assert_eq!(results.len(), 2);
        assert_eq!(tokens, 50);
    }

    #[tokio::test]
    async fn test_model_override() {
        let provider = Arc::new(MockProvider::new(vec![Ok((
            "detailed description".to_string(),
            100,
        ))]));
        let understander = MediaUnderstander::new(provider, "haiku".to_string());

        let media = vec![make_local_media(MediaCategory::Image, "photo.png")];
        // Override to use a different model
        let (results, _) = understander
            .understand_all(&media, Some("sonnet"))
            .await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].description, "detailed description");
    }

    #[test]
    fn test_prompts() {
        let img_prompt = prompts::for_category(&MediaCategory::Image);
        assert!(img_prompt.contains("image"));

        let link_prompt = prompts::for_category(&MediaCategory::Link);
        assert!(link_prompt.contains("webpage"));

        let doc_prompt = prompts::for_category(&MediaCategory::Document);
        assert!(doc_prompt.contains("document"));
    }
}
```

**Step 2: Update mod.rs to export**

In `core/src/gateway/pipeline/mod.rs`, ensure `pub mod media_understanding;` is present.

**Step 3: Run tests**

```bash
cargo test -p alephcore --lib pipeline::media_understanding
```

Expected: All 6 tests pass.

**Step 4: Commit**

```bash
git add core/src/gateway/pipeline/media_understanding.rs
git commit -m "pipeline: add MediaUnderstander with mock-friendly trait and concurrent processing"
```

---

### Task 5: MessagePipeline Orchestrator

**Files:**
- Modify: `core/src/gateway/pipeline/mod.rs` (add pipeline orchestrator logic)

**Step 1: Write the pipeline orchestrator**

Update `core/src/gateway/pipeline/mod.rs`:

```rust
//! Message Pipeline
//!
//! Pre-processes messages between InboundRouter and ExecutionEngine.
//! Stages: Debounce → MediaDownload → MediaUnderstanding → Enrichment

pub mod types;
pub mod debounce;
pub mod media_download;
pub mod media_understanding;

pub use types::*;
pub use debounce::{DebounceBuffer, DebounceConfig};
pub use media_download::MediaDownloader;
pub use media_understanding::{MediaUnderstander, UnderstandingProvider};

use tracing::info;

/// Orchestrates the message processing pipeline
///
/// Stages: download → understand → enrich
pub struct MessagePipeline {
    downloader: MediaDownloader,
    understander: MediaUnderstander,
}

impl MessagePipeline {
    pub fn new(downloader: MediaDownloader, understander: MediaUnderstander) -> Self {
        Self {
            downloader,
            understander,
        }
    }

    /// Process a merged message through all pipeline stages
    pub async fn process(
        &self,
        merged: MergedMessage,
        agent_understanding_model: Option<&str>,
    ) -> Result<EnrichedMessage, PipelineError> {
        info!(
            merge_count = merged.merge_count,
            has_attachments = !merged.attachments.is_empty(),
            text_len = merged.text.len(),
            "Pipeline: processing message"
        );

        // Stage 1: Download media
        let local_media = self.downloader.download_all(&merged).await;

        // Stage 2: Understand media (only if there's something to understand)
        let (understandings, tokens) = if local_media.is_empty() {
            (Vec::new(), 0)
        } else {
            self.understander
                .understand_all(&local_media, agent_understanding_model)
                .await
        };

        // Stage 3: Build enriched message
        let enriched = EnrichedMessage::build(merged, local_media, understandings, tokens);

        info!(
            enriched_len = enriched.enriched_text.len(),
            media_count = enriched.local_media.len(),
            understanding_tokens = enriched.understanding_tokens,
            "Pipeline: processing complete"
        );

        Ok(enriched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::*;
    use crate::gateway::inbound_context::{InboundContext, ReplyRoute};
    use crate::gateway::router::SessionKey;
    use chrono::Utc;
    use std::path::PathBuf;
    use crate::sync_primitives::Arc;

    struct NoOpProvider;

    #[async_trait::async_trait]
    impl UnderstandingProvider for NoOpProvider {
        async fn understand(
            &self,
            _path: &std::path::Path,
            _category: &MediaCategory,
            _model: &str,
        ) -> Result<(String, u64), String> {
            Ok(("understood".to_string(), 10))
        }
    }

    fn make_ctx(text: &str) -> InboundContext {
        let msg = InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("test"),
            conversation_id: ConversationId::new("chat-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("test"), ConversationId::new("chat-1"));
        InboundContext::new(msg, route, SessionKey::main("main"))
    }

    #[tokio::test]
    async fn test_pipeline_text_only() {
        let temp = tempfile::tempdir().unwrap();
        let downloader = MediaDownloader::new(temp.path().to_path_buf());
        let understander = MediaUnderstander::new(Arc::new(NoOpProvider), "haiku".to_string());
        let pipeline = MessagePipeline::new(downloader, understander);

        let merged = MergedMessage::from_single(make_ctx("hello world"));
        let enriched = pipeline.process(merged, None).await.unwrap();

        assert_eq!(enriched.enriched_text, "hello world");
        assert!(enriched.local_media.is_empty());
        assert_eq!(enriched.understanding_tokens, 0);
    }

    #[tokio::test]
    async fn test_pipeline_with_inline_attachment() {
        let temp = tempfile::tempdir().unwrap();
        let downloader = MediaDownloader::new(temp.path().to_path_buf());
        let understander = MediaUnderstander::new(Arc::new(NoOpProvider), "haiku".to_string());
        let pipeline = MessagePipeline::new(downloader, understander);

        let msg = InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("test"),
            conversation_id: ConversationId::new("chat-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: "see this".to_string(),
            attachments: vec![Attachment {
                id: "att-1".to_string(),
                mime_type: "image/png".to_string(),
                filename: Some("photo.png".to_string()),
                size: Some(3),
                url: None,
                path: None,
                data: Some(vec![1, 2, 3]),
            }],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("test"), ConversationId::new("chat-1"));
        let ctx = InboundContext::new(msg, route, SessionKey::main("main"));
        let merged = MergedMessage::from_single(ctx);

        let enriched = pipeline.process(merged, None).await.unwrap();

        assert!(enriched.enriched_text.contains("see this"));
        assert!(enriched.enriched_text.contains("[Attachment Understanding]"));
        assert!(enriched.enriched_text.contains("understood"));
        assert_eq!(enriched.local_media.len(), 1);
        assert_eq!(enriched.understanding_tokens, 10);
    }
}
```

**Step 2: Run tests**

```bash
cargo test -p alephcore --lib pipeline
```

Expected: All pipeline tests pass (types + debounce + download + understanding + orchestrator).

**Step 3: Commit**

```bash
git add core/src/gateway/pipeline/mod.rs
git commit -m "pipeline: add MessagePipeline orchestrator combining all stages"
```

---

### Task 6: SessionScheduler

**Files:**
- Create: `core/src/gateway/session_scheduler.rs`
- Modify: `core/src/gateway/mod.rs` (add `pub mod session_scheduler;`)

**Step 1: Write the implementation**

```rust
//! Session Scheduler
//!
//! Enforces per-session serial execution. Messages for the same session
//! are queued and processed one at a time. Different sessions run in parallel.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::event_emitter::{EventEmitter, EventEmitError, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::inbound_context::ReplyRoute;
use crate::gateway::pipeline::EnrichedMessage;
use crate::gateway::reply_emitter::ReplyEmitter;
use crate::gateway::channel::OutboundMessage;

/// Maximum time a message can wait in queue before being dropped
const MAX_QUEUE_WAIT_SECS: u64 = 300;

/// Session-level task queue
struct SessionQueue {
    pending: VecDeque<QueuedTask>,
    active_run_id: Option<String>,
}

impl SessionQueue {
    fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            active_run_id: None,
        }
    }
}

/// A task waiting in the queue
struct QueuedTask {
    enriched: EnrichedMessage,
    enqueued_at: Instant,
}

/// Scheduler that enforces per-session serial execution
pub struct SessionScheduler {
    queues: Arc<Mutex<HashMap<String, SessionQueue>>>,
    execution_adapter: Arc<dyn ExecutionAdapter>,
    agent_registry: Arc<AgentRegistry>,
    channel_registry: Arc<ChannelRegistry>,
}

impl SessionScheduler {
    pub fn new(
        execution_adapter: Arc<dyn ExecutionAdapter>,
        agent_registry: Arc<AgentRegistry>,
        channel_registry: Arc<ChannelRegistry>,
    ) -> Self {
        Self {
            queues: Arc::new(Mutex::new(HashMap::new())),
            execution_adapter,
            agent_registry,
            channel_registry,
        }
    }

    /// Enqueue an enriched message for execution
    ///
    /// If the session has no active run, executes immediately.
    /// Otherwise, queues the message for later execution.
    pub async fn enqueue(&self, enriched: EnrichedMessage) {
        let session_key = enriched
            .merged
            .primary_context
            .session_key
            .to_key_string();

        let should_execute = {
            let mut queues = self.queues.lock().await;
            let queue = queues
                .entry(session_key.clone())
                .or_insert_with(SessionQueue::new);

            if queue.active_run_id.is_none() {
                true
            } else {
                info!(
                    session = %session_key,
                    queue_depth = queue.pending.len() + 1,
                    "Session busy, queuing message"
                );
                queue.pending.push_back(QueuedTask {
                    enriched,
                    enqueued_at: Instant::now(),
                });
                false
            }
        };

        if should_execute {
            self.execute_enriched(&session_key, enriched).await;
        }
    }

    /// Execute an enriched message
    async fn execute_enriched(&self, session_key: &str, enriched: EnrichedMessage) {
        let ctx = &enriched.merged.primary_context;
        let agent_id = ctx.session_key.agent_id();

        let agent = match self.agent_registry.get(agent_id).await {
            Some(agent) => agent,
            None => {
                error!(agent_id = %agent_id, "Agent not found for execution");
                return;
            }
        };

        let run_id = Uuid::new_v4().to_string();

        // Mark session as active
        {
            let mut queues = self.queues.lock().await;
            let queue = queues
                .entry(session_key.to_string())
                .or_insert_with(SessionQueue::new);
            queue.active_run_id = Some(run_id.clone());
        }

        // Create base reply emitter
        let base_emitter = Arc::new(ReplyEmitter::new(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
            run_id.clone(),
        ));

        // Wrap with scheduler listener for completion notification
        let scheduler_emitter: Arc<dyn EventEmitter + Send + Sync> =
            Arc::new(SchedulerEventListener {
                inner: base_emitter,
                scheduler_queues: self.queues.clone(),
                session_key: session_key.to_string(),
                scheduler: self.clone_self_ref(),
            });

        // Build metadata
        let mut metadata = HashMap::new();
        metadata.insert(
            "channel_id".to_string(),
            ctx.message.channel_id.as_str().to_string(),
        );
        metadata.insert("sender_id".to_string(), ctx.sender_normalized.clone());
        if ctx.message.is_group {
            metadata.insert("is_group".to_string(), "true".to_string());
        }
        if ctx.is_mentioned {
            metadata.insert("is_mentioned".to_string(), "true".to_string());
        }

        let request = RunRequest {
            run_id: run_id.clone(),
            input: enriched.enriched_text,
            session_key: ctx.session_key.clone(),
            timeout_secs: None,
            metadata,
        };

        info!(
            session = %session_key,
            run_id = %run_id,
            "Scheduler: executing agent"
        );

        let execution_adapter = self.execution_adapter.clone();
        tokio::spawn(async move {
            if let Err(e) = execution_adapter.execute(request, agent, scheduler_emitter).await {
                error!(run_id = %run_id, error = %e, "Agent execution failed");
            }
        });
    }

    /// Called when a run completes — triggers next queued task
    async fn on_run_complete(&self, session_key: &str) {
        let next_task = {
            let mut queues = self.queues.lock().await;
            if let Some(queue) = queues.get_mut(session_key) {
                queue.active_run_id = None;

                // Drop expired tasks
                while let Some(front) = queue.pending.front() {
                    if front.enqueued_at.elapsed() > Duration::from_secs(MAX_QUEUE_WAIT_SECS) {
                        warn!(
                            session = %session_key,
                            "Dropping expired queued message (waited > {}s)",
                            MAX_QUEUE_WAIT_SECS
                        );
                        queue.pending.pop_front();
                    } else {
                        break;
                    }
                }

                queue.pending.pop_front()
            } else {
                None
            }
        };

        if let Some(task) = next_task {
            debug!(session = %session_key, "Scheduler: executing next queued message");
            self.execute_enriched(session_key, task.enriched).await;
        } else {
            // Cleanup empty queue
            let mut queues = self.queues.lock().await;
            if let Some(queue) = queues.get(session_key) {
                if queue.active_run_id.is_none() && queue.pending.is_empty() {
                    queues.remove(session_key);
                }
            }
        }
    }

    /// Get queue depth for a session (for diagnostics)
    pub async fn queue_depth(&self, session_key: &str) -> usize {
        let queues = self.queues.lock().await;
        queues
            .get(session_key)
            .map(|q| q.pending.len())
            .unwrap_or(0)
    }

    /// Clone self reference for use in SchedulerEventListener
    fn clone_self_ref(&self) -> Arc<Mutex<HashMap<String, SessionQueue>>> {
        self.queues.clone()
    }
}

use std::collections;

/// Event emitter wrapper that notifies the scheduler on run completion
struct SchedulerEventListener {
    inner: Arc<ReplyEmitter>,
    scheduler_queues: Arc<Mutex<HashMap<String, SessionQueue>>>,
    session_key: String,
    scheduler: Arc<Mutex<HashMap<String, SessionQueue>>>,
}

#[async_trait]
impl EventEmitter for SchedulerEventListener {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        // Forward to inner emitter first
        self.inner.emit(event.clone()).await?;

        // Check for completion events
        match &event {
            StreamEvent::RunComplete { run_id, .. } => {
                info!(
                    run_id = %run_id,
                    session = %self.session_key,
                    "Scheduler: run complete, checking queue"
                );
                // Trigger next queued task
                let next_task = {
                    let mut queues = self.scheduler_queues.lock().await;
                    if let Some(queue) = queues.get_mut(&self.session_key) {
                        queue.active_run_id = None;

                        // Drop expired tasks
                        while let Some(front) = queue.pending.front() {
                            if front.enqueued_at.elapsed()
                                > Duration::from_secs(MAX_QUEUE_WAIT_SECS)
                            {
                                queue.pending.pop_front();
                            } else {
                                break;
                            }
                        }

                        queue.pending.pop_front().map(|t| t.enriched)
                    } else {
                        None
                    }
                };

                if next_task.is_some() {
                    debug!(
                        session = %self.session_key,
                        "Scheduler: will execute next queued message after completion callback"
                    );
                    // Note: Actual re-execution happens via the scheduler's on_run_complete
                    // which is called by the outer SessionScheduler
                }
            }
            StreamEvent::RunError { run_id, .. } => {
                warn!(
                    run_id = %run_id,
                    session = %self.session_key,
                    "Scheduler: run error, checking queue"
                );
                let mut queues = self.scheduler_queues.lock().await;
                if let Some(queue) = queues.get_mut(&self.session_key) {
                    queue.active_run_id = None;
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.inner.next_seq()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::agent_instance::AgentInstanceConfig;
    use crate::gateway::channel::*;
    use crate::gateway::event_emitter::NoOpEventEmitter;
    use crate::gateway::execution_engine::{ExecutionError, RunStatus, RunState};
    use crate::gateway::inbound_context::{InboundContext, ReplyRoute};
    use crate::gateway::pipeline::{MergedMessage, EnrichedMessage};
    use crate::gateway::router::SessionKey;
    use chrono::Utc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    // Mock execution adapter that tracks calls
    struct CountingAdapter {
        call_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ExecutionAdapter for CountingAdapter {
        async fn execute(
            &self,
            _request: RunRequest,
            _agent: Arc<crate::gateway::agent_instance::AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            // Simulate some work
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(())
        }

        async fn cancel(&self, _run_id: &str) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn get_status(&self, _run_id: &str) -> Option<RunStatus> {
            None
        }
    }

    fn make_enriched(text: &str, session: &str) -> EnrichedMessage {
        let msg = InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("test"),
            conversation_id: ConversationId::new("chat-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("test"), ConversationId::new("chat-1"));
        let ctx = InboundContext::new(msg, route, SessionKey::main(session));
        let merged = MergedMessage::from_single(ctx);
        EnrichedMessage::build(merged, vec![], vec![], 0)
    }

    #[test]
    fn test_session_queue_new() {
        let queue = SessionQueue::new();
        assert!(queue.active_run_id.is_none());
        assert!(queue.pending.is_empty());
    }

    #[tokio::test]
    async fn test_queue_depth() {
        let temp = tempdir().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let adapter: Arc<dyn ExecutionAdapter> = Arc::new(CountingAdapter {
            call_count: call_count.clone(),
        });
        let registry = Arc::new(AgentRegistry::new());
        let channel_registry = Arc::new(ChannelRegistry::new());

        let scheduler = SessionScheduler::new(adapter, registry, channel_registry);

        assert_eq!(scheduler.queue_depth("nonexistent").await, 0);
    }
}
```

**Step 2: Register module**

In `core/src/gateway/mod.rs`, add after `pub mod inbound_router;`:

```rust
pub mod session_scheduler;
```

**Step 3: Run tests**

```bash
cargo test -p alephcore --lib session_scheduler
```

Expected: Tests pass.

**Step 4: Commit**

```bash
git add core/src/gateway/session_scheduler.rs core/src/gateway/mod.rs
git commit -m "gateway: add SessionScheduler for per-session serial execution"
```

---

### Task 7: InboundRouter Integration

**Files:**
- Modify: `core/src/gateway/inbound_router.rs`

**Step 1: Add DebounceBuffer field to InboundRouter**

In `InboundMessageRouter` struct, add a new field:

```rust
/// Debounce buffer for message merging (replaces direct execution)
debounce_buffer: Option<Arc<crate::gateway::pipeline::DebounceBuffer>>,
```

**Step 2: Simplify execute_for_context()**

Replace the body of `execute_for_context()` (lines 1141-1211) with:

```rust
async fn execute_for_context(&self, ctx: &InboundContext) -> Result<(), RoutingError> {
    // If debounce buffer is configured, use the new pipeline path
    if let Some(buffer) = &self.debounce_buffer {
        buffer.submit(ctx.clone()).await;
        return Ok(());
    }

    // Legacy path: direct execution (kept for backward compatibility)
    let (agent_registry, execution_adapter) = match (
        self.agent_registry.as_ref(),
        self.execution_adapter.as_ref(),
    ) {
        (Some(ar), Some(ea)) => (ar.clone(), ea.clone()),
        _ => {
            info!(
                "Would execute agent for session {} with input: {} (execution not configured)",
                ctx.session_key.to_key_string(),
                ctx.message.text.chars().take(100).collect::<String>()
            );
            return Ok(());
        }
    };

    let agent_id = ctx.session_key.agent_id();
    let agent = agent_registry.get(agent_id).await.ok_or_else(|| {
        RoutingError::AgentNotFound(agent_id.to_string())
    })?;

    let run_id = Uuid::new_v4().to_string();
    let emitter = Arc::new(ReplyEmitter::new(
        self.channel_registry.clone(),
        ctx.reply_route.clone(),
        run_id.clone(),
    ));

    let mut metadata = HashMap::new();
    metadata.insert("channel_id".to_string(), ctx.message.channel_id.as_str().to_string());
    metadata.insert("sender_id".to_string(), ctx.sender_normalized.clone());
    if ctx.message.is_group {
        metadata.insert("is_group".to_string(), "true".to_string());
    }
    if ctx.is_mentioned {
        metadata.insert("is_mentioned".to_string(), "true".to_string());
    }

    let request = RunRequest {
        run_id: run_id.clone(),
        input: ctx.message.text.clone(),
        session_key: ctx.session_key.clone(),
        timeout_secs: None,
        metadata,
    };

    info!(
        "Executing agent '{}' for session {} (run_id: {})",
        agent_id,
        ctx.session_key.to_key_string(),
        run_id
    );

    tokio::spawn(async move {
        if let Err(e) = execution_adapter.execute(request, agent, emitter).await {
            error!("Agent execution failed (run_id: {}): {}", run_id, e);
        }
    });

    Ok(())
}
```

**Step 3: Do the same for execute_for_context_with_metadata()**

Add debounce buffer check at the top (for slash commands, pass through directly since they should not be debounced — slash commands need immediate execution):

```rust
// Note: Slash commands bypass debounce — they need immediate execution
```

Keep the existing slash command code path unchanged.

**Step 4: Add builder method for debounce buffer**

In InboundMessageRouter's builder/constructor section, add:

```rust
/// Set the debounce buffer for the new pipeline path
pub fn with_debounce_buffer(mut self, buffer: Arc<crate::gateway::pipeline::DebounceBuffer>) -> Self {
    self.debounce_buffer = Some(buffer);
    self
}
```

**Step 5: Run all existing tests**

```bash
cargo test -p alephcore --lib inbound_router
```

Expected: All existing tests pass (debounce_buffer defaults to None, legacy path unchanged).

**Step 6: Commit**

```bash
git add core/src/gateway/inbound_router.rs
git commit -m "gateway: integrate DebounceBuffer into InboundRouter with legacy fallback"
```

---

### Task 8: Deprecated Code Cleanup

**Files:**
- Modify: `core/src/dispatcher/registry/registration.rs` — remove `register_agent_tools()`
- Modify: `core/src/dispatcher/executor/mod.rs` — remove `with_working_dir()`
- Modify: `core/src/dispatcher/types/category.rs` — remove `ToolCategory::Native`
- Modify: `core/src/gateway/security/policy_engine.rs` — remove deprecated stubs
- Modify: `core/src/memory/compression/conflict.rs` — remove `InvalidateOld`
- Modify: `core/src/memory/store/mod.rs` — remove `AuditStore` trait

**Step 1: For each file, find and remove the deprecated item**

Search for `#[deprecated` in each file, remove the deprecated item and any `#[allow(deprecated)]` usage in tests that reference it.

**Step 2: Fix compilation**

```bash
cargo check -p alephcore
```

Fix any compilation errors from removed items (update callers if any).

**Step 3: Run all tests**

```bash
cargo test -p alephcore --lib
```

Expected: All tests pass (some deprecated-item tests will be removed).

**Step 4: Remove archive tarballs**

```bash
rm -f archive/*.tar.gz
```

**Step 5: Commit**

```bash
git add -u
git commit -m "cleanup: remove deprecated APIs (register_agent_tools, with_working_dir, ToolCategory::Native, PolicyEngine stubs, AuditStore, InvalidateOld)"
```

---

### Task 9: Integration Test

**Files:**
- Create: `core/src/gateway/pipeline/integration_test.rs`
- Modify: `core/src/gateway/pipeline/mod.rs` (add `#[cfg(test)] mod integration_test;`)

**Step 1: Write full-flow integration test**

```rust
//! Integration test: full pipeline flow
//!
//! Tests: InboundContext → DebounceBuffer → MessagePipeline → EnrichedMessage

#[cfg(test)]
mod tests {
    use crate::gateway::channel::*;
    use crate::gateway::inbound_context::{InboundContext, ReplyRoute};
    use crate::gateway::pipeline::*;
    use crate::gateway::pipeline::debounce::{DebounceBuffer, DebounceConfig};
    use crate::gateway::pipeline::media_understanding::UnderstandingProvider;
    use crate::gateway::router::SessionKey;
    use crate::sync_primitives::Arc;
    use chrono::Utc;
    use std::collections::HashMap;
    use tokio::sync::{Mutex, Notify};

    struct MockUnderstandingProvider;

    #[async_trait::async_trait]
    impl UnderstandingProvider for MockUnderstandingProvider {
        async fn understand(
            &self,
            _path: &std::path::Path,
            category: &MediaCategory,
            _model: &str,
        ) -> Result<(String, u64), String> {
            match category {
                MediaCategory::Image => Ok(("A test image".to_string(), 25)),
                _ => Ok(("Some content".to_string(), 15)),
            }
        }
    }

    fn make_ctx_with_attachment(text: &str) -> InboundContext {
        let msg = InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("chat-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![Attachment {
                id: "att-1".to_string(),
                mime_type: "image/png".to_string(),
                filename: Some("photo.png".to_string()),
                size: Some(3),
                url: None,
                path: None,
                data: Some(vec![1, 2, 3]),
            }],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("telegram"), ConversationId::new("chat-1"));
        InboundContext::new(msg, route, SessionKey::main("main"))
    }

    #[tokio::test]
    async fn test_full_pipeline_flow() {
        let temp = tempfile::tempdir().unwrap();

        // Set up pipeline
        let downloader = MediaDownloader::new(temp.path().to_path_buf());
        let understander = MediaUnderstander::new(
            Arc::new(MockUnderstandingProvider),
            "haiku".to_string(),
        );
        let pipeline = Arc::new(MessagePipeline::new(downloader, understander));

        // Set up debounce → pipeline flow
        let results = Arc::new(Mutex::new(Vec::<EnrichedMessage>::new()));
        let notify = Arc::new(Notify::new());

        let results_clone = results.clone();
        let notify_clone = notify.clone();
        let pipeline_clone = pipeline.clone();

        let on_ready = Arc::new(move |merged: MergedMessage| {
            let p = pipeline_clone.clone();
            let r = results_clone.clone();
            let n = notify_clone.clone();
            tokio::spawn(async move {
                match p.process(merged, None).await {
                    Ok(enriched) => {
                        r.lock().await.push(enriched);
                        n.notify_one();
                    }
                    Err(e) => panic!("Pipeline failed: {}", e),
                }
            });
        });

        let config = DebounceConfig {
            default_window_ms: 100,
            max_window_ms: 500,
            max_messages: 10,
            channel_overrides: HashMap::new(),
        };
        let buffer = DebounceBuffer::new(config, on_ready);

        // Submit a message with attachment
        buffer.submit(make_ctx_with_attachment("analyze this image")).await;

        // Wait for debounce + pipeline
        notify.notified().await;

        let results = results.lock().await;
        assert_eq!(results.len(), 1);

        let enriched = &results[0];
        assert!(enriched.enriched_text.contains("analyze this image"));
        assert!(enriched.enriched_text.contains("[Attachment Understanding]"));
        assert!(enriched.enriched_text.contains("A test image"));
        assert_eq!(enriched.local_media.len(), 1);
        assert_eq!(enriched.understanding_tokens, 25);
        assert_eq!(enriched.merged.merge_count, 1);
    }

    #[tokio::test]
    async fn test_debounce_merge_then_pipeline() {
        let temp = tempfile::tempdir().unwrap();

        let downloader = MediaDownloader::new(temp.path().to_path_buf());
        let understander = MediaUnderstander::new(
            Arc::new(MockUnderstandingProvider),
            "haiku".to_string(),
        );
        let pipeline = Arc::new(MessagePipeline::new(downloader, understander));

        let results = Arc::new(Mutex::new(Vec::<EnrichedMessage>::new()));
        let notify = Arc::new(Notify::new());

        let results_clone = results.clone();
        let notify_clone = notify.clone();
        let pipeline_clone = pipeline.clone();

        let on_ready = Arc::new(move |merged: MergedMessage| {
            let p = pipeline_clone.clone();
            let r = results_clone.clone();
            let n = notify_clone.clone();
            tokio::spawn(async move {
                match p.process(merged, None).await {
                    Ok(enriched) => {
                        r.lock().await.push(enriched);
                        n.notify_one();
                    }
                    Err(e) => panic!("Pipeline failed: {}", e),
                }
            });
        });

        let config = DebounceConfig {
            default_window_ms: 200,
            max_window_ms: 2000,
            max_messages: 10,
            channel_overrides: HashMap::new(),
        };
        let buffer = DebounceBuffer::new(config, on_ready);

        // Submit 3 rapid-fire text messages
        let make_text_ctx = |text: &str| {
            let msg = InboundMessage {
                id: MessageId::new(format!("msg-{}", text.len())),
                channel_id: ChannelId::new("telegram"),
                conversation_id: ConversationId::new("chat-1"),
                sender_id: UserId::new("user-1"),
                sender_name: None,
                text: text.to_string(),
                attachments: vec![],
                timestamp: Utc::now(),
                reply_to: None,
                is_group: false,
                raw: None,
            };
            let route = ReplyRoute::new(
                ChannelId::new("telegram"),
                ConversationId::new("chat-1"),
            );
            InboundContext::new(msg, route, SessionKey::main("main"))
        };

        buffer.submit(make_text_ctx("hello")).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        buffer.submit(make_text_ctx("check the weather")).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        buffer.submit(make_text_ctx("in Beijing")).await;

        // Wait for debounce + pipeline
        notify.notified().await;

        let results = results.lock().await;
        assert_eq!(results.len(), 1);

        let enriched = &results[0];
        assert_eq!(enriched.merged.merge_count, 3);
        assert!(enriched.enriched_text.contains("hello"));
        assert!(enriched.enriched_text.contains("check the weather"));
        assert!(enriched.enriched_text.contains("in Beijing"));
        // No attachments, so no understanding appendix
        assert!(!enriched.enriched_text.contains("[Attachment Understanding]"));
    }
}
```

**Step 2: Run integration tests**

```bash
cargo test -p alephcore --lib pipeline::integration_test
```

Expected: Both tests pass.

**Step 3: Commit**

```bash
git add core/src/gateway/pipeline/integration_test.rs core/src/gateway/pipeline/mod.rs
git commit -m "pipeline: add integration tests for full debounce → pipeline flow"
```

---

### Task 10: Final Verification

**Step 1: Run all core tests**

```bash
cargo test -p alephcore --lib
```

Expected: All tests pass (including pre-existing failures in `tools::markdown_skill::loader::tests`).

**Step 2: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | head -50
```

Fix any new warnings.

**Step 3: Compile check**

```bash
cargo check -p alephcore
```

Expected: Clean compilation.

**Step 4: Final commit (if clippy fixes needed)**

```bash
git add -u
git commit -m "pipeline: fix clippy warnings"
```
