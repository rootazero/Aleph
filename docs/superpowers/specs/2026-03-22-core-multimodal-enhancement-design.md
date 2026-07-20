# Core Multimodal Enhancement Design

**Date**: 2026-03-22
**Status**: Approved
**Scope**: Fix the attachment pipeline and enable native multimodal LLM interaction

## Overview

Aleph's multimodal infrastructure has a critical pipeline break: attachments received by channels are captured in `InboundMessage.attachments` but lost when messages enter the agent loop. `ContentBlock::Image` exists in `UnifiedMessage` but is never populated. The Anthropic adapter supports images, but OpenAI silently drops them. This design fixes the pipeline and adds audio transcription.

Reference: OpenClaw analyzed for feature comparison. Adapted to Aleph's architecture (R3 Core Minimalism, R4 I/O-Only Interfaces, R8 LLM Sovereignty).

## Key Decisions

- **Images**: Native `ContentBlock::Image` injection when Provider supports vision; auto-fallback to description via VisionPipeline when not
- **Audio**: Provider API transcription (OpenAI Whisper), no local binary management (R3)
- **Caching**: Session-level temp directory, session only stores text summaries
- **No video processing**: YAGNI, can be added later

## Section 1: Fix Pipeline Break — Attachment → UnifiedMessage

### 4 Break Points and Fixes

**Break 1: `RunRequest` lacks attachments** (`execution_engine/mod.rs`)

Add `attachments: Vec<Attachment>` field to `RunRequest`. In `executor.rs:132`, populate from `ctx.message.attachments.clone()`.

**Break 2: `add_message()` text-only** (`engine.rs:248`)

Session stores text summaries only. When attachments are present, build a summary string and append to the text before storing:

```rust
// In engine.rs, before add_message():
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
agent.add_message(&request.session_key, MessageRole::User, &session_text).await;
```

After transcription completes (in run_loop), the audio marker in session can be updated to include the transcript text. Actual media data goes through MediaCache, not session storage.

**Break 3: Multimodal message construction** (`run_loop.rs`)

The `MediaProcessor.process()` call lives in **`run_loop.rs`**, NOT in `loop_core.rs`. This is because `run_loop.rs` has access to `RunRequest.attachments` and orchestrates the call to `AgentLoop`. The multimodal `UnifiedMessage` for the current input is built in `run_loop.rs` and prepended to the history before passing to `agent_loop.run_with_history()`:

```rust
// In run_loop.rs, before calling agent_loop.run_with_history():
let current_user_msg = if !request.attachments.is_empty() {
    let media_blocks = media_processor.process(
        &request.attachments, supports_vision, &session_key.to_string()
    ).await;
    let mut content = vec![ContentBlock::Text { text: request.input.clone() }];
    content.extend(media_blocks);
    UnifiedMessage::User { content }
} else {
    UnifiedMessage::user(&request.input)
};

// Append to history, then pass to agent loop
history.push(current_user_msg);
let result = agent_loop.run_with_history_messages(history, &mut callback).await?;
```

This requires adding a new method `run_with_history_messages(messages, callback)` to `AgentLoop` that accepts pre-built messages (without appending its own `UnifiedMessage::user(input)`). The existing `run_with_history(input, history, callback)` remains for non-multimodal callers.

Only the **current message** gets media blocks. History messages retain text summaries only (no historical image re-injection — saves tokens).

**Break 4: OpenAI adapter drops images** (`openai.rs:112`)

Fix `convert_messages()` to handle `ContentBlock::Image` — see Section 6.

### Files Changed

| File | Change |
|------|--------|
| `gateway/execution_engine/mod.rs` | Add `attachments` field to `RunRequest` |
| `gateway/inbound_router/executor.rs` | Pass `ctx.message.attachments` to RunRequest |
| `gateway/execution_engine/engine.rs` | Store text summary in session, pass attachments to run_loop |
| `gateway/execution_engine/run_loop.rs` | Call MediaProcessor, build multimodal UnifiedMessage, pass to AgentLoop |
| `agent_loop/loop_core.rs` | Add `run_with_history_messages()` method (accepts pre-built messages) |

## Section 2: MediaCache — Download & Temp Storage

**New module**: `src/media/cache.rs`

### Interface

```rust
pub struct MediaCache {
    cache_dir: PathBuf,  // std::env::temp_dir().join("aleph/media/")
}

impl MediaCache {
    pub fn new() -> Self;
    pub async fn resolve(&self, attachment: &Attachment, session_id: &str) -> Result<CachedMedia>;
    pub fn to_base64(&self, cached: &CachedMedia) -> Result<String>;
    pub fn cleanup_session(&self, session_id: &str);
    pub fn cleanup_stale(&self);  // Called on startup
}

pub struct CachedMedia {
    pub local_path: PathBuf,
    pub mime_type: String,
    pub size: u64,
}
```

