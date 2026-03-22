# Telegram Channel Enhancement Design

**Date**: 2026-03-22
**Status**: Approved
**Scope**: Three-wave enhancement of Telegram channel capabilities

## Overview

Strengthen the Telegram channel with missing capabilities, prioritized by user-visible impact. All enhancements respect Aleph's architecture: Core handles multimodal processing (R1/R3), Channel is pure I/O (R4), LLM sovereignty (R8), everything is a tool (R9).

Reference: OpenClaw Telegram implementation analyzed for feature gap identification. Adapted to Aleph's architecture, not copied.

## Wave 1: Interaction Experience + Multimodal Passthrough

### 1.1 Processing Status Reactions

**Goal**: Visual feedback — user sees ⏳ immediately, then ✅ or ❌ on completion.

**Changes**:

1. **`Channel` trait `react()` signature** (`channel.rs:490`):
   - Add `conversation_id: &ConversationId` parameter: `react(conversation_id, message_id, reaction)`
   - All existing Channel implementations updated (default impl unchanged)

2. **`TelegramChannel`**:
   - Set `capabilities.reactions = true` (`mod.rs:112`)
   - Implement `react()` via `bot.set_message_reaction(chat_id, msg_id, [ReactionType::Emoji(emoji)])`

3. **Reaction lifecycle** (in `ReplyEmitter` or execution layer):
   - On first `ResponseChunk`: react ⏳ on the inbound message
   - On `RunComplete` with success: change to ✅
   - On `RunComplete` with error: change to ❌

**Files**: `channel.rs`, `telegram/mod.rs`, `reply_emitter.rs`

### 1.2 Multimodal Media Passthrough

**Goal**: Download media files and pass URLs to Core layer for multimodal processing.

**Changes**:

1. **`extract_attachments()` → async** (`mod.rs:203`):
   - Accept `&Bot` parameter
   - Call `bot.get_file(file_id)` to obtain `file_path`
   - Construct download URL: `https://api.telegram.org/file/bot<token>/<file_path>`
   - Set `Attachment.url` to this URL

2. **`convert_message()` → async** (`mod.rs:125`):
   - Accept `&Bot` parameter (needed by async `extract_attachments`)
   - Message handler endpoint updated accordingly

3. **Allow attachment-only messages** (`mod.rs:169-171`):
   - Change guard from `text.is_empty()` to `text.is_empty() && attachments.is_empty()`
   - Reorder: extract attachments before the empty check

4. **Sticker support** in `extract_attachments()`:
   - Add `MediaKind::Sticker` branch
   - Static (WEBP): `mime_type = "image/webp"`
   - Animated (TGS): `mime_type = "application/x-tgsticker"`
   - Video (WEBM): `mime_type = "video/webm"`
   - Set `text = "[Sticker: {emoji}]"` when no caption present

**Files**: `telegram/mod.rs`

### 1.3 Smart Text Chunking

**Goal**: Avoid breaking code blocks and HTML entities when splitting long messages.

**Changes** in `ReplyEmitter::find_split_point()` (`reply_emitter.rs:330`):

1. **Code block awareness**:
   - Count ` ``` ` occurrences before the candidate split point
   - If odd count (inside a code block), search backward for the last ` ``` ` boundary and split before it
   - If split point would be too early (< 25% of max_len), split after the code block instead

2. **HTML entity safety**:
   - Check if split point falls inside `&amp;`, `&lt;`, `&gt;`, `&quot;` patterns
   - If so, adjust backward to before the `&`

**Files**: `reply_emitter.rs` (only `find_split_point()` modified)

## Wave 2: Topic Isolation + Sticker + Streaming Polish

### 2.1 Forum Topic Session Isolation

**Goal**: Each Forum topic is an independent conversation session (same Agent).

**Changes**:

1. **`convert_message()`** — conversation_id encoding:
   ```rust
   let conversation_id = if let Some(thread_id) = msg.thread_id {
       format!("{}:topic:{}", msg.chat.id.0, thread_id)
   } else {
       msg.chat.id.0.to_string()
   };
   ```

2. **Helper function**:
   ```rust
   fn parse_conversation_id(conv_id: &str) -> (i64, Option<i32>) {
       if let Some((chat, topic)) = conv_id.split_once(":topic:") {
           (chat.parse().unwrap_or(0), topic.parse().ok())
       } else {
           (conv_id.parse().unwrap_or(0), None)
       }
   }
   ```

3. **`send()`** — set `message_thread_id` when topic is present:
   - Parse conversation_id to extract `(chat_id, thread_id)`
   - If `thread_id.is_some() && thread_id != Some(1)`: set `.message_thread_id(thread_id)`
   - General topic (`thread_id == 1`): do NOT set `message_thread_id`

4. **`send_typing()`** — also needs topic-aware chat action
5. **`edit()`** — no change needed (edit uses chat_id + message_id, no thread needed)

