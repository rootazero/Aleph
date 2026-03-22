# Feishu Enhanced Features Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add streaming card output, markdown rendering, and typing indicators to the existing Feishu channel.

**Architecture:** A new `FeishuEventEmitter` implements the `EventEmitter` trait and intercepts `ResponseChunk` events for real-time Card Kit streaming updates. It wraps the standard `ReplyEmitter` for fallback. The emitter also manages typing indicator lifecycle (emoji reactions). Markdown rendering is achieved through Feishu's Card Kit schema 2.0 `markdown` tag — no conversion needed.

**Tech Stack:** Rust, tokio, reqwest, async-trait, serde_json (all existing deps)

**Spec:** `docs/superpowers/specs/2026-03-22-feishu-enhanced-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `core/src/gateway/interfaces/feishu/types.rs` | Modify | Add config fields, card payload types, ReactionResponse |
| `core/src/gateway/interfaces/feishu/client.rs` | Modify | Add Card Kit API methods, reaction methods, send_card |
| `core/src/gateway/interfaces/feishu/streaming.rs` | Create | FeishuStreamingCard + FeishuEventEmitter |
| `core/src/gateway/interfaces/feishu/mod.rs` | Modify | Update send() for markdown cards, update capabilities, add streaming module |
| `core/src/gateway/inbound_router/executor.rs` | Modify | Construct FeishuEventEmitter when channel is feishu |
| `interfaces/webchat/src/views/settings/channels/definitions.rs` | Modify | Add streaming, render_mode, typing_indicator UI fields |

---

### Task 1: Extend Types and Config

**Files:**
- Modify: `core/src/gateway/interfaces/feishu/types.rs`

- [ ] **Step 1: Add new config fields and types**

Add after the existing `FeishuConfig` fields (before the closing `}`):

```rust
    #[serde(default = "default_true")]
    pub streaming: bool,
    #[serde(default = "default_render_mode")]
    pub render_mode: String,
    #[serde(default = "default_true")]
    pub typing_indicator: bool,
```

Add the `default_render_mode` function near the other default functions:

```rust
fn default_render_mode() -> String { "auto".to_string() }
```

Add new API response types at the end of the file (before `#[cfg(test)]`):

```rust
// ── Reaction API Response ──

#[derive(Debug, Deserialize)]
pub struct ReactionResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<ReactionData>,
}

#[derive(Debug, Deserialize)]
pub struct ReactionData {
    pub reaction_id: Option<String>,
}

// ── Card Kit API Response ──

#[derive(Debug, Deserialize)]
pub struct CardCreateResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<CardCreateData>,
}

#[derive(Debug, Deserialize)]
pub struct CardCreateData {
    pub card_id: Option<String>,
}

// ── Typing State ──

#[derive(Debug, Clone)]
pub struct TypingState {
    pub message_id: String,
    pub reaction_id: String,
}
```

- [ ] **Step 2: Add tests for new config fields**

Add to the existing `tests` module:

```rust
    #[test]
    fn test_config_streaming_defaults() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret"
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(config.streaming);
        assert_eq!(config.render_mode, "auto");
        assert!(config.typing_indicator);
    }

    #[test]
    fn test_config_streaming_overrides() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret",
            "streaming": false,
            "render_mode": "card",
            "typing_indicator": false
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(!config.streaming);
        assert_eq!(config.render_mode, "card");
        assert!(!config.typing_indicator);
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib feishu::types -- --nocapture`
Expected: All tests PASS (including 2 new ones)

- [ ] **Step 4: Commit**

```bash
git add core/src/gateway/interfaces/feishu/types.rs
git commit -m "feat(feishu): add streaming, render_mode, typing config fields and API types"
```

---

### Task 2: Extend Client with Card Kit and Reaction APIs

**Files:**
- Modify: `core/src/gateway/interfaces/feishu/client.rs`

- [ ] **Step 1: Add Card Kit API methods**

Add these methods to `impl FeishuClient` (after the `download_media` method):

