# Telegram Channel Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Strengthen the Telegram channel with processing reactions, multimodal media passthrough, smart text chunking, forum topic isolation, sticker support, typewriter polish, network resilience, and pairing flow.

**Architecture:** Three-wave incremental enhancement. Wave 1 (reactions + multimodal + chunking) delivers immediate UX wins. Wave 2 (topics + stickers + typewriter) adds structural improvements. Wave 3 (resilience + pairing) hardens the channel. All changes stay within the Channel I/O layer; Core multimodal processing is out of scope.

**Tech Stack:** Rust, teloxide 0.13, tokio, tokio-util (already in deps)

**Spec:** `docs/superpowers/specs/2026-03-22-telegram-channel-enhancement-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/gateway/channel.rs` | Modify | Add `conversation_id` to `react()` signature |
| `src/gateway/channel_registry.rs` | Modify | Add `react()` wrapper method |
| `src/gateway/interfaces/telegram/mod.rs` | Modify | Reactions, async media, topics, stickers, stall detection, pairing |
| `src/gateway/interfaces/telegram/config.rs` | Modify | Pairing code storage, runtime allowlist mutation |
| `src/gateway/interfaces/slack/mod.rs` | Modify | Update `react()` signature |
| `src/gateway/interfaces/nostr/mod.rs` | Modify | Update `react()` signature |
| `src/gateway/interfaces/discord/mod.rs` | Modify | Update `react()` signature |
| `src/gateway/interfaces/matrix/mod.rs` | Modify | Update `react()` signature |
| `src/gateway/interfaces/signal/mod.rs` | Modify | Update `react()` signature |
| `src/gateway/interfaces/mattermost/mod.rs` | Modify | Update `react()` signature |
| `src/gateway/interfaces/feishu/mod.rs` | Modify | Update `react()` signature (if exists) |
| `src/gateway/reply_emitter.rs` | Modify | Reaction lifecycle, smart chunking, typing persistence, intermediate typewriter |

---

## Wave 1: Interaction Experience + Multimodal Passthrough

### Task 1: Update `react()` signature across Channel trait and all implementations

**Files:**
- Modify: `src/gateway/channel.rs:489-497`
- Modify: `src/gateway/channel_registry.rs` (add `react()` method)
- Modify: `src/gateway/interfaces/slack/mod.rs:223`
- Modify: `src/gateway/interfaces/nostr/mod.rs:219`
- Modify: `src/gateway/interfaces/discord/mod.rs:481`
- Modify: `src/gateway/interfaces/matrix/mod.rs:243`
- Modify: `src/gateway/interfaces/signal/mod.rs:190`
- Modify: `src/gateway/interfaces/mattermost/mod.rs:288`
- Modify: Any other Channel impl with `react()` override (check feishu)
- Note: `imessage/message_ops.rs:87` has `react(params: ReactParams)` — this is the **`MessageOperations` trait** (tool-side), NOT `Channel::react()`. No change needed for iMessage.

- [ ] **Step 1: Update Channel trait `react()` signature**

In `src/gateway/channel.rs`, change the `react()` method:

```rust
// FROM:
async fn react(&self, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
    if !self.capabilities().reactions {
        return Err(ChannelError::UnsupportedFeature("reactions".to_string()));
    }
    let _ = (message_id, reaction);
    Ok(())
}

// TO:
async fn react(&self, conversation_id: &ConversationId, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
    if !self.capabilities().reactions {
        return Err(ChannelError::UnsupportedFeature("reactions".to_string()));
    }
    let _ = (conversation_id, message_id, reaction);
    Ok(())
}
```

- [ ] **Step 2: Update all Channel implementations that override `react()`**

For each file, add `conversation_id: &ConversationId` as the first parameter after `&self`. The implementations that are stubs (return `UnsupportedFeature`) just need the signature change. Nostr and Mattermost have real implementations — update their signatures and import `ConversationId` if not already imported.

Slack (`mod.rs:223`):
```rust
async fn react(&self, _conversation_id: &ConversationId, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
```

Apply the same pattern to: Nostr, Discord, Matrix, Signal, Mattermost, and any other Channel impl.

- [ ] **Step 3: Add `react()` to `ChannelRegistry`**

In `src/gateway/channel_registry.rs`, after the `edit()` method (~line 262), add:

```rust
/// React to a message in a specific channel
pub async fn react(
    &self,
    channel_id: &ChannelId,
    conversation_id: &ConversationId,
    message_id: &super::channel::MessageId,
    reaction: &str,
) -> ChannelResult<()> {
    let channel_arc = self
        .get(channel_id)
        .await
        .ok_or_else(|| ChannelError::NotConnected(format!("Channel not found: {}", channel_id)))?;

    let channel = channel_arc.read().await;
    channel.react(conversation_id, message_id, reaction).await
}
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS — no callers of the old signature remain (the only callers are the trait default and the implementations themselves)

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "gateway: add conversation_id to react() signature, add ChannelRegistry::react()"
```

