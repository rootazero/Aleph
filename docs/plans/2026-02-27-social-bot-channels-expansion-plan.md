# Social Bot Channels Expansion — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Expand Aleph's social bot capabilities from 5 channels to 15 by integrating OpenFang's channel implementations, adapted to Aleph's Channel trait architecture.

**Architecture:** Each new channel implements Aleph's existing `Channel` trait + `ChannelFactory`, following the exact patterns established by Telegram/Discord. A shared `MessageFormatter` provides cross-platform markup conversion. A `WebhookReceiver` provides shared HTTP server for webhook-based channels.

**Tech Stack:** Rust + Tokio, axum (webhook server), tokio-tungstenite (WebSocket channels), zeroize (secret safety), wiremock (testing)

**Reference Codebases:**
- Aleph existing channels: `core/src/gateway/interfaces/{telegram,discord,imessage,whatsapp}/`
- Aleph channel abstraction: `core/src/gateway/channel.rs` (Channel trait, ChannelFactory, ChannelCapabilities, etc.)
- OpenFang channels: `~/Workspace/openfang/crates/openfang-channels/src/`

---

## Implementation Order

```
Phase 0: Infrastructure (Tasks 1-3)
  ├── Task 1: MessageFormatter
  ├── Task 2: WebhookReceiver
  └── Task 3: Secret Zeroization

Phase 1: HTTP/REST Channels (Tasks 4-6)
  ├── Task 4: Slack
  ├── Task 5: Email
  └── Task 6: Matrix

Phase 2: WebSocket/Streaming (Tasks 7-9)
  ├── Task 7: Signal
  ├── Task 8: Mattermost
  └── Task 9: IRC

Phase 3: Webhook Channels (Tasks 10-11)
  ├── Task 10: WhatsApp (complete stub)
  └── Task 11: Generic Webhook

Phase 4: Supplemental (Tasks 12-13)
  ├── Task 12: XMPP
  └── Task 13: Nostr
```

---

## Standard Channel File Structure

Every new channel follows this 3-file pattern (matching Telegram/Discord/iMessage):

```
core/src/gateway/interfaces/{channel_name}/
├── mod.rs           # Channel trait impl + ChannelFactory impl
├── config.rs        # {Channel}Config struct with serde + validate()
└── message_ops.rs   # Platform API calls, message conversion, media handling
```

**Standard mod.rs skeleton:**

```rust
use async_trait::async_trait;
use tokio::sync::{mpsc, watch, RwLock};
use std::sync::Arc;
use tracing::{info, warn, debug};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId,
    ChannelInfo, ChannelResult, ChannelStatus, InboundMessage, OutboundMessage,
    PairingData, SendResult,
};

mod config;
mod message_ops;

pub use config::*;

pub struct {Name}Channel {
    info: ChannelInfo,
    config: {Name}Config,
    inbound_tx: mpsc::Sender<InboundMessage>,
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    shutdown_tx: Option<watch::Sender<bool>>,
    status: Arc<RwLock<ChannelStatus>>,
}

impl {Name}Channel {
    pub fn new(id: impl Into<String>, config: {Name}Config) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(100);
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "{Name}".to_string(),
            channel_type: "{name}".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };
        Self {
            info, config, inbound_tx,
            inbound_rx: Some(inbound_rx),
            shutdown_tx: None,
            status: Arc::new(RwLock::new(ChannelStatus::Disconnected)),
        }
    }

    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities { /* per-platform values */ }
    }
}

#[async_trait]
impl Channel for {Name}Channel {
    fn info(&self) -> &ChannelInfo { &self.info }
    fn status(&self) -> ChannelStatus { self.info.status }

    async fn start(&mut self) -> ChannelResult<()> {
        // Platform-specific startup
    }
    async fn stop(&mut self) -> ChannelResult<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        self.info.status = ChannelStatus::Disconnected;
        Ok(())
    }
    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        // Platform-specific send
    }
    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        None // Taken once in start()
    }
}

pub struct {Name}ChannelFactory;

#[async_trait]
impl ChannelFactory for {Name}ChannelFactory {
    fn channel_type(&self) -> &str { "{name}" }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: {Name}Config = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid {Name} config: {}", e)))?;
        config.validate().map_err(ChannelError::ConfigError)?;
        Ok(Box::new({Name}Channel::new("{name}", config)))
    }
}
```

**Standard config.rs skeleton:**

```rust
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct {Name}Config {
    pub token: String,  // Will be wrapped in Zeroizing at runtime
    #[serde(default)]
    pub allowed_users: Vec<String>,
    #[serde(default = "default_true")]
    pub send_typing: bool,
}

impl Default for {Name}Config {
    fn default() -> Self { /* ... */ }
}

impl {Name}Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.token.is_empty() {
            return Err("token is required".to_string());
        }
        Ok(())
    }
}
```

---

## Phase 0: Infrastructure