```rust
    // ── Card Kit (Streaming Cards) ──

    /// Create a streaming card and return the card_id.
    pub async fn create_streaming_card(&self, initial_text: &str) -> Result<String, String> {
        let token = self.get_token().await?;
        let url = format!("{}/open-apis/cardkit/v1/cards", self.base_url);

        let card_body = serde_json::json!({
            "schema": "2.0",
            "config": {
                "streaming_mode": true,
                "summary": { "content": "[Generating...]" },
                "streaming_config": {
                    "print_frequency_ms": { "default": 50 },
                    "print_step": { "default": 1 }
                }
            },
            "body": {
                "elements": [
                    { "tag": "markdown", "content": initial_text, "element_id": "content" }
                ]
            }
        });

        let body = serde_json::json!({
            "type": "card_json",
            "data": card_body.to_string(),
        });

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Create streaming card failed: {e}"))?;

        let card_resp: super::types::CardCreateResponse = resp.json().await
            .map_err(|e| format!("Card create response parse failed: {e}"))?;

        if card_resp.code != 0 {
            return Err(format!("Card create error: code={}, msg={}", card_resp.code, card_resp.msg));
        }

        card_resp.data
            .and_then(|d| d.card_id)
            .ok_or_else(|| "No card_id in response".to_string())
    }

    /// Send a card message (streaming or static) to a chat.
    pub async fn send_card_message(
        &self,
        chat_id: &str,
        card_id: &str,
        reply_to: Option<&str>,
    ) -> Result<String, FeishuSendError> {
        let content = serde_json::json!({
            "type": "card",
            "data": { "card_id": card_id }
        });

        if let Some(msg_id) = reply_to {
            return self.reply_message(msg_id, "interactive", &content.to_string()).await;
        }

        let token = self.get_token().await?;
        let url = format!("{}/open-apis/im/v1/messages?receive_id_type=chat_id", self.base_url);

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "interactive",
            "content": content.to_string(),
        });

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send card message failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    /// Update a streaming card's content element.
    pub async fn update_streaming_card(
        &self,
        card_id: &str,
        content: &str,
        sequence: u32,
    ) -> Result<(), String> {
        let token = self.get_token().await?;
        let url = format!(
            "{}/open-apis/cardkit/v1/cards/{}/elements/content/content",
            self.base_url, card_id
        );

        let body = serde_json::json!({
            "content": content,
            "sequence": sequence,
            "uuid": format!("s_{}_{}", card_id, sequence),
        });

        let resp = self.http.put(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Update streaming card failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Update streaming card HTTP {}", resp.status()));
        }
        Ok(())
    }

    /// Close a streaming card (disable streaming_mode).
    pub async fn close_streaming_card(
        &self,
        card_id: &str,
        summary: &str,
        sequence: u32,
    ) -> Result<(), String> {
        let token = self.get_token().await?;
        let url = format!("{}/open-apis/cardkit/v1/cards/{}/settings", self.base_url, card_id);

        let settings = serde_json::json!({
            "config": {
                "streaming_mode": false,
                "summary": {
                    "content": summary.get(..50).unwrap_or(summary)
                }
            }
        });

        let body = serde_json::json!({
            "settings": settings.to_string(),
            "sequence": sequence,
            "uuid": format!("c_{}_{}", card_id, sequence),
        });

        let resp = self.http.patch(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Close streaming card failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Close streaming card HTTP {}", resp.status()));
        }
        Ok(())
    }

    /// Send a static markdown card (non-streaming).
    pub async fn send_card(
        &self,
        chat_id: &str,
        markdown_text: &str,
        reply_to: Option<&str>,
    ) -> Result<String, FeishuSendError> {
        let card = serde_json::json!({
            "schema": "2.0",
            "config": { "wide_screen_mode": true },
            "body": {
                "elements": [
                    { "tag": "markdown", "content": markdown_text }
                ]
            }
        });

        if let Some(msg_id) = reply_to {
            return self.reply_message(msg_id, "interactive", &card.to_string()).await;
        }

        let token = self.get_token().await?;
        let url = format!("{}/open-apis/im/v1/messages?receive_id_type=chat_id", self.base_url);

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "interactive",
            "content": card.to_string(),
        });

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send card failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    // ── Reactions ──

    /// Add an emoji reaction to a message. Returns reaction_id.
    pub async fn add_reaction(&self, message_id: &str, emoji_type: &str) -> Result<String, String> {
        let token = self.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/reactions",
            self.base_url, message_id
        );

        let body = serde_json::json!({
            "reaction_type": { "emoji_type": emoji_type }
        });

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Add reaction failed: {e}"))?;

        let reaction_resp: super::types::ReactionResponse = resp.json().await
            .map_err(|e| format!("Reaction response parse failed: {e}"))?;

        if reaction_resp.code != 0 {
            return Err(format!("Reaction error: code={}, msg={}", reaction_resp.code, reaction_resp.msg));
        }

        reaction_resp.data
            .and_then(|d| d.reaction_id)
            .ok_or_else(|| "No reaction_id in response".to_string())
    }

    /// Remove an emoji reaction from a message.
    pub async fn remove_reaction(&self, message_id: &str, reaction_id: &str) -> Result<(), String> {
        let token = self.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/reactions/{}",
            self.base_url, message_id, reaction_id
        );

        let resp = self.http.delete(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| format!("Remove reaction failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Remove reaction HTTP {}", resp.status()));
        }
        Ok(())
    }
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles (new methods not called yet)

- [ ] **Step 3: Commit**

```bash
git add core/src/gateway/interfaces/feishu/client.rs
git commit -m "feat(feishu): add Card Kit streaming, static card, and reaction API methods"
```

---

### Task 3: Create FeishuEventEmitter (`streaming.rs`)

**Files:**
- Create: `core/src/gateway/interfaces/feishu/streaming.rs`
- Reference: `core/src/gateway/event_emitter/mod.rs` (EventEmitter trait: `emit()`, `next_seq()`)
- Reference: `core/src/gateway/reply_emitter.rs` (ReplyEmitter to wrap)

This is the most complex task. The `FeishuEventEmitter` implements `EventEmitter`, intercepts `ResponseChunk` for real-time card streaming, and manages typing indicators.

- [ ] **Step 1: Create streaming.rs**

```rust
// core/src/gateway/interfaces/feishu/streaming.rs