---

### Task 2: Implement Telegram `react()` in Channel trait

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs:106-122` (capabilities)
- Modify: `src/gateway/interfaces/telegram/mod.rs` (add `react()` impl in `impl Channel`)

- [ ] **Step 1: Enable reactions capability**

In `mod.rs`, change line 112:
```rust
// FROM:
reactions: false, // Telegram reactions are limited
// TO:
reactions: true,
```

- [ ] **Step 2: Implement `react()` in `impl Channel for TelegramChannel`**

Add after the `send_typing()` method (after line 629):

```rust
async fn react(&self, conversation_id: &ConversationId, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
    let bot = self
        .bot
        .as_ref()
        .ok_or_else(|| ChannelError::NotConnected("Bot not initialized".to_string()))?;

    let chat_id = ChatId(
        conversation_id
            .as_str()
            .parse::<i64>()
            .map_err(|e| ChannelError::Internal(format!("Invalid chat ID: {}", e)))?,
    );

    let msg_id = teloxide::types::MessageId(
        message_id
            .as_str()
            .parse::<i32>()
            .map_err(|e| ChannelError::Internal(format!("Invalid message ID: {}", e)))?,
    );

    let reactions = if reaction.is_empty() {
        vec![] // Remove reactions
    } else {
        vec![teloxide::types::ReactionType::Emoji {
            emoji: reaction.to_string(),
        }]
    };

    // Reactions are non-critical UX — swallow errors silently
    match bot.set_message_reaction(chat_id, msg_id).reaction(reactions).await {
        Ok(_) => {
            tracing::debug!("Reaction '{}' set on message {}", reaction, message_id.as_str());
            Ok(())
        }
        Err(e) => {
            tracing::debug!("Failed to set reaction (non-critical): {}", e);
            Ok(()) // Swallow — reactions are best-effort
        }
    }
}
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "telegram: implement react() with best-effort emoji reactions"
```

---

### Task 3: Add reaction lifecycle to ReplyEmitter

**Files:**
- Modify: `src/gateway/reply_emitter.rs`
- Modify: `src/gateway/inbound_context.rs` (ReplyRoute needs inbound message_id)

- [ ] **Step 1: Extend ReplyRoute with inbound_message_id**

In `src/gateway/inbound_context.rs`, add a field to `ReplyRoute`:

```rust
pub struct ReplyRoute {
    pub channel_id: ChannelId,
    pub conversation_id: ConversationId,
    pub reply_to: Option<MessageId>,
    /// Original inbound message ID (for reactions)
    pub inbound_message_id: Option<MessageId>,
}
```

Update `ReplyRoute::new()` to set `inbound_message_id: None`, and add:
```rust
pub fn with_inbound_message_id(mut self, id: MessageId) -> Self {
    self.inbound_message_id = Some(id);
    self
}
```

Fix any compilation errors from the new field (e.g., struct literal construction sites).

- [ ] **Step 2: Set `inbound_message_id` when constructing ReplyRoute**

Search for where `ReplyRoute` is constructed from `InboundMessage` (likely in `inbound_router` or `execution_engine`). Add `.with_inbound_message_id(message.id.clone())` to the builder chain.

Run: `cargo check -p alephcore` to find all construction sites that need updating.

- [ ] **Step 3: Add reaction methods to ReplyEmitter**

In `reply_emitter.rs`, add helper methods:

```rust
/// React on the original inbound message (best-effort, non-blocking)
async fn react_on_inbound(&self, emoji: &str) {
    if let Some(ref msg_id) = self.route.inbound_message_id {
        let _ = self.channel_registry.react(
            &self.route.channel_id,
            &self.route.conversation_id,
            msg_id,
            emoji,
        ).await;
    }
}
```

- [ ] **Step 4: Wire reactions into `emit()` lifecycle**

In the `emit()` method:

1. On first `ResponseChunk` (when `!has_sent` and `!is_intermediate`): call `self.react_on_inbound("👀").await`
2. On `RunComplete`: call `self.react_on_inbound("👍").await`
3. On `RunError`: call `self.react_on_inbound("👎").await`

```rust
// In ResponseChunk handler, before buffering:
if !self.has_sent.load(Ordering::SeqCst) && !is_intermediate {
    self.react_on_inbound("👀").await;
}

// In RunComplete handler, after sending:
self.react_on_inbound("👍").await;

// In RunError handler, after sending error:
self.react_on_inbound("👎").await;
```

- [ ] **Step 5: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib reply_emitter`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "reply_emitter: add processing status reactions (👀→👍/👎)"
```

---

### Task 4: Async multimodal media passthrough

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs:125-200` (convert_message)
- Modify: `src/gateway/interfaces/telegram/mod.rs:203-291` (extract_attachments)
- Modify: `src/gateway/interfaces/telegram/mod.rs:412-426` (message handler)

- [ ] **Step 1: Make `extract_attachments()` async and accept `&Bot`**