### Task 1: MessageFormatter

**Files:**
- Create: `core/src/gateway/formatter.rs`
- Modify: `core/src/gateway/mod.rs` (add `pub mod formatter;`)

**Reference:** `~/Workspace/openfang/crates/openfang-channels/src/formatter.rs`

**Step 1: Write failing tests for Markdown→TelegramHtml conversion**

```rust
// core/src/gateway/formatter.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bold_to_telegram_html() {
        assert_eq!(
            MessageFormatter::format("**hello**", MarkupFormat::TelegramHtml),
            "<b>hello</b>"
        );
    }

    #[test]
    fn test_italic_to_telegram_html() {
        assert_eq!(
            MessageFormatter::format("*hello*", MarkupFormat::TelegramHtml),
            "<i>hello</i>"
        );
    }

    #[test]
    fn test_code_to_telegram_html() {
        assert_eq!(
            MessageFormatter::format("`code`", MarkupFormat::TelegramHtml),
            "<code>code</code>"
        );
    }

    #[test]
    fn test_link_to_telegram_html() {
        assert_eq!(
            MessageFormatter::format("[text](https://example.com)", MarkupFormat::TelegramHtml),
            "<a href=\"https://example.com\">text</a>"
        );
    }

    #[test]
    fn test_code_block_to_telegram_html() {
        assert_eq!(
            MessageFormatter::format("```rust\nlet x = 1;\n```", MarkupFormat::TelegramHtml),
            "<pre><code class=\"language-rust\">let x = 1;\n</code></pre>"
        );
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cd core && cargo test --features gateway formatter::tests -- --nocapture`
Expected: FAIL — module `formatter` not found

**Step 3: Implement MarkupFormat enum and MessageFormatter::format for TelegramHtml**

Reference OpenFang's `markdown_to_telegram_html()` from `formatter.rs` for the state-machine approach. Adapt to Aleph's style (no external markdown parser — keep it simple, character-by-character processing).

```rust
// core/src/gateway/formatter.rs

/// Target markup format for message formatting
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkupFormat {
    /// Standard Markdown (passthrough for Matrix, Discourse)
    Markdown,
    /// Telegram HTML: <b>, <i>, <code>, <a>
    TelegramHtml,
    /// Slack mrkdwn: *bold*, _italic_, `code`, <url|text>
    SlackMrkdwn,
    /// Discord Markdown: **bold**, *italic*, `code` (near-standard)
    DiscordMarkdown,
    /// IRC formatting: \x02 bold, \x1D italic, \x03 color
    IrcFormatting,
    /// Plain text: all markup stripped
    PlainText,
}

pub struct MessageFormatter;

impl MessageFormatter {
    /// Convert standard Markdown to platform-specific markup
    pub fn format(markdown: &str, target: MarkupFormat) -> String {
        match target {
            MarkupFormat::Markdown => markdown.to_string(),
            MarkupFormat::TelegramHtml => Self::markdown_to_telegram_html(markdown),
            MarkupFormat::SlackMrkdwn => Self::markdown_to_slack_mrkdwn(markdown),
            MarkupFormat::DiscordMarkdown => markdown.to_string(), // near-identical
            MarkupFormat::IrcFormatting => Self::markdown_to_irc(markdown),
            MarkupFormat::PlainText => Self::markdown_to_plain(markdown),
        }
    }

    /// Smart message splitting that respects paragraph and code block boundaries.
    /// Never splits in the middle of a code block.
    pub fn split(text: &str, max_len: usize) -> Vec<String> {
        if text.len() <= max_len {
            return vec![text.to_string()];
        }
        // Split at paragraph boundaries (\n\n) first, then at line boundaries (\n)
        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut in_code_block = false;

        for line in text.lines() {
            if line.starts_with("```") {
                in_code_block = !in_code_block;
            }

            let would_be = if current.is_empty() {
                line.len()
            } else {
                current.len() + 1 + line.len() // +1 for newline
            };

            if would_be > max_len && !current.is_empty() && !in_code_block {
                chunks.push(current.clone());
                current.clear();
            }

            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        chunks
    }

    /// Normalize platform markup to standard Markdown (inbound direction)
    pub fn normalize(platform_text: &str, source: MarkupFormat) -> String {
        match source {
            MarkupFormat::Markdown | MarkupFormat::DiscordMarkdown => {
                platform_text.to_string()
            }
            MarkupFormat::TelegramHtml => Self::telegram_html_to_markdown(platform_text),
            MarkupFormat::SlackMrkdwn => Self::slack_mrkdwn_to_markdown(platform_text),
            MarkupFormat::PlainText | MarkupFormat::IrcFormatting => {
                platform_text.to_string()
            }
        }
    }

    // --- Private conversion methods ---
    // Port from OpenFang's formatter.rs, adapting the state-machine approach.
    // Each method handles: bold, italic, code, code blocks, links.

    fn markdown_to_telegram_html(text: &str) -> String { /* ... */ }
    fn markdown_to_slack_mrkdwn(text: &str) -> String { /* ... */ }
    fn markdown_to_irc(text: &str) -> String { /* ... */ }
    fn markdown_to_plain(text: &str) -> String { /* ... */ }
    fn telegram_html_to_markdown(text: &str) -> String { /* ... */ }
    fn slack_mrkdwn_to_markdown(text: &str) -> String { /* ... */ }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd core && cargo test --features gateway formatter::tests -- --nocapture`
Expected: PASS

**Step 5: Add tests for SlackMrkdwn, PlainText, IRC, and split()**

```rust
#[test]
fn test_bold_to_slack() {
    assert_eq!(
        MessageFormatter::format("**hello**", MarkupFormat::SlackMrkdwn),
        "*hello*"
    );
}