### Resolution Priority

1. `attachment.data: Some(bytes)` → write to temp file
2. `attachment.path: Some(path)` → use directly (no copy)
3. `attachment.url: Some(url)` → HTTP GET download

### Constraints

- Max file size: 20MB (configurable), skip if exceeded
- Cache dir: `std::env::temp_dir()/aleph/media/{session_id}/` (platform-safe, not hardcoded `/tmp`)
- Cleanup: on RunComplete/RunError + startup stale cleanup
- Timeout: 30s for HTTP downloads

### Files

| File | Action |
|------|--------|
| `src/media/mod.rs` | New module declaration |
| `src/media/cache.rs` | MediaCache implementation |

## Section 3: Provider Vision Capability Detection

### Existing Infrastructure

`ProviderModelInfo.supports_vision: bool` already exists (`extension/types/runtime.rs:237`) but is never consulted during message building.

### Wire It Up

1. When agent loop starts a run, resolve the current model's `supports_vision` flag
2. Pass `supports_vision: bool` to `MediaProcessor.process()`
3. MediaProcessor decides per-image: native injection vs. description fallback

### Fallback Path

When `supports_vision == false`:
1. `MediaCache.resolve()` → get local file
2. `VisionPipeline.understand_image(&ImageInput::from_base64(b64, mime), "Describe this image concisely")` → `VisionResult`
3. Return `ContentBlock::Text { text: format!("[Image: {}]", result.description) }`

VisionPipeline already exists at `src/vision/mod.rs` with `understand_image(&ImageInput, &str) -> Result<VisionResult>` API. Uses `ImageInput` wrapper (not raw path).

### Capability Propagation

The `supports_vision` flag needs to reach `run_loop.rs`. Minimal path:
- `AgentInstance` already knows its provider → query model info → extract `supports_vision`
- Pass as parameter to the message-building function

### Files

| File | Change |
|------|--------|
| `gateway/execution_engine/run_loop.rs` | Query supports_vision, pass to MediaProcessor |
| No new files needed — uses existing VisionPipeline |

## Section 4: Audio Transcription

### TranscriptionService Trait

**New file**: `src/media/transcription.rs`

```rust
#[async_trait]
pub trait TranscriptionService: Send + Sync {
    async fn transcribe(&self, audio: &CachedMedia) -> Result<TranscriptionResult>;
}

pub struct TranscriptionResult {
    pub text: String,
    pub language: Option<String>,
}
```

### WhisperTranscription Implementation

**New file**: `src/media/whisper.rs`

- Calls OpenAI-compatible `POST /v1/audio/transcriptions` (multipart form-data)
- Reuses existing Provider API key and base_url from config
- Default model: `whisper-1`
- Supported input formats: mp3, mp4, mpeg, mpga, m4a, wav, webm, ogg

### Configuration

No new config fields. Auto-detect from existing Provider:
1. If active Provider is OpenAI-compatible → use its API key/base_url for Whisper
2. If not → skip transcription, inject `[Audio: {mime_type}]` placeholder

### Injection Format

```
[Voice message transcript]:
"The transcribed text goes here..."
```

### Files

| File | Action |
|------|--------|
| `src/media/transcription.rs` | TranscriptionService trait |
| `src/media/whisper.rs` | WhisperTranscription impl |

## Section 5: MediaProcessor — Unified Entry Point

**New file**: `src/media/processor.rs`

### Interface

```rust
pub struct MediaProcessor {
    cache: MediaCache,
    transcription: Option<Box<dyn TranscriptionService>>,
    vision: Option<Arc<VisionPipeline>>,
}

impl MediaProcessor {
    pub async fn process(
        &self,
        attachments: &[Attachment],
        supports_vision: bool,
        session_id: &str,
    ) -> Vec<ContentBlock>;

    pub fn cleanup(&self, session_id: &str);
}
```

### Processing Logic Per Attachment

| MIME prefix | supports_vision=true | supports_vision=false |
|-------------|---------------------|----------------------|
| `image/*` | base64 → `ContentBlock::Image` | VisionPipeline → `ContentBlock::Text("[Image: desc]")` |
| `audio/*` | Transcription → `ContentBlock::Text("[Transcript]: ...")` | Same |
| Other | `ContentBlock::Text("[Attachment: name (type)]")` | Same |

### Error Isolation