Change the signature and add `getFile` call for each attachment:

```rust
async fn extract_attachments(msg: &teloxide::types::Message, bot: &Bot) -> Vec<Attachment> {
    let mut attachments = Vec::new();

    if let MessageKind::Common(common) = &msg.kind {
        let file_id_opt: Option<(&str, &str, Option<&str>, u64)> = match &common.media_kind {
            MediaKind::Photo(photo) => {
                photo.photo.last().map(|p| (
                    p.file.id.as_str(),
                    "image/jpeg",
                    None,
                    p.file.size as u64,
                ))
            }
            MediaKind::Document(doc) => Some((
                doc.document.file.id.as_str(),
                doc.document.mime_type.as_ref().map(|m| m.as_ref()).unwrap_or("application/octet-stream"),
                doc.document.file_name.as_deref(),
                doc.document.file.size as u64,
            )),
            MediaKind::Audio(audio) => Some((
                audio.audio.file.id.as_str(),
                audio.audio.mime_type.as_ref().map(|m| m.as_ref()).unwrap_or("audio/mpeg"),
                audio.audio.file_name.as_deref(),
                audio.audio.file.size as u64,
            )),
            MediaKind::Video(video) => Some((
                video.video.file.id.as_str(),
                video.video.mime_type.as_ref().map(|m| m.as_ref()).unwrap_or("video/mp4"),
                video.video.file_name.as_deref(),
                video.video.file.size as u64,
            )),
            MediaKind::Voice(voice) => Some((
                voice.voice.file.id.as_str(),
                voice.voice.mime_type.as_ref().map(|m| m.as_ref()).unwrap_or("audio/ogg"),
                None,
                voice.voice.file.size as u64,
            )),
            MediaKind::Sticker(sticker) => {
                let mime = if sticker.sticker.is_animated {
                    "application/x-tgsticker"
                } else if sticker.sticker.is_video {
                    "video/webm"
                } else {
                    "image/webp"
                };
                Some((
                    sticker.sticker.file.id.as_str(),
                    mime,
                    None,
                    sticker.sticker.file.size as u64,
                ))
            }
            _ => None,
        };

        if let Some((file_id, mime_type, filename, size)) = file_id_opt {
            // Resolve download URL via getFile API
            let url = match bot.get_file(file_id).await {
                Ok(file) => {
                    let token = bot.token();
                    Some(format!(
                        "https://api.telegram.org/file/bot{}/{}",
                        token, file.path
                    ))
                }
                Err(e) => {
                    tracing::warn!("Failed to get file URL for {}: {}", file_id, e);
                    None
                }
            };

            attachments.push(Attachment {
                id: file_id.to_string(),
                mime_type: mime_type.to_string(),
                filename: filename.map(|s| s.to_string()),
                size: Some(size),
                url,
                path: None,
                data: None,
            });
        }
    }

    attachments
}
```

- [ ] **Step 2: Make `convert_message()` async and accept `&Bot`**

```rust
async fn convert_message(
    msg: &teloxide::types::Message,
    bot: &Bot,
    config: &TelegramConfig,
    channel_id: &ChannelId,
) -> Option<InboundMessage> {
```

Key changes inside:
1. Move attachment extraction BEFORE the empty text check
2. Add sticker text representation
3. Change empty guard:

```rust
// Extract attachments (async — resolves file URLs)
let attachments = Self::extract_attachments(msg, bot).await;

// Extract text content
let mut text = match &msg.kind {
    MessageKind::Common(common) => match &common.media_kind {
        MediaKind::Text(text_msg) => text_msg.text.clone(),
        MediaKind::Photo(photo) => photo.caption.clone().unwrap_or_default(),
        MediaKind::Document(doc) => doc.caption.clone().unwrap_or_default(),
        MediaKind::Audio(audio) => audio.caption.clone().unwrap_or_default(),
        MediaKind::Video(video) => video.caption.clone().unwrap_or_default(),
        MediaKind::Voice(voice) => voice.caption.clone().unwrap_or_default(),
        MediaKind::Sticker(sticker) => {
            // Use sticker emoji as text representation
            let emoji = sticker.sticker.emoji.as_deref().unwrap_or("?");
            format!("[Sticker: {}]", emoji)
        }
        _ => String::new(),
    },
    _ => return None,
};

// Skip messages with no content at all
if text.is_empty() && attachments.is_empty() {
    return None;
}
```

- [ ] **Step 3: Update message handler closure**

In the `message_handler` endpoint (~line 412), update to pass `bot`:

```rust
let message_handler = Update::filter_message().endpoint(
    move |bot: Bot, msg: teloxide::types::Message| {
        let inbound_tx = inbound_tx.clone();
        let config = config.clone();
        let channel_id = channel_id.clone();
        async move {
            if let Some(inbound) = TelegramChannel::convert_message(&msg, &bot, &config, &channel_id).await {
                if let Err(e) = inbound_tx.send(inbound).await {
                    tracing::error!("Failed to send inbound message: {}", e);
                }
            }
            Ok::<(), std::convert::Infallible>(())
        }
    },
);
```