use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::sync_primitives::Arc;
use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::reply_emitter::ReplyEmitter;
use crate::gateway::inbound_context::ReplyRoute;
use crate::gateway::channel::ConversationId;

use super::client::FeishuClient;
use super::types::TypingState;

const STREAM_THROTTLE_MS: u64 = 100;

/// Manages a single streaming card lifecycle.
pub struct FeishuStreamingCard {
    card_id: String,
    sequence: AtomicU32,
    accumulated_text: Mutex<String>,
    last_update: Mutex<Instant>,
    closed: AtomicBool,
}

impl FeishuStreamingCard {
    fn new(card_id: String) -> Self {
        Self {
            card_id,
            sequence: AtomicU32::new(1),
            accumulated_text: Mutex::new(String::new()),
            last_update: Mutex::new(Instant::now()),
            closed: AtomicBool::new(false),
        }
    }

    fn next_sequence(&self) -> u32 {
        self.sequence.fetch_add(1, Ordering::SeqCst)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Accumulate text and send a throttled update to the card.
    async fn update(&self, client: &FeishuClient, chunk: &str) {
        if self.is_closed() {
            return;
        }

        // Accumulate
        {
            let mut text = self.accumulated_text.lock().await;
            text.push_str(chunk);
        }

        // Throttle check
        let should_send = {
            let last = self.last_update.lock().await;
            last.elapsed() >= Duration::from_millis(STREAM_THROTTLE_MS)
        };

        if should_send {
            self.flush(client).await;
        }
    }

    /// Send the current accumulated text to the card.
    async fn flush(&self, client: &FeishuClient) {
        if self.is_closed() {
            return;
        }

        let text = {
            let text = self.accumulated_text.lock().await;
            text.clone()
        };

        if text.is_empty() {
            return;
        }

        let seq = self.next_sequence();
        match client.update_streaming_card(&self.card_id, &text, seq).await {
            Ok(()) => {
                *self.last_update.lock().await = Instant::now();
            }
            Err(e) => {
                warn!("Failed to update streaming card: {e}");
            }
        }
    }

    /// Close the streaming card with final text.
    async fn close(&self, client: &FeishuClient, final_text: &str) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return; // Already closed
        }

