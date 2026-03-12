# Message Pipeline Design — 消息管线与会话调度

**Date:** 2026-03-13
**Status:** Approved
**Inspired by:** OpenClaw source code architecture analysis

## Background

OpenClaw 的核心价值在于构建了一套完整的"运行时装配体系"——在 Agent 循环前完成消息去重、防抖合并、媒体预理解、上下文装配、会话调度等所有预处理工作。Aleph 当前 InboundRouter 直接 `tokio::spawn` 调用 ExecutionAdapter，缺少这一关键层。

本设计借鉴 OpenClaw 的网关产品思维，在 Aleph 现有架构上增加 MessagePipeline 和 SessionScheduler，补齐消息入口管线能力。

## Design Decisions

| 决策 | 选择 | 理由 |
|------|------|------|
| 架构模式 | Pipeline Pattern（非中间件链） | 消息预处理是线性流水线，不需要动态拦截 |
| 防抖策略 | 滑动窗口 + 上限截断 | 覆盖 IM 连续发消息场景 |
| 媒体预理解 | 深度预理解（LLM 调用） | Agent 拿到"已理解"的输入 |
| 预理解模型 | 可配置，默认轻量模型 | 平衡成本和效果 |
| 会话并发 | 同会话严格串行 + 防抖前置 | 最简单可预测，防抖已覆盖 80% 快发场景 |
| 与现有代码关系 | 纯增量插入 | ExecutionEngine/Adapter/ReplyEmitter 不变 |

## Architecture

### New Message Flow

```
InboundRouter.handle_message()
    → DebounceBuffer.submit(InboundContext)          // NEW: debounce
        ⟶ (window expires)
    → MessagePipeline.process(MergedMessage)          // NEW: pipeline
        ├─ MediaDownloadStage
        ├─ MediaUnderstandingStage
        └─ ContextEnrichmentStage
    → SessionScheduler.enqueue(EnrichedMessage)       // NEW: session queue
        ⟶ (wait for previous run to complete)
    → ExecutionAdapter.execute(RunRequest, ...)        // EXISTING: unchanged
```

### New Module Layout

```
core/src/gateway/
    ├── pipeline/
    │   ├── mod.rs                  // MessagePipeline
    │   ├── debounce.rs             // DebounceBuffer
    │   ├── media_download.rs       // MediaDownloader
    │   ├── media_understanding.rs  // MediaUnderstander
    │   └── enrichment.rs           // EnrichedMessage
    ├── session_scheduler.rs        // SessionScheduler
    └── ... (existing files unchanged)
```

## Component Design

### 1. DebounceBuffer

Collects rapid-fire messages per session, merges them after a configurable window.

```rust
pub struct DebounceBuffer {
    pending: Arc<Mutex<HashMap<String, DebounceBatch>>>,
    config: DebounceConfig,
    on_ready: Arc<dyn Fn(MergedMessage) + Send + Sync>,
}

pub struct DebounceConfig {
    pub default_window_ms: u64,                    // Default: 2000
    pub max_window_ms: u64,                        // Default: 5000
    pub max_messages: usize,                       // Default: 10
    pub channel_overrides: HashMap<String, u64>,   // e.g., webchat: 500
}

pub struct MergedMessage {
    pub text: String,                  // Messages joined by newline
    pub attachments: Vec<Attachment>,  // All attachments aggregated
    pub primary_context: InboundContext,
    pub merged_message_ids: Vec<MessageId>,
    pub merge_count: usize,
}
```

**Trigger rules (first one wins):**
1. Window expires (`default_window_ms` since first message)
2. Max messages reached (`max_messages`)
3. Hard deadline (`max_window_ms`, prevents infinite wait)

**Sliding window:** Each new message resets the timer (but cannot exceed `max_window_ms`).

**Single message optimization:** When window expires with only 1 message, `MergedMessage` wraps it directly with `merge_count = 1`, zero-overhead passthrough.

### 2. MessagePipeline

Linear processing stages: download → understand → enrich.

```rust
pub struct MessagePipeline {
    media_downloader: MediaDownloader,
    media_understander: MediaUnderstander,
    default_understanding_model: String,  // Default: "haiku"
}

impl MessagePipeline {
    pub async fn process(&self, merged: MergedMessage) -> Result<EnrichedMessage, PipelineError> {
        let local_media = self.media_downloader.download_all(&merged).await?;
        let understandings = self.media_understander.understand_all(&local_media, &merged).await?;
        Ok(EnrichedMessage::from(merged, local_media, understandings))
    }
}
```

#### Stage 1: MediaDownloader

```rust
pub struct MediaDownloader {
    workspace_root: PathBuf,
    http_client: reqwest::Client,
    max_file_size: u64,               // Default: 50MB
    supported_types: HashSet<String>,
}

pub struct LocalMedia {
    pub original: Attachment,
    pub local_path: PathBuf,           // {workspace}/media/{run_id}/{filename}
    pub media_category: MediaCategory,
}

pub enum MediaCategory {
    Image, Document, Link, Audio, Video, Unknown,
}
```