Note: `bot` is received by value in the closure (teloxide pattern), so passing `&bot` to `convert_message` inside `async move` is valid since `bot` is moved into the async block.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS. Fix any type mismatches (especially around `MediaKind::Sticker` field names — verify against teloxide 0.13 types).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "telegram: async media passthrough with file URL resolution and sticker support"
```

---

### Task 5: Smart text chunking

**Files:**
- Modify: `src/gateway/reply_emitter.rs:307-351` (split_message, find_split_point)

- [ ] **Step 1: Write test for code block splitting**

Add to the test module in `reply_emitter.rs`:

```rust
#[test]
fn test_split_message_preserves_code_block() {
    // Code block should not be split in the middle
    let code = "x".repeat(100);
    let content = format!("Before text\n\n```rust\n{}\n```\n\nAfter text", code);
    let chunks = ReplyEmitter::split_message(&content, 80);
    // No chunk should contain an odd number of ``` markers
    for chunk in &chunks {
        let count = chunk.matches("```").count();
        assert!(
            count % 2 == 0 || count == 0,
            "Chunk has unbalanced code fence: {:?}",
            chunk
        );
    }
}

#[test]
fn test_split_message_html_entity_safe() {
    // Should not split in the middle of &amp;
    let content = format!("{}a&amp;b{}", "x".repeat(95), "y".repeat(100));
    let chunks = ReplyEmitter::split_message(&content, 100);
    for chunk in &chunks {
        // No chunk should end with a partial entity like "&am" or "&amp"
        assert!(
            !chunk.ends_with('&') && !chunk.ends_with("&a") && !chunk.ends_with("&am"),
            "Chunk ends with partial HTML entity: {:?}",
            chunk
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib reply_emitter::tests::test_split_message_preserves_code_block`
Expected: FAIL (code block gets split)

- [ ] **Step 3: Enhance `find_split_point()` with code block and HTML entity awareness**

```rust
fn find_split_point(text: &str, max_len: usize) -> usize {
    let mut safe_max = max_len;
    while safe_max > 0 && !text.is_char_boundary(safe_max) {
        safe_max -= 1;
    }

    let search_range = &text[..safe_max];

    // Check if we're inside a code block (odd number of ``` before split point)
    let fence_count = search_range.matches("```").count();
    if fence_count % 2 == 1 {
        // Inside a code block — try to split before the opening fence
        if let Some(last_fence) = search_range.rfind("```") {
            if last_fence > safe_max / 4 {
                // Split before the code fence (at the preceding newline)
                if let Some(nl) = text[..last_fence].rfind('\n') {
                    return nl + 1;
                }
                return last_fence;
            }
        }
        // If code block starts too early, split at safe_max (unavoidable)
    }

    // Check for partial HTML entities at the split point
    // Look back up to 6 chars for an unfinished &...; entity
    let entity_start = search_range[..safe_max].rfind('&');
    if let Some(amp_pos) = entity_start {
        let after_amp = &search_range[amp_pos..safe_max];
        // If there's an & but no closing ; before safe_max, we're mid-entity
        if !after_amp.contains(';') && after_amp.len() <= 8 {
            safe_max = amp_pos;
        }
    }

    // Existing logic: prefer paragraph boundary, then newline
    let search_range = &text[..safe_max];

    if let Some(pos) = search_range.rfind("\n\n") {
        if pos > safe_max / 4 {
            return pos + 1;
        }
    }

    if let Some(pos) = search_range.rfind('\n') {
        if pos > safe_max / 4 {
            return pos + 1;
        }
    }

    safe_max
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib reply_emitter`
Expected: ALL PASS

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "reply_emitter: code block and HTML entity aware text splitting"
```

---

## Wave 2: Topic Isolation + Sticker + Streaming Polish

### Task 6: Forum topic session isolation

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Add `parse_conversation_id()` helper**

Add to `impl TelegramChannel` (after `extract_attachments`):

```rust
use teloxide::types::ThreadId;

/// Parse a conversation_id that may contain a forum topic suffix.
///
/// Format: "{chat_id}" or "{chat_id}:topic:{thread_id}"
/// Returns (ChatId, Option<ThreadId>) with proper teloxide types.
fn parse_conversation_id(conv_id: &str) -> (ChatId, Option<ThreadId>) {
    if let Some((chat, topic)) = conv_id.split_once(":topic:") {
        let chat_id = ChatId(chat.parse().unwrap_or(0));
        let thread_id = topic.parse::<i32>().ok().map(|id| {
            ThreadId(teloxide::types::MessageId(id))
        });
        (chat_id, thread_id)
    } else {
        (ChatId(conv_id.parse().unwrap_or(0)), None)
    }
}

/// General topic thread_id constant — must NOT be passed to message_thread_id()
const GENERAL_TOPIC: ThreadId = ThreadId(teloxide::types::MessageId(1));
```

- [ ] **Step 2: Encode topic in `convert_message()`**

In `convert_message()`, replace the conversation_id construction:

```rust
// FROM:
conversation_id: ConversationId::new(msg.chat.id.0.to_string()),

// TO:
conversation_id: ConversationId::new(
    if let Some(thread_id) = msg.thread_id {
        format!("{}:topic:{}", msg.chat.id.0, thread_id)
    } else {
        msg.chat.id.0.to_string()
    }
),
```

Note: `msg.thread_id` is `Option<ThreadId>` in teloxide. Access the inner value via `.0.0` (ThreadId wraps MessageId which wraps i32).

- [ ] **Step 3: Update `send()` to use `parse_conversation_id()` and set `message_thread_id`**

Replace the chat_id parsing at the top of `send()`:

```rust
let (chat_id, thread_id) = Self::parse_conversation_id(message.conversation_id.as_str());

// In build_request closure, after setting reply_parameters:
if let Some(tid) = &thread_id {
    if *tid != Self::GENERAL_TOPIC {
        req = req.message_thread_id(*tid);
    }
}
```

Add a defensive fallback: if send returns 400 with "message thread not found", retry once without `message_thread_id` (handles stale topic IDs when Forum mode is disabled).

- [ ] **Step 4: Update `send_typing()` to be topic-aware**

```rust
async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
    let bot = self.bot.as_ref()
        .ok_or_else(|| ChannelError::NotConnected("Bot not initialized".to_string()))?;

    let (chat_id, thread_id) = Self::parse_conversation_id(conversation_id.as_str());

    let mut req = bot.send_chat_action(chat_id, teloxide::types::ChatAction::Typing);
    if let Some(tid) = &thread_id {
        if *tid != Self::GENERAL_TOPIC {
            req = req.message_thread_id(*tid);
        }
    }

    req.await.map_err(|e| ChannelError::Internal(format!("Failed to send typing: {}", e)))?;
    Ok(())
}
```

- [ ] **Step 5: Update `react()` to be topic-aware**

The `react()` implementation from Task 2 already takes `conversation_id` — just update chat_id parsing to use `parse_conversation_id()`:

```rust
let (chat_id, _thread_id) = Self::parse_conversation_id(conversation_id.as_str());
```

- [ ] **Step 6: Update `edit()` to use `parse_conversation_id()`**

In `edit_message()`, replace the chat_id parsing:
```rust
let (chat, _thread_id) = Self::parse_conversation_id(chat_id.as_str());
```

- [ ] **Step 7: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "telegram: forum topic session isolation via conversation_id encoding"
```

---

### Task 7: Outbound sticker sending

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs:650-692` (send_attachment)

- [ ] **Step 1: Add sticker case to `send_attachment()`**

In the MIME type dispatch section, add before the document fallback:

```rust
} else if mime == "image/webp" || mime == "application/x-tgsticker" || mime == "video/webm" {
    bot.send_sticker(chat_id, input_file)
        .await
        .map_err(|e| ChannelError::SendFailed(format!("Failed to send sticker: {}", e)))?;
```

- [ ] **Step 2: Topic-aware attachment sending**

The `send_attachment()` currently takes `chat_id: ChatId`. We need to also pass `thread_id: Option<ThreadId>` and set `message_thread_id` on the send requests. Update the signature:

```rust
async fn send_attachment(
    &self,
    bot: &Bot,
    chat_id: ChatId,
    thread_id: Option<ThreadId>,
    attachment: &Attachment,
) -> ChannelResult<()> {
```

On each `bot.send_photo/audio/video/document/sticker` call, add:
```rust
let mut req = bot.send_photo(chat_id, input_file);
if let Some(tid) = &thread_id {
    if *tid != Self::GENERAL_TOPIC {
        req = req.message_thread_id(*tid);
    }
}
req.await.map_err(...)?;
```

Update the caller in `send()` to pass `thread_id`.

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "telegram: sticker sending and topic-aware attachments"
```

---

### Task 8: Typewriter mode polish

**Files:**
- Modify: `src/gateway/channel_registry.rs` (add `send_typing()` wrapper)
- Modify: `src/gateway/reply_emitter.rs`

- [ ] **Step 1: Add `send_typing()` to ChannelRegistry**

In `src/gateway/channel_registry.rs`, after `react()` (added in Task 1):

```rust
/// Send typing indicator through a specific channel
pub async fn send_typing(
    &self,
    channel_id: &ChannelId,
    conversation_id: &ConversationId,
) -> ChannelResult<()> {
    let channel_arc = self.get(channel_id).await
        .ok_or_else(|| ChannelError::NotConnected(format!("Channel not found: {}", channel_id)))?;
    let channel = channel_arc.read().await;
    channel.send_typing(conversation_id).await
}
```

Run: `cargo check -p alephcore` — Expected: PASS

- [ ] **Step 2: Add CancellationToken for typing indicator**

Add to `ReplyEmitter` struct:

```rust
use tokio_util::sync::CancellationToken;

pub struct ReplyEmitter {
    // ... existing fields ...
    /// Cancellation token for persistent typing indicator task
    typing_cancel: CancellationToken,
}
```

Initialize in constructors: `typing_cancel: CancellationToken::new()`

Add `Drop` impl:
```rust
impl Drop for ReplyEmitter {
    fn drop(&mut self) {
        self.typing_cancel.cancel();
    }
}
```

- [ ] **Step 3: Add persistent typing indicator**

Add method to `ReplyEmitter`:

```rust
/// Start a background task that sends typing indicator every 4 seconds
fn start_typing_indicator(&self) {
    let registry = self.channel_registry.clone();
    let channel_id = self.route.channel_id.clone();
    let conversation_id = self.route.conversation_id.clone();
    let cancel = self.typing_cancel.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(4));
        interval.tick().await; // Skip first immediate tick
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let _ = registry.send_typing(&channel_id, &conversation_id).await;
                }
                _ = cancel.cancelled() => {
                    break;
                }
            }
        }
    });
}
```

- [ ] **Step 4: Wire typing indicator into emit()**

In the `ResponseChunk` handler, on first non-intermediate chunk:

```rust
if !self.has_sent.load(Ordering::SeqCst) && !is_intermediate {
    self.react_on_inbound("👀").await;
    self.start_typing_indicator();
}
```

In `RunComplete` and `RunError`, cancel the typing indicator:

```rust
self.typing_cancel.cancel();
```

- [ ] **Step 5: Intermediate messages use typewriter**

In `emit()` `ResponseChunk` handler, change the intermediate branch:

```rust
if is_intermediate {
    if !content.is_empty() {
        if self.config.stream_enabled && content.chars().count() > TYPEWRITER_CHARS_PER_STEP * 2 {
            self.send_typewriter(&content).await;
        } else {
            self.send_to_channel(&content).await;
        }
    }
}
```

- [ ] **Step 6: Adaptive edit interval**

In `send_typewriter()`, replace the fixed interval:

```rust
// Replace: tokio::time::sleep(TYPEWRITER_EDIT_INTERVAL).await;
// With:
let progress = revealed_chars as f64 / total_chars as f64;
let interval_ms = if progress > 0.5 {
    let extra = ((progress - 0.5) * 2.0 * 200.0) as u64; // 0..200
    300 + extra.min(200)
} else {
    300
};
tokio::time::sleep(Duration::from_millis(interval_ms)).await;
```

- [ ] **Step 7: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib reply_emitter`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "reply_emitter: persistent typing indicator, intermediate typewriter, adaptive intervals"
```

---

## Wave 3: Resilience + Pairing

### Task 9: Network resilience — stall detection and error classification

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs` (dispatcher task)