**Files**: `telegram/mod.rs`

### 2.2 Sticker Support (Detail)

Covered in Wave 1 section 1.2 (sticker extraction). Additional Wave 2 work:

1. **Outbound sticker sending** — in `send_attachment()` (`mod.rs:651`):
   - When `mime_type` is `image/webp` or `application/x-tgsticker`: use `bot.send_sticker()` instead of `send_photo()`/`send_document()`

**Files**: `telegram/mod.rs`

### 2.3 Typewriter Mode Polish

**Goal**: Three small improvements, no architecture changes.

1. **Intermediate messages use typewriter** (`reply_emitter.rs:366-370`):
   - When `stream_enabled && is_intermediate && content.chars().count() > TYPEWRITER_CHARS_PER_STEP * 2`:
     use `send_typewriter()` instead of `send_to_channel()`

2. **Persistent typing indicator**:
   - On first `ResponseChunk` received, spawn a background task that sends typing every 4 seconds
   - Cancel on `RunComplete`
   - Requires storing `channel_id` and `conversation_id` in `ReplyEmitter` (already available via `route`)

3. **Adaptive edit interval**:
   - When `revealed_chars > total_chars / 2`: increase interval from 300ms to 500ms progressively
   - Formula: `300 + (revealed_chars - total_chars/2) * 200 / (total_chars/2)` ms, capped at 500ms

**Files**: `reply_emitter.rs`

## Wave 3: Resilience + Pairing

### 3.1 Network Resilience

**Goal**: Detect silent stalls and recover automatically.

1. **Stall detection**:
   - `AtomicU64` timestamp in message handler, updated on every received update
   - Watchdog task checks every 60 seconds; if last update > 90 seconds ago → stall

2. **Auto-restart with exponential backoff**:
   - On stall: stop dispatcher, wait backoff, restart
   - Backoff: 2s → 4s → 8s → 16s → 30s max
   - Reset backoff counter after 60 seconds of stable operation

3. **API error classification** in `send()` and `edit()`:
   - Recoverable (timeout, 5xx): retry up to `config.max_retries`
   - Unrecoverable (401, 400): return error immediately
   - Rate limited (429): return `ChannelError::RateLimited { retry_after_secs }`

**Files**: `telegram/mod.rs`

### 3.2 Pairing Flow

**Goal**: New users authorize via chat conversation, not config file editing.

1. **Pairing code storage** — `TelegramChannel` holds:
   ```rust
   pairing_codes: Arc<RwLock<HashMap<String, PairingEntry>>>
   // PairingEntry { code: String, expires_at: Instant }
   ```

2. **`get_pairing_data()`** — generates 6-digit alphanumeric code, stores with 1-hour TTL

3. **Unauthorized message handling** — in message handler endpoint:
   - If `convert_message()` returns `None` due to allowlist check:
     - If message text matches a valid pairing code → add user to runtime `allowed_users`, persist config, reply "Paired successfully"
     - Otherwise → reply "Please enter your pairing code" (rate-limited to 1 prompt per user per 5 minutes)

4. **Tool integration** (R9):
   - `telegram_pairing_generate`: generates and returns a pairing code
   - `telegram_pairing_list`: lists active pairing codes with expiry

5. **Config persistence** — on successful pairing, append user_id to `allowed_users` via the existing config save mechanism

**Files**: `telegram/mod.rs`, `telegram/config.rs`, new tool definitions

## Architecture Alignment

| Principle | How This Design Complies |
|-----------|--------------------------|
| R1 Brain-Limb Separation | Channel only extracts/passes media, Core does understanding |
| R3 Core Minimalism | No new heavy deps in core — teloxide already present |
| R4 I/O-Only Interfaces | Channel converts formats, doesn't process content |
| R8 LLM Sovereignty | No intent detection or content classification in Channel |
| R9 Everything is a Tool | Pairing management exposed as tools |
| P6 Simplicity | Minimal changes per feature, reuses existing patterns |
| P7 Defensive Design | Error classification, stall detection, graceful fallbacks |

## Files Changed Summary

| File | Wave | Changes |
|------|------|---------|
| `channel.rs` | 1 | `react()` signature: add `conversation_id` |
| `telegram/mod.rs` | 1,2,3 | Reactions, async media extraction, stickers, topic routing, stall detection, pairing |
| `telegram/config.rs` | 3 | Pairing code storage, runtime allowlist mutation |
| `reply_emitter.rs` | 1,2 | Reaction lifecycle, smart chunking, typing persistence, intermediate typewriter |

## Not In Scope

- Per-topic Agent routing (breaks 1:1 model)
- Real-time streaming (chunk-by-chunk edit — architecture change)
- Webhook mode
- Proxy configuration (use `HTTPS_PROXY` env var)
- Sticker description caching (Core multimodal handles this)
- Multi-bot / multi-account support
- QR code pairing
- Admin approval workflow for pairing