Each attachment processed independently. Single failure → fallback text, not abort:
- Download failed → `[Attachment: download failed]`
- Transcription failed → `[Audio: transcription unavailable]`
- Vision failed → `[Image: description unavailable]`

### Lifecycle

- Created at server startup, owned by `ExecutionEngine` (same pattern as `session_compactor`)
- Accessed in `run_loop.rs` during message construction (NOT inside AgentLoop)
- `process()` called per message with attachments
- `cleanup()` called on RunComplete/RunError

### Files

| File | Action |
|------|--------|
| `src/media/processor.rs` | MediaProcessor implementation |
| `src/media/mod.rs` | Module declarations |

## Section 6: OpenAI Protocol Adapter Fix

### Current Bug

`openai.rs` line 112: `.filter_map(|b| b.as_text())` silently drops all `ContentBlock::Image`.

### Fix

When content contains Image blocks, use OpenAI multimodal format:

```json
{
  "role": "user",
  "content": [
    {"type": "text", "text": "What's in this image?"},
    {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,..."}}
  ]
}
```

When content is text-only, keep existing simple format (backward compatible):
```json
{"role": "user", "content": "Hello"}
```

### MessageContent — Already Exists

`providers/openai/types.rs` (NOT `providers/protocols/openai/types.rs`) already has:
```rust
pub enum MessageContent {
    Text { content: String },
    Multimodal { content: Vec<ContentBlock> },  // Already defined!
}
```

The type layer is ready. Only the `convert_messages()` function in `providers/protocols/openai.rs` needs fixing to actually USE the Multimodal variant when Image blocks are present.

### Anthropic Adapter

Already supports images via `ImageSource { source_type: "base64", media_type, data }`. No changes needed. Confirm `ContentBlock::Image.data` is pure base64 (no data-URI prefix) — consistent with current convention.

### Dependency Note

Verify `reqwest` has `multipart` feature enabled in `Cargo.toml` (needed for Whisper API upload in Section 4). Add if missing.

### Files

| File | Change |
|------|--------|
| `providers/protocols/openai.rs` | Handle ContentBlock::Image in convert_messages() using existing Multimodal variant from `providers/openai/types.rs` |

## Architecture Alignment

| Principle | How This Design Complies |
|-----------|--------------------------|
| R1 Brain-Limb Separation | Media processing in Core, no platform-specific code |
| R3 Core Minimalism | Whisper via API (no local binary), VisionPipeline reused |
| R4 I/O-Only Interfaces | Channels only capture attachments, Core processes them |
| R8 LLM Sovereignty | Native vision when supported, LLM sees raw images |
| P1 Low Coupling | MediaProcessor is a standalone service, injected via trait |
| P4 Dependency Inversion | TranscriptionService is a trait, implementations are pluggable |
| P6 Simplicity | Session stores summaries only, no complex caching strategy |
| P7 Defensive Design | Per-attachment error isolation, graceful fallbacks |

## New Files Summary

| File | Responsibility |
|------|---------------|
| `src/media/mod.rs` | Module declarations |
| `src/media/cache.rs` | MediaCache: download, temp storage, cleanup |
| `src/media/processor.rs` | MediaProcessor: unified attachment → ContentBlock |
| `src/media/transcription.rs` | TranscriptionService trait + TranscriptionResult |
| `src/media/whisper.rs` | WhisperTranscription: OpenAI Whisper API impl |

## Modified Files Summary

| File | Change |
|------|--------|
| `gateway/execution_engine/mod.rs` | Add `attachments` to RunRequest |
| `gateway/inbound_router/executor.rs` | Pass attachments to RunRequest |
| `gateway/execution_engine/engine.rs` | Pass attachments to run_loop |
| `gateway/execution_engine/run_loop.rs` | Call MediaProcessor, build multimodal UnifiedMessage |
| `agent_loop/loop_core.rs` | Add `run_with_history_messages()` method |
| `providers/protocols/openai.rs` | Handle ContentBlock::Image (types already exist in `providers/openai/types.rs`) |
| `providers/message.rs` | Add `UnifiedMessage::user_multimodal()` constructor |

## Not In Scope

- Video processing (frame extraction too complex, YAGNI)
- Document text extraction (PDF/CSV — future iteration)
- Multiple transcription providers (Deepgram, Groq — extensible via trait)
- Local whisper.cpp / sherpa-onnx (implement as MCP Server per R3)
- Cross-session media caching
- Echo transcript to chat (OpenClaw feature, not core need)
- Sticker description caching (Core sees stickers as images, LLM handles natively)
- Image sanitization / resize (trust Provider limits for now)