- [ ] **Step 1: Add stall detection atomic to dispatcher**

Before `tokio::spawn` in `start()`, create the shared timestamp:

```rust
use std::sync::atomic::AtomicU64;
use std::time::{SystemTime, UNIX_EPOCH};

let last_update_at = Arc::new(AtomicU64::new(
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
));
let last_update_for_handler = last_update_at.clone();
let last_update_for_watchdog = last_update_at.clone();
```

In the message handler, update the timestamp:
```rust
last_update_for_handler.store(
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
    std::sync::atomic::Ordering::Relaxed,
);
```

- [ ] **Step 2: Add watchdog task**

Inside the spawned task, after the dispatcher starts, spawn a watchdog:

```rust
let watchdog_cancel = CancellationToken::new();
let watchdog_token = watchdog_cancel.clone();

let watchdog = tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let last = last_update_for_watchdog.load(std::sync::atomic::Ordering::Relaxed);
                if now - last > 90 {
                    tracing::warn!(
                        "Telegram polling stall detected ({}s since last update)",
                        now - last
                    );
                    // NOTE: Auto-restart with exponential backoff is spec'd but deferred.
                    // Requires restructuring the dispatcher lifecycle (stop + recreate + resume).
                    // For now, log warning. The watchdog makes stalls observable for operators.
                    // Future task: implement dispatcher restart with 2s→4s→8s→16s→30s backoff.
                }
            }
            _ = watchdog_token.cancelled() => break,
        }
    }
});
```