        // Final content update
        let seq = self.next_sequence();
        if let Err(e) = client.update_streaming_card(&self.card_id, final_text, seq).await {
            warn!("Failed to send final streaming card update: {e}");
        }

        // Disable streaming mode
        let summary = final_text.get(..50).unwrap_or(final_text);
        let close_seq = self.next_sequence();
        if let Err(e) = client.close_streaming_card(&self.card_id, summary, close_seq).await {
            warn!("Failed to close streaming card: {e}");
        }
    }
}

/// EventEmitter that streams to Feishu cards in real-time.
///
/// Wraps a standard ReplyEmitter for non-streaming events.
/// When streaming is enabled, intercepts ResponseChunk events to
/// create and update a Feishu streaming card in real-time.
pub struct FeishuEventEmitter {
    /// Fallback emitter for non-streaming or non-chunk events
    inner: ReplyEmitter,
    /// Feishu client for API calls
    client: Arc<FeishuClient>,
    /// Active streaming card
    card: Arc<Mutex<Option<FeishuStreamingCard>>>,
    /// Reply route (for sending the initial card message)
    route: ReplyRoute,
    /// Chat ID for sending messages
    chat_id: String,
    /// Reply-to message ID (for threading)
    reply_to_message_id: Option<String>,
    /// Whether streaming cards are enabled
    streaming_enabled: bool,
    /// Whether typing indicator is enabled
    typing_enabled: bool,
    /// Typing indicator state
    typing_state: Arc<StdMutex<Option<TypingState>>>,
    /// Sequence counter for EventEmitter trait
    seq_counter: AtomicU64,
}

impl FeishuEventEmitter {
    pub fn new(
        inner: ReplyEmitter,
        client: Arc<FeishuClient>,
        route: ReplyRoute,
        chat_id: String,
        reply_to_message_id: Option<String>,
        streaming_enabled: bool,
        typing_enabled: bool,
    ) -> Self {
        Self {
            inner,
            client,
            card: Arc::new(Mutex::new(None)),
            route,
            chat_id,
            reply_to_message_id,
            streaming_enabled,
            typing_enabled,
            typing_state: Arc::new(StdMutex::new(None)),
            seq_counter: AtomicU64::new(0),
        }
    }

    /// Add typing reaction to the inbound message.
    async fn start_typing(&self) {
        if !self.typing_enabled {
            return;
        }

        // Use reply_to_message_id as the message to react to
        let msg_id = match &self.reply_to_message_id {
            Some(id) => id.clone(),
            None => return,
        };

        match self.client.add_reaction(&msg_id, "Typing").await {
            Ok(reaction_id) => {
                let mut state = self.typing_state.lock().unwrap_or_else(|e| e.into_inner());
                *state = Some(TypingState {
                    message_id: msg_id,
                    reaction_id,
                });
                debug!("Added typing indicator");
            }
            Err(e) => {
                debug!("Failed to add typing indicator (non-critical): {e}");
            }
        }
    }

    /// Remove typing reaction.
    async fn stop_typing(&self) {
        let state = {
            let mut guard = self.typing_state.lock().unwrap_or_else(|e| e.into_inner());
            guard.take()
        };

        if let Some(typing) = state {
            if let Err(e) = self.client.remove_reaction(&typing.message_id, &typing.reaction_id).await {
                debug!("Failed to remove typing indicator (non-critical): {e}");
            } else {
                debug!("Removed typing indicator");
            }
        }
    }