**Behavior:**
- `url` attachments: download to local workspace
- `data` attachments: write inline bytes to local file
- `path` attachments: skip (already local)
- URLs extracted from message text: download HTML, convert to text
- Download failures: log warning, skip (non-blocking)

#### Stage 2: MediaUnderstander

```rust
pub struct MediaUnderstander {
    provider_registry: Arc<dyn ThinkerProviderRegistry>,
}

pub struct MediaUnderstanding {
    pub media: LocalMedia,
    pub description: String,
    pub understanding_type: UnderstandingType,
}

pub enum UnderstandingType {
    ImageDescription,
    LinkSummary,
    DocumentSummary,
    Skipped(String),
}
```

**Model selection priority:**
1. Agent config `understanding_model` field
2. Pipeline `default_understanding_model` (e.g., "haiku")
3. Fallback to session's primary model

**Prompts (fixed, not via PromptPipeline):**
- Image: "Describe this image concisely in the user's language. Focus on key content, text, and actionable details."
- Link: "Summarize this webpage content in 2-3 sentences. Focus on the main topic and key information."
- Document: "Summarize this document concisely. List key sections and main points."

**Concurrency:** Multiple attachments processed in parallel (`futures::join_all`). Individual failures don't block others.

#### Stage 3: EnrichedMessage

```rust
pub struct EnrichedMessage {
    pub merged: MergedMessage,
    pub enriched_text: String,       // Original text + understanding appendix
    pub local_media: Vec<LocalMedia>,
    pub understanding_tokens: u64,   // Cost tracking
}
```

**enriched_text format:**
```
{original user message}

[Attachment Understanding]
- Image "photo.jpg": Shows an office scene with...
- Link "https://example.com": Article discusses AI trends in 2026...
```

This replaces `RunRequest.input`, so the Agent loop receives fully understood input.

### 3. SessionScheduler

Per-session strict serial execution with queue management.

```rust
pub struct SessionScheduler {
    queues: Arc<Mutex<HashMap<String, SessionQueue>>>,
    execution_adapter: Arc<dyn ExecutionAdapter>,
    agent_registry: Arc<AgentRegistry>,
    channel_registry: Arc<ChannelRegistry>,
}

struct SessionQueue {
    pending: VecDeque<QueuedTask>,
    active_run_id: Option<String>,
}
```

**Completion notification:** `SchedulerEventListener` wraps `ReplyEmitter`, intercepts `RunComplete`/`RunError` events to trigger next queued task.

```rust
pub struct SchedulerEventListener {
    inner: Arc<ReplyEmitter>,
    scheduler: Arc<SessionScheduler>,
    session_key: String,
}
```

**Queue timeout:** Tasks waiting > 300s are dropped with user notification.

**Idle cleanup:** Empty session entries periodically purged.

## Existing Code Changes

### Modified Files

**`inbound_router.rs`** — Slim down:
- `execute_for_context()`: ~70 lines → ~5 lines (just `debounce_buffer.submit()`)
- Remove `execution_adapter` and `agent_registry` fields (moved to SessionScheduler)

**`gateway/mod.rs`** — Assembly:
- Initialize Pipeline, Scheduler, DebounceBuffer
- Wire them together with callbacks

### Unchanged Files

- `execution_engine/engine.rs` — No changes
- `execution_adapter.rs` — No changes
- `reply_emitter.rs` — No changes
- `agent_instance.rs` — No changes
- `channel.rs` — No changes

## Deprecated Code Cleanup

Performed alongside this refactoring:

| File | Item | Action |
|------|------|--------|
| `dispatcher/registry/registration.rs` | `register_agent_tools()` | Remove |
| `dispatcher/executor/mod.rs` | `with_working_dir()` | Remove |
| `dispatcher/types/category.rs` | `ToolCategory::Native` | Remove |
| `gateway/security/policy_engine.rs` | `new()`, `set_guest_scope()`, `remove_guest_scope()` | Remove |
| `memory/compression/conflict.rs` | `ConflictResolution::InvalidateOld` | Remove |
| `memory/store/mod.rs` | `AuditStore` trait | Remove |
| `archive/*.tar.gz` | Redundant compressed backups | Remove |

## Testing Strategy

- **Unit tests:** Each pipeline stage independently testable with mock providers
- **Integration tests:** Full flow from InboundMessage → EnrichedMessage → RunRequest
- **Debounce tests:** Timing-based tests for merge behavior (single message, rapid-fire, max window)
- **Scheduler tests:** Serial execution ordering, completion callbacks, timeout handling
- **Property tests:** MergedMessage text never empty, attachment count matches, enriched_text contains original

## Non-Goals

- Changing the Agent Loop (Think → Act cycle) — out of scope
- Modifying provider/model selection logic — existing failover system is sufficient
- Adding new channel types — pipeline is channel-agnostic
- Conversation history sliding window — existing truncation strategy is adequate