Cancel watchdog when dispatcher stops: `watchdog_cancel.cancel();`

- [ ] **Step 3: Add error classification helper**

Add to `impl TelegramChannel`:

```rust
/// Classify a teloxide error for retry decisions
fn classify_error(err: &teloxide::RequestError) -> ErrorClass {
    match err {
        teloxide::RequestError::Api(api_err) => {
            let msg = api_err.to_string();
            if msg.contains("429") || msg.contains("Too Many Requests") {
                // Extract retry_after if available
                ErrorClass::RateLimited(30) // Default 30s
            } else if msg.contains("401") || msg.contains("Unauthorized") {
                ErrorClass::Unrecoverable
            } else if msg.contains("400") {
                ErrorClass::Unrecoverable
            } else {
                ErrorClass::Recoverable
            }
        }
        teloxide::RequestError::Network(_) => ErrorClass::Recoverable,
        _ => ErrorClass::Recoverable,
    }
}

enum ErrorClass {
    Recoverable,
    Unrecoverable,
    RateLimited(u64),
}
```

- [ ] **Step 4: Add retry wrapper for `send()` and `edit()`**

Wrap the actual API calls with retry logic based on `classify_error()`. Keep it simple — retry on `Recoverable`, return immediately on `Unrecoverable`, sleep and retry on `RateLimited`.