    /// Create a streaming card and send it to the chat.
    async fn create_card(&self) -> Option<FeishuStreamingCard> {
        let card_id = match self.client.create_streaming_card("⏳ Thinking...").await {
            Ok(id) => id,
            Err(e) => {
                warn!("Failed to create streaming card, falling back to instant: {e}");
                return None;
            }
        };

        // Send the card message to the chat
        let reply_to = self.reply_to_message_id.as_deref();
        match self.client.send_card_message(&self.chat_id, &card_id, reply_to).await {
            Ok(_msg_id) => {
                debug!("Streaming card created and sent: {card_id}");
                Some(FeishuStreamingCard::new(card_id))
            }
            Err(e) => {
                warn!("Failed to send streaming card message: {:?}", e);
                None
            }
        }
    }
}

#[async_trait]
impl EventEmitter for FeishuEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        match &event {
            StreamEvent::ResponseChunk { content, is_final, is_intermediate, .. } => {
                // Intermediate messages: delegate to inner (sends immediately)
                if *is_intermediate {
                    return self.inner.emit(event).await;
                }

                // Non-streaming mode: delegate everything to inner
                if !self.streaming_enabled {
                    return self.inner.emit(event).await;
                }

                // Start typing on first chunk
                {
                    let card = self.card.lock().await;
                    if card.is_none() {
                        drop(card);
                        self.start_typing().await;
                    }
                }

                // Create card on first non-empty chunk
                if !content.is_empty() {
                    let mut card_guard = self.card.lock().await;
                    if card_guard.is_none() {
                        if let Some(new_card) = self.create_card().await {
                            *card_guard = Some(new_card);
                        } else {
                            // Fallback to inner emitter
                            drop(card_guard);
                            return self.inner.emit(event).await;
                        }
                    }

                    // Update card with new content
                    if let Some(card) = card_guard.as_ref() {
                        card.update(&self.client, content).await;
                    }
                }

                // On final chunk: flush and close
                if *is_final {
                    let card_guard = self.card.lock().await;
                    if let Some(card) = card_guard.as_ref() {
                        let final_text = card.accumulated_text.lock().await.clone();
                        card.close(&self.client, &final_text).await;
                    }
                    drop(card_guard);
                    self.stop_typing().await;
                }

                Ok(())
            }

            StreamEvent::RunComplete { .. } => {
                // Ensure card is closed if still open
                let card_guard = self.card.lock().await;
                if let Some(card) = card_guard.as_ref() {
                    if !card.is_closed() {
                        let final_text = card.accumulated_text.lock().await.clone();
                        card.close(&self.client, &final_text).await;
                    }
                    drop(card_guard);
                    self.stop_typing().await;
                    Ok(())
                } else {
                    drop(card_guard);
                    self.stop_typing().await;
                    // No card was created — delegate to inner for instant send
                    self.inner.emit(event).await
                }
            }

            StreamEvent::RunError { .. } => {
                // Close card if open, then delegate error to inner
                let card_guard = self.card.lock().await;
                if let Some(card) = card_guard.as_ref() {
                    if !card.is_closed() {
                        let text = card.accumulated_text.lock().await.clone();
                        let error_text = if text.is_empty() {
                            "❌ An error occurred".to_string()
                        } else {
                            text
                        };
                        card.close(&self.client, &error_text).await;
                    }
                }
                drop(card_guard);
                self.stop_typing().await;
                self.inner.emit(event).await
            }

            // All other events: delegate to inner
            _ => self.inner.emit(event).await,
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}
```

- [ ] **Step 2: Add module declaration in `mod.rs`**

At the top of `core/src/gateway/interfaces/feishu/mod.rs`, after `pub mod client;`, add:

```rust
pub mod streaming;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles. Fix any import issues if needed.

- [ ] **Step 4: Commit**

```bash
git add core/src/gateway/interfaces/feishu/streaming.rs core/src/gateway/interfaces/feishu/mod.rs
git commit -m "feat(feishu): add FeishuEventEmitter with streaming cards and typing indicators"
```

---

### Task 4: Update Channel for Markdown Cards

**Files:**
- Modify: `core/src/gateway/interfaces/feishu/mod.rs`

Update the `send()` method to use card rendering for markdown content, and update capabilities/manifest.

- [ ] **Step 1: Add `should_use_card` function and update `send()`**

Add the helper function (before `impl Channel for FeishuChannel`):