#[test]
fn test_link_to_slack() {
    assert_eq!(
        MessageFormatter::format("[text](https://example.com)", MarkupFormat::SlackMrkdwn),
        "<https://example.com|text>"
    );
}

#[test]
fn test_strip_to_plain() {
    assert_eq!(
        MessageFormatter::format("**bold** and *italic*", MarkupFormat::PlainText),
        "bold and italic"
    );
}

#[test]
fn test_split_respects_code_blocks() {
    let text = "intro\n\n```\nlong code\nblock\nhere\n```\n\nconclusion";
    let chunks = MessageFormatter::split(text, 30);
    // Code block should not be split
    assert!(chunks.iter().any(|c| c.contains("```\nlong code")));
}

#[test]
fn test_split_short_message() {
    let chunks = MessageFormatter::split("short", 100);
    assert_eq!(chunks, vec!["short"]);
}
```

**Step 6: Implement remaining conversion methods and pass all tests**

Run: `cd core && cargo test --features gateway formatter -- --nocapture`
Expected: ALL PASS

**Step 7: Register module in gateway/mod.rs**

Add `pub mod formatter;` to `core/src/gateway/mod.rs` inside the `#[cfg(feature = "gateway")]` block, alongside the existing module declarations.

**Step 8: Verify full build**

Run: `cd core && cargo build --features gateway`
Expected: BUILD SUCCESS

**Step 9: Commit**

```bash
git add core/src/gateway/formatter.rs core/src/gateway/mod.rs
git commit -m "gateway: add unified MessageFormatter for cross-platform markup conversion"
```

---

### Task 2: WebhookReceiver

**Files:**
- Create: `core/src/gateway/webhook_receiver.rs`
- Modify: `core/src/gateway/mod.rs` (add `pub mod webhook_receiver;`)

**Reference:** `~/Workspace/openfang/crates/openfang-channels/src/webhook.rs` (axum server pattern)

**Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use bytes::Bytes;

    #[test]
    fn test_hmac_signature_verification() {
        let secret = "test-secret";
        let body = b"hello world";
        let sig = WebhookReceiver::compute_signature(secret, body);
        assert!(WebhookReceiver::verify_signature(secret, body, &sig));
    }

    #[test]
    fn test_hmac_signature_rejects_invalid() {
        let secret = "test-secret";
        let body = b"hello world";
        assert!(!WebhookReceiver::verify_signature(secret, body, "sha256=invalid"));
    }

    #[test]
    fn test_hmac_constant_time_comparison() {
        let secret = "test-secret";
        let body = b"hello world";
        let sig = WebhookReceiver::compute_signature(secret, body);
        // Wrong last char
        let mut bad_sig = sig.clone();
        bad_sig.pop();
        bad_sig.push('x');
        assert!(!WebhookReceiver::verify_signature(secret, body, &bad_sig));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cd core && cargo test --features gateway webhook_receiver::tests -- --nocapture`
Expected: FAIL

**Step 3: Implement WebhookReceiver and WebhookHandler trait**

```rust
// core/src/gateway/webhook_receiver.rs

use async_trait::async_trait;
use axum::{body::Bytes, http::HeaderMap, Router};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use crate::gateway::channel::{ChannelResult, InboundMessage};

/// Trait for individual webhook channel handlers
#[async_trait]
pub trait WebhookHandler: Send + Sync {
    /// Verify webhook signature (HMAC-SHA256, platform-specific headers, etc.)
    fn verify(&self, headers: &HeaderMap, body: &[u8]) -> bool;

    /// Parse webhook payload into InboundMessages
    async fn handle(&self, headers: &HeaderMap, body: Bytes) -> ChannelResult<Vec<InboundMessage>>;

    /// URL path for this handler (e.g., "/webhook/whatsapp")
    fn path(&self) -> &str;
}

/// Shared HTTP server for all webhook-based channels.
/// Each channel registers a WebhookHandler at a unique path.
pub struct WebhookReceiver {
    port: u16,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl WebhookReceiver {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            shutdown_tx: None,
        }
    }

    /// Start the shared webhook HTTP server with registered handlers.
    pub async fn start(
        &mut self,
        handlers: Vec<Arc<dyn WebhookHandler>>,
        inbound_tx: mpsc::Sender<InboundMessage>,
    ) -> ChannelResult<()> {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let mut router = Router::new();
        for handler in handlers {
            let path = handler.path().to_string();
            let handler = Arc::clone(&handler);
            let tx = inbound_tx.clone();
            router = router.route(
                &path,
                axum::routing::post(move |headers: HeaderMap, body: Bytes| {
                    let handler = Arc::clone(&handler);
                    let tx = tx.clone();
                    async move {
                        if !handler.verify(&headers, &body) {
                            warn!("Webhook: invalid signature at {}", handler.path());
                            return (axum::http::StatusCode::FORBIDDEN, "Forbidden");
                        }
                        match handler.handle(&headers, body).await {
                            Ok(messages) => {
                                for msg in messages {
                                    let _ = tx.send(msg).await;
                                }
                                (axum::http::StatusCode::OK, "ok")
                            }
                            Err(e) => {
                                warn!("Webhook handler error: {}", e);
                                (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "error")
                            }
                        }
                    }
                }),
            );
        }

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], self.port));
        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
            crate::gateway::channel::ChannelError::Internal(format!(
                "Failed to bind webhook port {}: {}",
                self.port, e
            ))
        })?;

        info!("Webhook receiver listening on port {}", self.port);

        let mut shutdown_rx = shutdown_rx;
        tokio::spawn(async move {
            let server = axum::serve(listener, router);
            tokio::select! {
                result = server => {
                    if let Err(e) = result {
                        warn!("Webhook server error: {}", e);
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("Webhook receiver shutting down");
                }
            }
        });

        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
    }

    /// Compute HMAC-SHA256 signature for webhook verification
    pub fn compute_signature(secret: &str, data: &[u8]) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
        mac.update(data);
        let result = mac.finalize();
        format!("sha256={}", hex::encode(result.into_bytes()))
    }

    /// Verify HMAC-SHA256 signature with constant-time comparison
    pub fn verify_signature(secret: &str, body: &[u8], signature: &str) -> bool {
        let expected = Self::compute_signature(secret, body);
        if expected.len() != signature.len() {
            return false;
        }
        let mut diff = 0u8;
        for (a, b) in expected.bytes().zip(signature.bytes()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd core && cargo test --features gateway webhook_receiver::tests -- --nocapture`
Expected: PASS

**Step 5: Add dependencies to Cargo.toml if needed**

Check if `hmac`, `sha2`, `hex` are already in dependencies. If not:

```toml
hmac = "0.12"
sha2 = "0.10"
hex = "0.4"
```

**Step 6: Register module in gateway/mod.rs**

Add `pub mod webhook_receiver;` inside the `#[cfg(feature = "gateway")]` block.

**Step 7: Verify full build**

Run: `cd core && cargo build --features gateway`
Expected: BUILD SUCCESS

**Step 8: Commit**

```bash
git add core/src/gateway/webhook_receiver.rs core/src/gateway/mod.rs core/Cargo.toml
git commit -m "gateway: add WebhookReceiver with HMAC-SHA256 signature verification"
```

---

### Task 3: Secret Zeroization

**Files:**
- Modify: `core/Cargo.toml` (add `zeroize` dependency)
- Modify: `core/src/gateway/interfaces/telegram/config.rs`
- Modify: `core/src/gateway/interfaces/discord/config.rs`
- Modify: `core/src/gateway/interfaces/whatsapp/config.rs`

**Step 1: Add zeroize dependency**

Add to `core/Cargo.toml`:
```toml
zeroize = { version = "1", features = ["derive"] }
```

**Step 2: Update TelegramConfig**

In `core/src/gateway/interfaces/telegram/config.rs`, note that `bot_token` is currently `String`. For now, just add `use zeroize::Zeroizing;` and document that new channels should use `Zeroizing<String>`. Do NOT change existing config structs to avoid breaking serialization — mark this as a future migration.

Add a comment at top of file:
```rust
// NOTE: For new channels, use Zeroizing<String> for sensitive fields (tokens, passwords).
// Existing channels will be migrated in a separate task to avoid breaking config serialization.
```

**Step 3: Verify build**

Run: `cd core && cargo build --features gateway,telegram,discord,whatsapp`
Expected: BUILD SUCCESS

**Step 4: Commit**

```bash
git add core/Cargo.toml core/src/gateway/interfaces/telegram/config.rs
git commit -m "gateway: add zeroize dependency for secret safety in new channels"
```

---

## Phase 1: HTTP/REST Channels

### Task 4: Slack Channel

**Files:**
- Create: `core/src/gateway/interfaces/slack/mod.rs`
- Create: `core/src/gateway/interfaces/slack/config.rs`
- Create: `core/src/gateway/interfaces/slack/message_ops.rs`
- Modify: `core/src/gateway/interfaces/mod.rs` (add `#[cfg(feature = "slack")] pub mod slack;`)
- Modify: `core/Cargo.toml` (add `slack` feature flag)

**Reference:** `~/Workspace/openfang/crates/openfang-channels/src/slack.rs`

**Protocol:** Slack Socket Mode (WebSocket for receiving) + REST API (for sending).

**Step 1: Add feature flag and register module**

In `core/Cargo.toml`, add:
```toml
slack = ["gateway"]
```

Update `all-channels` to include `"slack"`.

In `core/src/gateway/interfaces/mod.rs`, add:
```rust
#[cfg(feature = "slack")]
pub mod slack;
#[cfg(feature = "slack")]
pub use slack::{SlackChannel, SlackChannelFactory, SlackConfig};
```

**Step 2: Write SlackConfig**

File: `core/src/gateway/interfaces/slack/config.rs`

```rust
use serde::{Deserialize, Serialize};

fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackConfig {
    /// Socket Mode app-level token (xapp-...)
    pub app_token: String,
    /// Bot token (xoxb-...)
    pub bot_token: String,
    /// Only respond in these channel IDs (empty = all)
    #[serde(default)]
    pub allowed_channels: Vec<String>,
    /// Send typing indicators
    #[serde(default = "default_true")]
    pub send_typing: bool,
    /// Allow DMs
    #[serde(default = "default_true")]
    pub dm_allowed: bool,
}

impl Default for SlackConfig {
    fn default() -> Self {
        Self {
            app_token: String::new(),
            bot_token: String::new(),
            allowed_channels: Vec::new(),
            send_typing: true,
            dm_allowed: true,
        }
    }
}

impl SlackConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.app_token.is_empty() {
            return Err("app_token is required (xapp-...)".to_string());
        }
        if !self.app_token.starts_with("xapp-") {
            return Err("app_token must start with 'xapp-'".to_string());
        }
        if self.bot_token.is_empty() {
            return Err("bot_token is required (xoxb-...)".to_string());
        }
        if !self.bot_token.starts_with("xoxb-") {
            return Err("bot_token must start with 'xoxb-'".to_string());
        }
        Ok(())
    }

    pub fn is_channel_allowed(&self, channel_id: &str) -> bool {
        if self.allowed_channels.is_empty() { return true; }
        self.allowed_channels.iter().any(|c| c == channel_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validate_empty_tokens() {
        let config = SlackConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validate_valid() {
        let config = SlackConfig {
            app_token: "xapp-test".to_string(),
            bot_token: "xoxb-test".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = SlackConfig {
            app_token: "xapp-test".to_string(),
            bot_token: "xoxb-test".to_string(),
            allowed_channels: vec!["C123".to_string()],
            send_typing: false,
            dm_allowed: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SlackConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.app_token, "xapp-test");
        assert_eq!(parsed.allowed_channels, vec!["C123"]);
    }
}
```

**Step 3: Write SlackChannel mod.rs (struct, Channel trait impl, Factory)**

File: `core/src/gateway/interfaces/slack/mod.rs`

Follow the standard skeleton above. Key Slack-specific details:

- **Capabilities:** attachments=true, images=true, audio=false, video=false, reactions=true, replies=true (threads), editing=true, deletion=true, typing=true, read_receipts=false, rich_text=true, max_message_length=3000, max_attachment_size=1GB
- **start():** Validate bot token via `auth.test`, get bot user ID, spawn Socket Mode WebSocket loop (from OpenFang pattern: get URL via `apps.connections.open`, connect, process `events_api` envelopes, acknowledge with `envelope_id`)
- **send():** POST to `chat.postMessage` with `channel` and `text` fields. Use `MessageFormatter::format(text, MarkupFormat::SlackMrkdwn)` for outbound formatting.
- **Shutdown:** `watch::channel(false)` pattern

**Step 4: Write message_ops.rs (Slack API calls, message conversion)**

File: `core/src/gateway/interfaces/slack/message_ops.rs`

Key functions (reference OpenFang's slack.rs):
- `get_socket_mode_url(client, app_token) -> Result<String>` — POST to `apps.connections.open`
- `send_message(client, bot_token, channel, text) -> Result<SendResult>` — POST to `chat.postMessage`
- `convert_slack_event(event_json) -> Option<InboundMessage>` — Parse Slack event envelope into InboundMessage
- `start_socket_mode_loop(...)` — WebSocket loop with backoff and envelope ACK

**Step 5: Write integration tests with wiremock**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_slack_config_validation() {
        let config = SlackConfig::default();
        assert!(config.validate().is_err());
    }

    // Additional tests using wiremock for auth.test, chat.postMessage, etc.
}
```

**Step 6: Verify build and tests**

Run: `cd core && cargo build --features slack && cargo test --features slack slack -- --nocapture`
Expected: BUILD SUCCESS, TESTS PASS

**Step 7: Commit**

```bash
git add core/src/gateway/interfaces/slack/ core/src/gateway/interfaces/mod.rs core/Cargo.toml
git commit -m "gateway: add Slack channel (Socket Mode + REST API)"
```

---

### Task 5: Email Channel

**Files:**
- Create: `core/src/gateway/interfaces/email/mod.rs`
- Create: `core/src/gateway/interfaces/email/config.rs`
- Create: `core/src/gateway/interfaces/email/message_ops.rs`
- Modify: `core/src/gateway/interfaces/mod.rs`
- Modify: `core/Cargo.toml`

**Reference:** `~/Workspace/openfang/crates/openfang-channels/src/email.rs`

**Protocol:** IMAP (receive) + SMTP (send). Dependencies: `lettre` (SMTP), `imap` (IMAP client).

**Step 1: Add feature flag and dependencies**

```toml
email = ["gateway", "dep:lettre", "dep:imap", "dep:native-tls"]

[dependencies]
lettre = { version = "0.11", features = ["tokio1-rustls-tls"], optional = true }
imap = { version = "3", optional = true }
native-tls = { version = "0.2", optional = true }
```

**Step 2: Write EmailConfig**

Key fields: `imap_host`, `imap_port`, `smtp_host`, `smtp_port`, `username`, `password`, `poll_interval_secs`, `folders` (default `["INBOX"]`), `allowed_senders`, `from_address`.

Reference OpenFang's email adapter for field design.

**Step 3: Write EmailChannel**

- **Capabilities:** attachments=true, images=true (as attachments), audio=false, video=false, reactions=false, replies=true (via subject Re:), editing=false, deletion=false, typing=false, read_receipts=false, rich_text=true (HTML email), max_message_length=1MB, max_attachment_size=25MB
- **start():** Spawn polling loop that connects to IMAP, polls folders, extracts new messages
- **send():** Connect to SMTP, send email with `lettre`. Use `MessageFormatter::format(text, MarkupFormat::Markdown)` wrapped in HTML body
- **Agent routing:** Extract from email subject `[agent-name]` prefix (from OpenFang pattern)

**Step 4: Write message_ops.rs**

Key functions:
- `poll_imap(host, port, user, pass, folders) -> Vec<InboundMessage>` — Connect, SEARCH UNSEEN, FETCH, parse
- `send_smtp(host, port, user, pass, to, subject, body) -> SendResult` — Build and send email
- `parse_email_body(raw) -> (String, Vec<Attachment>)` — Extract text and attachments from MIME

**Step 5: Tests and commit**

Same pattern as Task 4.

```bash
git commit -m "gateway: add Email channel (IMAP + SMTP)"
```

---

### Task 6: Matrix Channel

**Files:**
- Create: `core/src/gateway/interfaces/matrix/mod.rs`
- Create: `core/src/gateway/interfaces/matrix/config.rs`
- Create: `core/src/gateway/interfaces/matrix/message_ops.rs`
- Modify: `core/src/gateway/interfaces/mod.rs`
- Modify: `core/Cargo.toml`

**Reference:** `~/Workspace/openfang/crates/openfang-channels/src/matrix.rs`

**Protocol:** Matrix Client-Server API v3. Uses HTTP REST `/sync` long-polling. No external crate needed — just `reqwest`.

**Step 1: Add feature flag**

```toml
matrix = ["gateway"]
```

No new dependencies — uses existing `reqwest`.

**Step 2: Write MatrixConfig**

Key fields: `homeserver_url`, `access_token`, `allowed_rooms` (room IDs), `sync_timeout_ms` (default 30000).

Reference OpenFang's matrix adapter.

**Step 3: Write MatrixChannel**

- **Capabilities:** attachments=true, images=true, audio=true, video=true, reactions=true, replies=true, editing=true, deletion=false, typing=true, read_receipts=true, rich_text=true (Markdown), max_message_length=65535, max_attachment_size=100MB
- **start():** Validate token via `/_matrix/client/v3/whoami`. Spawn sync loop: GET `/_matrix/client/v3/sync?timeout=30000&since={token}`. Track `since_token` in `Arc<RwLock<Option<String>>>`. Process `rooms.join.*.timeline.events` for `m.room.message` events. Skip own messages.
- **send():** PUT `/_matrix/client/v3/rooms/{room}/send/m.room.message/{txn}`. Body: `{"msgtype":"m.text","body":"...","format":"org.matrix.custom.html","formatted_body":"..."}`

**Step 4: Write message_ops.rs**

Reference OpenFang's matrix adapter for:
- `/sync` response parsing
- Room event filtering
- Transaction ID generation (`Uuid::new_v4`)
- Typing indicator: PUT `/_matrix/client/v3/rooms/{room}/typing/{user}`

**Step 5: Tests and commit**

```bash
git commit -m "gateway: add Matrix channel (Client-Server API v3)"
```

---

## Phase 2: WebSocket/Streaming Channels

### Task 7: Signal Channel

**Files:** Standard 3-file structure in `core/src/gateway/interfaces/signal/`

**Reference:** `~/Workspace/openfang/crates/openfang-channels/src/signal.rs`

**Protocol:** REST API wrapping `signal-cli` daemon. No external crate needed — just `reqwest`.

**Step 1: Add feature flag**

```toml
signal = ["gateway"]
```

**Step 2: Implement**

- **Config:** `api_url` (signal-cli REST endpoint, default `http://localhost:8080`), `phone_number`, `allowed_users`
- **Capabilities:** attachments=true, images=true, audio=true, video=true, reactions=true, replies=true, editing=false, deletion=false, typing=true, read_receipts=true, rich_text=false (plain text only), max_message_length=65535, max_attachment_size=100MB
- **start():** Poll `GET /v1/receive/{phone}` in a loop with backoff
- **send():** POST to `/v2/send` with `{"number":"+1...", "message":"..."}`

**Step 3: Tests and commit**

```bash
git commit -m "gateway: add Signal channel (signal-cli REST API)"
```

---

### Task 8: Mattermost Channel

**Files:** Standard 3-file structure in `core/src/gateway/interfaces/mattermost/`

**Reference:** `~/Workspace/openfang/crates/openfang-channels/src/mattermost.rs` (if exists, else adapt from Slack pattern)

**Protocol:** WebSocket (receive) + REST API (send). Similar to Slack but simpler.

**Step 1: Add feature flag**

```toml
mattermost = ["gateway"]
```

**Step 2: Implement**

- **Config:** `server_url`, `bot_token`, `allowed_channels`, `send_typing`
- **Capabilities:** Similar to Slack but max_message_length=16383
- **start():** Get WebSocket URL via `GET /api/v4/websocket`. Connect and process `posted` events.
- **send():** POST to `/api/v4/posts` with `{"channel_id":"...","message":"..."}`
- **Reconnection:** Exponential backoff (from OpenFang pattern)

**Step 3: Tests and commit**

```bash
git commit -m "gateway: add Mattermost channel (WebSocket + REST API)"
```

---

### Task 9: IRC Channel

**Files:** Standard 3-file structure in `core/src/gateway/interfaces/irc/`

**Reference:** `~/Workspace/openfang/crates/openfang-channels/src/irc.rs`

**Protocol:** Raw TCP (RFC 2812). No external crate — use `tokio::net::TcpStream` + `tokio::io::BufReader`.

**Step 1: Add feature flag**

```toml
irc = ["gateway"]
```

**Step 2: Implement**

- **Config:** `server`, `port` (default 6667), `nick`, `password` (optional NickServ), `channels` (list of #channels to join), `use_tls` (default false)
- **Capabilities:** attachments=false, images=false, audio=false, video=false, reactions=false, replies=false, editing=false, deletion=false, typing=false, read_receipts=false, rich_text=false, max_message_length=400 (conservative PRIVMSG), max_attachment_size=0
- **start():** Connect TCP, send NICK/USER, handle 001 (registration complete) → JOIN channels, process PRIVMSG events, respond to PING with PONG. Use `MessageFormatter::format(text, MarkupFormat::IrcFormatting)` for outbound.
- **send():** `PRIVMSG {target} :{text}\r\n`. Use `MessageFormatter::split()` for messages > 400 chars.
- **IRC line parser:** Port from OpenFang's `parse_irc_line()` — handles prefix, command, params, trailing.

**Step 3: Tests and commit**

```bash
git commit -m "gateway: add IRC channel (RFC 2812 raw TCP)"
```

---

## Phase 3: Webhook Channels

### Task 10: WhatsApp Channel (Complete Stub)

**Files:**
- Modify: `core/src/gateway/interfaces/whatsapp/mod.rs`
- Modify: `core/src/gateway/interfaces/whatsapp/config.rs`
- Modify: `core/src/gateway/interfaces/whatsapp/message_ops.rs`

**Reference:** `~/Workspace/openfang/crates/openfang-channels/src/whatsapp.rs` + existing Aleph WhatsApp stub

**Note:** Aleph already has a WhatsApp channel with `BridgeManager` pattern. This task completes the implementation, filling in the stub methods. Review the existing code thoroughly before modifying — the bridge-based architecture should be preserved and enhanced, not replaced.

**Step 1: Read existing WhatsApp implementation thoroughly**

Understand: BridgeManager, BridgeRpcClient, PairingState, bridge_protocol.rs.

**Step 2: Complete message_ops.rs**

Fill in any stub methods for message sending, media handling, and status tracking.

**Step 3: Add webhook verification**

If the WhatsApp implementation uses webhooks for inbound (instead of bridge), implement `WebhookHandler` trait. Otherwise, ensure the BridgeManager event loop correctly processes all `BridgeEvent` variants.

**Step 4: Tests and commit**

```bash
git commit -m "gateway: complete WhatsApp channel implementation"
```

---

### Task 11: Generic Webhook Channel

**Files:** Standard 3-file structure in `core/src/gateway/interfaces/webhook/`

**Reference:** `~/Workspace/openfang/crates/openfang-channels/src/webhook.rs`

**Protocol:** Generic bidirectional HTTP. Implements `WebhookHandler` trait from Task 2.

**Step 1: Add feature flag**

```toml
webhook = ["gateway"]
```

**Step 2: Implement**

- **Config:** `port` (webhook receive port), `secret` (HMAC-SHA256 secret), `callback_url` (outbound POST URL), `path` (default `/webhook/generic`)
- **Capabilities:** attachments=false, images=false, audio=false, video=false, reactions=false, replies=false, editing=false, deletion=false, typing=false, read_receipts=false, rich_text=true (JSON body), max_message_length=1MB, max_attachment_size=0
- **Inbound:** Implement `WebhookHandler` trait. Expect JSON body: `{"sender_id":"...","sender_name":"...","message":"...","thread_id":"...","is_group":false,"metadata":{}}`
- **Outbound:** POST to `callback_url` with same JSON format + `X-Webhook-Signature` header
- **This is the "universal adapter"** — any system that can POST JSON and receive POST JSON can integrate with Aleph

**Step 3: Tests and commit**

```bash
git commit -m "gateway: add generic Webhook channel (bidirectional HTTP + HMAC)"
```

---

## Phase 4: Supplemental Channels

### Task 12: XMPP Channel

**Files:** Standard 3-file structure in `core/src/gateway/interfaces/xmpp/`

**Protocol:** XMPP (TCP). Dependency: `xmpp-parsers` or `tokio-xmpp`.

**Step 1: Add feature flag and dependency**

```toml
xmpp = ["gateway", "dep:tokio-xmpp", "dep:xmpp-parsers"]

[dependencies]
tokio-xmpp = { version = "4", optional = true }
xmpp-parsers = { version = "0.21", optional = true }
```

**Step 2: Implement**

- **Config:** `jid` (XMPP address), `password`, `server` (optional, derived from JID), `muc_rooms` (MUC group chat rooms to join)
- **Capabilities:** attachments=false, images=false, audio=false, video=false, reactions=false, replies=false, editing=false, deletion=false, typing=true, read_receipts=true, rich_text=false, max_message_length=65535, max_attachment_size=0
- **start():** Connect, authenticate, send presence, join MUC rooms. Process incoming message stanzas.
- **send():** Send message stanza to JID or MUC room

**Step 3: Tests and commit**

```bash
git commit -m "gateway: add XMPP channel (MUC group chat support)"
```

---

### Task 13: Nostr Channel

**Files:** Standard 3-file structure in `core/src/gateway/interfaces/nostr/`

**Protocol:** Nostr NIP-01 (WebSocket to relays). Dependency: `nostr-sdk`.

**Step 1: Add feature flag and dependency**

```toml
nostr = ["gateway", "dep:nostr-sdk"]

[dependencies]
nostr-sdk = { version = "0.35", optional = true }
```

**Step 2: Implement**

- **Config:** `private_key` (hex or nsec), `relays` (list of relay URLs), `allowed_pubkeys` (hex pubkeys to accept DMs from)
- **Capabilities:** attachments=false, images=false, audio=false, video=false, reactions=true, replies=true, editing=false, deletion=false, typing=false, read_receipts=false, rich_text=false, max_message_length=65535, max_attachment_size=0
- **start():** Connect to relays, subscribe to NIP-04 encrypted DMs and mentions. Decrypt incoming messages.
- **send():** Encrypt with NIP-04, publish to relays.

**Step 3: Tests and commit**

```bash
git commit -m "gateway: add Nostr channel (NIP-01 relay + NIP-04 encrypted DM)"
```

---

## Final Verification

### Task 14: Integration Verification

**Step 1: Build with all channels**

```bash
cd core && cargo build --features "all-channels,slack,email,matrix,signal,mattermost,irc,webhook,xmpp,nostr"
```

**Step 2: Run all tests**

```bash
cd core && cargo test --features "all-channels,slack,email,matrix,signal,mattermost,irc,webhook,xmpp,nostr" -- --nocapture
```

**Step 3: Update `all-channels` feature flag**

Ensure `core/Cargo.toml` `all-channels` includes all new channels:
```toml
all-channels = ["telegram", "discord", "whatsapp", "slack", "email", "matrix", "signal", "mattermost", "irc", "webhook", "xmpp", "nostr"]
```

**Step 4: Final commit**

```bash
git commit -m "gateway: update all-channels feature flag with 10 new channels"
```