```rust
async fn with_retry<F, Fut, T>(&self, max_retries: u32, mut f: F) -> ChannelResult<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, teloxide::RequestError>>,
{
    let mut attempts = 0;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                attempts += 1;
                match Self::classify_error(&e) {
                    ErrorClass::Unrecoverable => {
                        return Err(ChannelError::SendFailed(e.to_string()));
                    }
                    ErrorClass::RateLimited(secs) => {
                        if attempts > max_retries {
                            return Err(ChannelError::RateLimited { retry_after_secs: secs });
                        }
                        tokio::time::sleep(Duration::from_secs(secs)).await;
                    }
                    ErrorClass::Recoverable => {
                        if attempts > max_retries {
                            return Err(ChannelError::SendFailed(e.to_string()));
                        }
                        tokio::time::sleep(Duration::from_millis(500 * attempts as u64)).await;
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 5: Inline retry loop in `send()` main API call**

The generic `with_retry` wrapper has closure ownership issues with `build_request`. Instead, inline a retry loop at the `send()` call site:

```rust
let max_retries = self.config.max_retries;
let mut attempts = 0u32;
let sent = loop {
    let result = build_request(Some(ParseMode::Html), &html_text).await;
    match result {
        Ok(msg) => break msg,
        Err(e) => {
            attempts += 1;
            match Self::classify_error(&e) {
                ErrorClass::Unrecoverable => {
                    // Try plain text fallback (existing behavior)
                    break build_request(None, &message.text).await
                        .map_err(|e| ChannelError::SendFailed(format!("Telegram send error: {}", e)))?;
                }
                ErrorClass::RateLimited(secs) => {
                    if attempts > max_retries {
                        return Err(ChannelError::RateLimited { retry_after_secs: secs });
                    }
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                }
                ErrorClass::Recoverable => {
                    if attempts > max_retries {
                        return Err(ChannelError::SendFailed(e.to_string()));
                    }
                    tokio::time::sleep(Duration::from_millis(500 * attempts as u64)).await;
                }
            }
        }
    }
};
```

Remove the unused generic `with_retry` method — the inline approach is clearer and avoids closure capture issues.

- [ ] **Step 6: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "telegram: stall detection watchdog and API error classification with retry"
```

---

### Task 10: Pairing flow

**Files:**
- Modify: `src/gateway/interfaces/telegram/config.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Add pairing code storage to TelegramChannel**

In `config.rs`, add:

```rust
use std::time::Instant;

/// A pending pairing code
#[derive(Debug, Clone)]
pub struct PairingEntry {
    pub code: String,
    pub created_at: Instant,
    /// TTL in seconds (default 3600 = 1 hour)
    pub ttl_secs: u64,
}