```rust
/// Determine if the response should use a Feishu card for markdown rendering.
fn should_use_card(text: &str, render_mode: &str) -> bool {
    match render_mode {
        "card" => true,
        "raw" => false,
        // "auto": use card for rich/long content
        _ => text.len() > 200
            || text.contains("```")
            || text.contains("|---|")
            || text.contains("|:--"),
    }
}
```

In the `send()` method, replace the text-only sending logic (the `else` branch where `has_image` is false) with:

```rust
        } else {
            if message.text.is_empty() {
                return Err(ChannelError::SendFailed("Empty message".to_string()));
            }
            // Use card for markdown-rich content
            if should_use_card(&message.text, &self.config.render_mode) {
                client.send_card(chat_id, &message.text, reply_to).await
                    .map_err(map_send_error)?
            } else {
                client.send_text(chat_id, &message.text, reply_to).await
                    .map_err(map_send_error)?
            }
        };
```

- [ ] **Step 2: Update capabilities**

Replace the `capabilities()` function body:

```rust
    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: false,
            images: true,
            audio: false,
            video: false,
            reactions: true,
            replies: true,
            editing: true,
            deletion: false,
            typing_indicator: true,
            read_receipts: false,
            rich_text: true,
            max_message_length: 4096,
            max_attachment_size: 20 * 1024 * 1024,
        }
    }
```

- [ ] **Step 3: Update InteractionManifest**

Replace the `ChannelProvider` impl:

```rust
impl ChannelProvider for FeishuChannel {
    fn interaction_manifest(&self) -> InteractionManifest {
        InteractionManifest::new(InteractionParadigm::Messaging)
            .with_constraints(
                InteractionConstraints::new()
                    .max_output_chars(4096)
                    .supports_streaming(self.config.streaming)
                    .prefer_compact(false),
            )
    }
}
```

- [ ] **Step 4: Add test for `should_use_card`**

Add at the bottom of `mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::should_use_card;

    #[test]
    fn test_should_use_card_auto_plain() {
        assert!(!should_use_card("Hello world", "auto"));
    }

    #[test]
    fn test_should_use_card_auto_code_block() {
        assert!(should_use_card("Here is code:\n```rust\nfn main() {}\n```", "auto"));
    }

    #[test]
    fn test_should_use_card_auto_table() {
        assert!(should_use_card("| A |---|B |", "auto"));
    }

    #[test]
    fn test_should_use_card_auto_long() {
        let long_text = "a".repeat(201);
        assert!(should_use_card(&long_text, "auto"));
    }

    #[test]
    fn test_should_use_card_forced() {
        assert!(should_use_card("Hi", "card"));
    }

    #[test]
    fn test_should_use_card_raw() {
        assert!(!should_use_card("```code```", "raw"));
    }
}
```

- [ ] **Step 5: Verify compilation and tests**

Run: `cargo test -p alephcore --lib feishu -- --nocapture`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add core/src/gateway/interfaces/feishu/mod.rs
git commit -m "feat(feishu): add markdown card rendering and updated capabilities"
```

---

### Task 5: Wire FeishuEventEmitter into Execution Flow

**Files:**
- Modify: `core/src/gateway/inbound_router/executor.rs:73-90`

This is the key integration point. When the inbound message comes from a feishu channel with streaming enabled, construct a `FeishuEventEmitter` instead of the standard `ReplyEmitter`.

- [ ] **Step 1: Add feishu detection and emitter construction**

Replace lines 73-90 in `executor.rs` with:

```rust
        let reply_config = match &self.app_config {
            Some(cfg) => {
                let cfg = cfg.read().await;
                let mode = cfg.behavior.as_ref()
                    .map(|b| b.output_mode.as_str())
                    .unwrap_or("typewriter");
                ReplyEmitterConfig::from_output_mode(mode)
            }
            None => ReplyEmitterConfig::default(),
        };

        // Check if this is a Feishu channel with streaming enabled
        let emitter: Arc<dyn EventEmitter + Send + Sync> = {
            let channel_type = self.channel_registry
                .get_channel_type(&ctx.reply_route.channel_id).await;

            if channel_type.as_deref() == Some("feishu") {
                // Try to get feishu config and client
                if let Some(feishu_emitter) = self.try_create_feishu_emitter(
                    &ctx, &run_id, reply_config.clone(),
                ).await {
                    Arc::new(feishu_emitter)
                } else {
                    Arc::new(ReplyEmitter::with_config(
                        self.channel_registry.clone(),
                        ctx.reply_route.clone(),
                        run_id.clone(),
                        reply_config,
                    ))
                }
            } else {
                Arc::new(ReplyEmitter::with_config(
                    self.channel_registry.clone(),
                    ctx.reply_route.clone(),
                    run_id.clone(),
                    reply_config,
                ))
            }
        };
```

- [ ] **Step 2: Add `try_create_feishu_emitter` method**

Add this method to the impl block that contains `execute_for_context_inner`. You'll need to check what struct it's on — look at the file to find the correct `impl` block.

```rust
    /// Try to create a FeishuEventEmitter for feishu channels.
    /// Returns None if the channel doesn't have an active feishu client.
    async fn try_create_feishu_emitter(
        &self,
        ctx: &InboundContext,
        run_id: &str,
        reply_config: ReplyEmitterConfig,
    ) -> Option<crate::gateway::interfaces::feishu::streaming::FeishuEventEmitter> {
        use crate::gateway::interfaces::feishu::streaming::FeishuEventEmitter;
        use crate::gateway::interfaces::feishu::FeishuConfig;

        // Get feishu config from app config
        let (streaming_enabled, typing_enabled) = match &self.app_config {
            Some(cfg) => {
                let cfg = cfg.read().await;
                // Look for feishu channel config in the channels array
                let feishu_cfg = cfg.channels.iter()
                    .find(|ch| ch.id == ctx.reply_route.channel_id.as_str())
                    .and_then(|ch| serde_json::from_value::<FeishuConfig>(ch.config.clone()).ok());
                match feishu_cfg {
                    Some(fc) => (fc.streaming, fc.typing_indicator),
                    None => (true, true), // defaults
                }
            }
            None => (true, true),
        };

        // Get the feishu client from the channel registry
        let client = self.channel_registry
            .get_feishu_client(&ctx.reply_route.channel_id).await?;

        let inner = ReplyEmitter::with_config(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
            run_id.to_string(),
            reply_config,
        );

        Some(FeishuEventEmitter::new(
            inner,
            client,
            ctx.reply_route.clone(),
            ctx.message.conversation_id.as_str().to_string(),
            ctx.reply_route.reply_to.as_ref().map(|id| id.as_str().to_string()),
            streaming_enabled,
            typing_enabled,
        ))
    }
```

- [ ] **Step 3: Add `get_channel_type` and `get_feishu_client` to ChannelRegistry**

In `core/src/gateway/channel_registry.rs`, add these helper methods:

```rust
    /// Get the channel type for a given channel ID.
    pub async fn get_channel_type(&self, channel_id: &ChannelId) -> Option<String> {
        let channels = self.channels.read().await;
        channels.get(channel_id.as_str())
            .map(|handle| handle.channel_type().to_string())
    }

    /// Get an Arc<FeishuClient> if the channel is a running feishu channel.
    /// Returns None if the channel is not feishu or not connected.
    pub async fn get_feishu_client(&self, channel_id: &ChannelId) -> Option<Arc<crate::gateway::interfaces::feishu::client::FeishuClient>> {
        // This requires the feishu channel to expose its client.
        // For now, return None — the FeishuEventEmitter will create its own client
        // from the config. This is a clean separation.
        None
    }
```

**Alternative approach (simpler):** Instead of getting the client from the registry, have the `try_create_feishu_emitter` create a fresh `FeishuClient` from the config. This is cleaner because it doesn't require the channel to expose its internal client.

Update `try_create_feishu_emitter` to create its own client:

```rust
        // Create a dedicated feishu client for the emitter
        let feishu_cfg = match &self.app_config {
            Some(cfg) => {
                let cfg = cfg.read().await;
                cfg.channels.iter()
                    .find(|ch| ch.id == ctx.reply_route.channel_id.as_str())
                    .and_then(|ch| serde_json::from_value::<FeishuConfig>(ch.config.clone()).ok())
            }
            None => None,
        }?;

        let client = Arc::new(crate::gateway::interfaces::feishu::client::FeishuClient::new(&feishu_cfg));
        // Acquire initial token
        if let Err(e) = client.refresh_token().await {
            warn!("Failed to create feishu emitter client: {e}");
            return None;
        }

        let inner = ReplyEmitter::with_config(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
            run_id.to_string(),
            reply_config,
        );

        Some(FeishuEventEmitter::new(
            inner,
            client,
            ctx.reply_route.clone(),
            ctx.message.conversation_id.as_str().to_string(),
            ctx.reply_route.reply_to.as_ref().map(|id| id.as_str().to_string()),
            feishu_cfg.streaming,
            feishu_cfg.typing_indicator,
        ))
```

**Note**: The above is a sketch. The implementer must check:
1. The exact struct name for the `impl` block in executor.rs
2. The exact field names in the channels config struct (`id`, `config`, etc.)
3. Whether `ReplyRoute.reply_to` exists or if it's only in `InboundMessage`
4. Import paths for all referenced types

- [ ] **Step 4: Add necessary imports to executor.rs**

```rust
use crate::gateway::event_emitter::EventEmitter;
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles. This step will likely require several iterations to fix imports, field access patterns, and type mismatches.

- [ ] **Step 6: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: All existing tests pass, no regressions

- [ ] **Step 7: Commit**

```bash
git add core/src/gateway/inbound_router/executor.rs core/src/gateway/channel_registry.rs
git commit -m "feat(feishu): wire FeishuEventEmitter into execution flow"
```

---

### Task 6: Update Panel UI Fields

**Files:**
- Modify: `interfaces/webchat/src/views/settings/channels/definitions.rs`

- [ ] **Step 1: Add new fields to FEISHU_FIELDS**

Add these three fields to the `FEISHU_FIELDS` array (after `require_mention`):

```rust
    FieldDef {
        key: "streaming",
        label: "Streaming Output",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Stream responses in real-time via Feishu cards",
        required: false,
        default_value: "true",
        options: &[],
    },
    FieldDef {
        key: "render_mode",
        label: "Render Mode",
        kind: FieldKind::Text,
        placeholder: "auto",
        help: "\"auto\" (card for rich content), \"card\" (always card), \"raw\" (plain text)",
        required: false,
        default_value: "auto",
        options: &[],
    },
    FieldDef {
        key: "typing_indicator",
        label: "Typing Indicator",
        kind: FieldKind::Toggle,
        placeholder: "",
        help: "Show typing emoji on user message while processing",
        required: false,
        default_value: "true",
        options: &[],
    },
```

- [ ] **Step 2: Verify panel compilation**

Run: `cargo check -p aleph-panel`
Expected: Compiles

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/settings/channels/definitions.rs
git commit -m "feat(panel): add streaming, render_mode, typing_indicator fields to Feishu settings"
```

---

### Task 7: Final Verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -W clippy::all 2>&1 | grep feishu`
Expected: No feishu-specific warnings

- [ ] **Step 3: Fix any clippy warnings**

Address and re-run until clean.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(feishu): final cleanup for enhanced features"
```

---

## Verification Checklist

After all tasks:

- [ ] `cargo check -p alephcore` passes
- [ ] `cargo test -p alephcore --lib feishu` passes (all existing + new tests)
- [ ] `cargo clippy -p alephcore` clean for feishu module
- [ ] `cargo check -p aleph-panel` passes
- [ ] New config fields deserialize with correct defaults
- [ ] `should_use_card()` tests cover: plain text, code blocks, tables, long text, forced modes
- [ ] `FeishuEventEmitter` compiles with correct `EventEmitter` trait impl
- [ ] `streaming.rs` created with `FeishuStreamingCard` + `FeishuEventEmitter`
- [ ] Panel UI has streaming, render_mode, typing_indicator fields