impl PairingEntry {
    pub fn new(code: String) -> Self {
        Self {
            code,
            created_at: Instant::now(),
            ttl_secs: 3600,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed().as_secs() > self.ttl_secs
    }
}
```

Add to `TelegramConfig`:
```rust
/// Runtime-mutable allowed users (extends the configured list)
#[serde(skip)]
pub runtime_allowed_users: Vec<i64>,
```

Update `is_user_allowed()` to also check `runtime_allowed_users`.

- [ ] **Step 2: Add pairing state to TelegramChannel**

Add fields to `TelegramChannel`:

```rust
use tokio::sync::RwLock;

pub struct TelegramChannel {
    // ... existing fields ...
    /// Active pairing codes
    pairing_codes: Arc<RwLock<HashMap<String, PairingEntry>>>,
    /// Rate limiter for pairing prompts: user_id -> last prompt time
    pairing_prompt_times: Arc<RwLock<HashMap<i64, Instant>>>,
}
```

Initialize in `new()`:
```rust
pairing_codes: Arc::new(RwLock::new(HashMap::new())),
pairing_prompt_times: Arc::new(RwLock::new(HashMap::new())),
```

- [ ] **Step 3: Implement `get_pairing_data()`**

Override the default in `impl Channel for TelegramChannel`:

```rust
async fn get_pairing_data(&self) -> ChannelResult<PairingData> {
    use rand::Rng;
    let code: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(6)
        .map(char::from)
        .collect::<String>()
        .to_uppercase();

    let entry = PairingEntry::new(code.clone());
    self.pairing_codes.write().await.insert(code.clone(), entry);

    // Clean up expired codes
    self.pairing_codes.write().await.retain(|_, e| !e.is_expired());

    Ok(PairingData::Code(code))
}
```

- [ ] **Step 4: Handle unauthorized messages in the dispatcher**

In the message handler, after `convert_message()` returns `None`, add pairing logic:

```rust
if let Some(inbound) = TelegramChannel::convert_message(&msg, &bot, &config, &channel_id).await {
    // ... existing send logic ...
} else if let Some(from) = &msg.from {
    let user_id = from.id.0 as i64;
    // Check if user sent a pairing code
    if let Some(text) = msg.text() {
        let code = text.trim().to_uppercase();
        let mut codes = pairing_codes.write().await;
        if let Some(entry) = codes.get(&code) {
            if !entry.is_expired() {
                // Pairing successful
                codes.remove(&code);
                config_mut.runtime_allowed_users.push(user_id);
                // TODO: persist to config file
                let _ = bot.send_message(msg.chat.id, "✓ Paired successfully!").await;
                tracing::info!("User {} paired via code {}", user_id, code);
            } else {
                codes.remove(&code);
                let _ = bot.send_message(msg.chat.id, "Pairing code expired. Please request a new one.").await;
            }
        } else {
            // Rate-limited prompt
            let mut times = pairing_prompt_times.write().await;
            let should_prompt = times.get(&user_id)
                .map(|t| t.elapsed().as_secs() > 300)
                .unwrap_or(true);
            if should_prompt {
                times.insert(user_id, Instant::now());
                let _ = bot.send_message(msg.chat.id, "Please enter your pairing code to start.").await;
            }
        }
    }
}
```

Note: The `pairing_codes`, `pairing_prompt_times`, and `config` need to be cloned into the closure. Since config is currently immutable (`config.clone()`), we'll need to share the runtime_allowed_users via `Arc<RwLock<Vec<i64>>>` instead of putting it directly in config.

- [ ] **Step 5: Implement config persistence on successful pairing**

After adding user to runtime allowlist, persist the change. The config save mechanism is in `src/config/save.rs` (atomic write via temp + rename).

In the pairing success branch, after `config_mut.runtime_allowed_users.push(user_id)`:
```rust
// Persist the new user to config file
// Load current config, append user_id to allowed_users, save
if let Ok(mut config) = Config::load_from_file(&config_path) {
    // Find the telegram channel config and add user
    for channel in &mut config.channels {
        if channel.channel_type == "telegram" {
            if let Some(allowed) = channel.config.get_mut("allowed_users") {
                if let Some(arr) = allowed.as_array_mut() {
                    arr.push(serde_json::Value::Number(user_id.into()));
                }
            }
        }
    }
    if let Err(e) = config.save_to_file(&config_path) {
        tracing::warn!("Failed to persist pairing: {}", e);
        // Non-fatal — runtime allowlist is already updated
    }
}
```

Note: The config path needs to be available in the handler. Pass it via the shared state or resolve from `AppContext`.

- [ ] **Step 6: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS — may need to adjust shared state patterns for the closure.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "telegram: pairing flow with config persistence"
```

---

### Task 10b: Pairing management tools (R9)

**Files:**
- Create or modify tool definitions (location depends on existing tool registration pattern)

- [ ] **Step 1: Find existing tool registration pattern**

Search for how tools like `telegram_send` or `message_send` are defined and registered. Follow the same pattern.

Run: `grep -r "tool_name.*telegram\|register.*tool" src/builtin_tools/ --include="*.rs" -l`

- [ ] **Step 2: Define `telegram_pairing_generate` tool**

The tool should:
- Accept no parameters (or optional TTL)
- Call `channel.get_pairing_data()` on the Telegram channel
- Return the 6-digit code

- [ ] **Step 3: Define `telegram_pairing_list` tool**

The tool should:
- Accept no parameters
- Read `pairing_codes` from the Telegram channel
- Return a list of active codes with expiry times

- [ ] **Step 4: Register both tools**

Follow the existing registration pattern to make tools available to LLM.

- [ ] **Step 5: Compile and test**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "telegram: add pairing management tools (R9 compliance)"
```

---

### Task 11: Final cleanup and documentation

- [ ] **Step 1: Update module doc comment**

In `telegram/mod.rs`, update the doc comment at the top to reflect new features:

```rust
//! Telegram Channel Implementation
//!
//! # Features
//!
//! - Long-polling or webhook mode
//! - User/group allowlists with pairing flow
//! - File and image attachments with URL resolution
//! - Inline keyboards with callback routing
//! - Reply threading
//! - Forum topic session isolation
//! - Processing status reactions (👀/👍/👎)
//! - Sticker support (static/animated/video)
//! - Network stall detection
```

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: ALL PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No new warnings

- [ ] **Step 4: Final commit**

```bash
git add -A && git commit -m "telegram: update docs and cleanup for channel enhancement"
```
