# Telegram Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement missing Telegram features aligned with OpenClaw: multi-account routing, session state, group policy, command menu, inline keyboard, ACP bindings, update deduplication, and webhook support.

**Architecture:** New `TelegramInboundContext` object as the core context passed through the message processing pipeline. Each feature module (session, group_policy, etc.) is a focused, single-responsibility module.

**Tech Stack:** Rust (teloxide framework), SQLite via existing StateDatabase, async/await with tokio

---

## Task 1: Create context.rs - TelegramInboundContext

**Files:**
- Create: `src/gateway/interfaces/telegram/context.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Create context.rs with TelegramInboundContext struct**

```rust
use serde::{Deserialize, Serialize};
use crate::gateway::channel::{ChatType, MessageId, UserId};
use super::session::SessionState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramInboundContext {
    pub chat_type: ChatType,
    pub chat_id: i64,
    pub thread_id: Option<i64>,
    pub sender: UserId,
    pub message_id: MessageId,
    pub message_text: String,
    pub media: Vec<MediaItem>,
    pub session: SessionState,
    pub access_level: AccessLevel,
    pub conversation_key: ConversationKey,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConversationKey {
    pub chat_id: i64,
    pub thread_id: Option<i64>,
}

impl ConversationKey {
    pub fn new(chat_id: i64, thread_id: Option<i64>) -> Self {
        Self { chat_id, thread_id }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum AccessLevel {
    Owner,
    Admin,
    Member,
    Stranger,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub kind: MediaKind,
    pub file_id: String,
    pub file_size: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MediaKind {
    Photo,
    Video,
    Voice,
    Document,
    Sticker,
}
```

- [ ] **Step 2: Add mod.rs exports**

Add to `src/gateway/interfaces/telegram/mod.rs`:
```rust
pub mod context;
pub use context::{TelegramInboundContext, ConversationKey, AccessLevel, MediaItem, MediaKind};
```

- [ ] **Step 3: Run cargo check to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/telegram/context.rs src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: add TelegramInboundContext and ConversationKey"
```

---

## Task 2: Create session.rs - Session State Management

**Files:**
- Create: `src/gateway/interfaces/telegram/session.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Create session.rs with SessionStore**

```rust
use crate::gateway::interfaces::telegram::context::ConversationKey;
use crate::resilience::StateDatabase;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;

const SESSION_TABLE: &str = "telegram_sessions";

#[derive(Debug, Clone)]
pub struct SessionState {
    pub session_id: uuid::Uuid,
    pub conversation_key: ConversationKey,
    pub history: Vec<crate::gateway::channel::Message>,
    pub last_activity: DateTime<Utc>,
    pub metadata: std::collections::HashMap<String, String>,
}

impl SessionState {
    pub fn new(conversation_key: ConversationKey) -> Self {
        Self {
            session_id: uuid::Uuid::new_v4(),
            conversation_key,
            history: Vec::new(),
            last_activity: Utc::now(),
            metadata: std::collections::HashMap::new(),
        }
    }

    pub fn is_expired(&self, ttl: Duration) -> bool {
        Utc::now() - self.last_activity > ttl
    }
}

pub struct SessionStore {
    db: Arc<StateDatabase>,
}

impl SessionStore {
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self { db }
    }

    pub async fn init(&self) -> Result<()> {
        self.db.execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS {} (
                    chat_id INTEGER NOT NULL,
                    thread_id INTEGER,
                    session_id TEXT NOT NULL,
                    history_json TEXT NOT NULL,
                    last_activity INTEGER NOT NULL,
                    metadata_json TEXT NOT NULL,
                    PRIMARY KEY (chat_id, thread_id)
                )",
                SESSION_TABLE
            ),
        ).await?;
        Ok(())
    }

    pub async fn get(&self, key: &ConversationKey) -> Result<Option<SessionState>> {
        let result = self.db.query_row(
            &format!(
                "SELECT session_id, history_json, last_activity, metadata_json
                 FROM {} WHERE chat_id = ? AND thread_id IS ?",
                SESSION_TABLE
            ),
            rusqlite::params![key.chat_id, key.thread_id],
        ).await;

        match result {
            Ok(row) => {
                let session_id: String = row.get(0)?;
                let history_json: String = row.get(1)?;
                let last_activity_ts: i64 = row.get(2)?;
                let metadata_json: String = row.get(3)?;

                Ok(Some(SessionState {
                    session_id: uuid::Uuid::parse_str(&session_id)?,
                    conversation_key: key.clone(),
                    history: serde_json::from_str(&history_json)?,
                    last_activity: DateTime::from_timestamp(last_activity_ts, 0)
                        .unwrap_or_else(Utc::now),
                    metadata: serde_json::from_str(&metadata_json)?,
                }))
            }
            Err(_) => Ok(None),
        }
    }

    pub async fn set(&self, state: &SessionState) -> Result<()> {
        self.db.execute(
            &format!(
                "INSERT OR REPLACE INTO {} (chat_id, thread_id, session_id, history_json, last_activity, metadata_json)
                 VALUES (?, ?, ?, ?, ?, ?)",
                SESSION_TABLE
            ),
            rusqlite::params![
                state.conversation_key.chat_id,
                state.conversation_key.thread_id,
                state.session_id.to_string(),
                serde_json::to_string(&state.history)?,
                state.last_activity.timestamp(),
                serde_json::to_string(&state.metadata)?
            ],
        ).await?;
        Ok(())
    }

    pub async fn delete(&self, key: &ConversationKey) -> Result<()> {
        self.db.execute(
            &format!("DELETE FROM {} WHERE chat_id = ? AND thread_id IS ?", SESSION_TABLE),
            rusqlite::params![key.chat_id, key.thread_id],
        ).await?;
        Ok(())
    }

    pub async fn cleanup_expired(&self, ttl: Duration) -> Result<u64> {
        let cutoff = (Utc::now() - ttl).timestamp();
        let result = self.db.execute(
            &format!("DELETE FROM {} WHERE last_activity < ?", SESSION_TABLE),
            rusqlite::params![cutoff],
        ).await?;
        Ok(result as u64)
    }
}
```

- [ ] **Step 2: Add mod.rs exports**

```rust
pub mod session;
pub use session::{SessionStore, SessionState};
```

- [ ] **Step 3: Run cargo check to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/telegram/session.rs src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: add SessionStore for persistent session state"
```

---

## Task 3: Fix Multi-Account Routing in send()

**Files:**
- Modify: `src/gateway/interfaces/telegram/config_resolver.rs`
- Modify: `src/gateway/interfaces/telegram/delivery.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Add resolve_account_for_chat to ConfigResolver**

In `config_resolver.rs`, add:
```rust
impl ConfigResolver {
    /// Resolve which account should handle messages for a given chat_id.
    /// Falls back to first account if no specific match.
    pub fn resolve_account_for_chat(&self, chat_id: i64) -> Result<&ResolvedConfig, ChannelError> {
        // Check group overrides first
        for account in &self.accounts {
            if let Some(config) = self.resolve(account, 0, None)? {
                if config.is_chat_allowed(chat_id) {
                    return Ok(config);
                }
            }
        }
        // Fallback to first account
        self.accounts.first()
            .and_then(|a| self.resolve(a, 0, None).ok().flatten())
            .ok_or_else(|| ChannelError::Configuration("No account found".into()))
    }
}
```

- [ ] **Step 2: Update TelegramDelivery::send to accept account context**

In `delivery.rs`:
```rust
pub struct TelegramDelivery {
    bot: Bot,
    account: ResolvedConfig,
}

impl TelegramDelivery {
    pub fn new(bot: Bot, account: ResolvedConfig) -> Self {
        Self { bot, account }
    }

    pub async fn send(&self, target: &ChatId, msg: OutboundMessage) -> SendResult {
        let chat_id = target.0;
        // Use account's bot for this chat_id
        self.send_to_chat(chat_id, msg).await
    }
}
```

- [ ] **Step 3: Update TelegramChannel::send to route correctly**

In `mod.rs`:
```rust
pub async fn send(&mut self, target: ChatId, msg: OutboundMessage) -> SendResult {
    let account = self.config_resolver.resolve_account_for_chat(target.0)?;
    let bot = self.bot_for_account(&account.id);
    TelegramDelivery::new(bot, account).send(&target, msg).await
}
```

- [ ] **Step 4: Run cargo check to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git commit -m "telegram: fix multi-account routing in send()"
```

---

## Task 4: Create group_policy.rs - Group Policy Engine

**Files:**
- Create: `src/gateway/interfaces/telegram/group_policy.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Create group_policy.rs**

```rust
use crate::gateway::interfaces::telegram::context::{TelegramInboundContext, AccessLevel};
use crate::gateway::interfaces::telegram::config_v2::{GroupPolicyConfig, DmPolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessDecision {
    Allow,
    Deny,
    Challenge,  // Requires @mention in group
}

pub struct GroupPolicy {
    pub global_default: AccessDecision,
    pub group_overrides: std::collections::HashMap<i64, GroupPolicyConfig>,
}

impl GroupPolicy {
    pub fn new() -> Self {
        Self {
            global_default: AccessDecision::Deny,
            group_overrides: std::collections::HashMap::new(),
        }
    }

    pub fn evaluate(&self, ctx: &TelegramInboundContext) -> AccessDecision {
        match ctx.chat_type {
            ChatType::Direct => self.evaluate_dm(ctx),
            ChatType::Group | ChatType::Supergroup | ChatType::Channel => {
                self.evaluate_group(ctx)
            }
        }
    }

    fn evaluate_dm(&self, _ctx: &TelegramInboundContext) -> AccessDecision {
        // DM policy handled by existing pairing/access logic
        AccessDecision::Allow
    }

    fn evaluate_group(&self, ctx: &TelegramInboundContext) -> AccessDecision {
        // Check group-specific override
        if let Some(config) = self.group_overrides.get(&ctx.chat_id) {
            return self.evaluate_config(config, ctx);
        }
        // Fall back to global default (deny for groups)
        self.global_default
    }

    fn evaluate_config(&self, config: &GroupPolicyConfig, ctx: &TelegramInboundContext) -> AccessDecision {
        // Check if sender is in allowlist
        if !config.allow_from.is_empty() {
            let sender_id = ctx.sender.0;
            if !config.allow_from.contains(&sender_id) {
                return AccessDecision::Deny;
            }
        }

        // Check mention requirement
        if config.require_mention && !ctx.message_text.contains('@') {
            return AccessDecision::Challenge;
        }

        AccessDecision::Allow
    }
}
```

- [ ] **Step 2: Add mod.rs exports**

```rust
pub mod group_policy;
pub use group_policy::{GroupPolicy, AccessDecision};
```

- [ ] **Step 3: Run cargo check to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/telegram/group_policy.rs src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: add GroupPolicy engine for access control"
```

---

## Task 5: Create command_menu.rs - Native Command Menu

**Files:**
- Create: `src/gateway/interfaces/telegram/command_menu.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`
- Modify: `src/gateway/interfaces/telegram/bot_instance.rs`

- [ ] **Step 1: Create command_menu.rs**

```rust
use teloxide::types::BotCommand;
use crate::dispatcher::ToolRegistry;
use super::context::AccessLevel;

pub struct CommandMenu {
    pub commands: Vec<CommandDefinition>,
}

pub struct CommandDefinition {
    pub command: String,
    pub description: String,
    pub visible_for: AccessLevel,
    pub usable_for: AccessLevel,
    pub aliases: Vec<String>,
}

impl CommandMenu {
    pub fn from_tool_registry(registry: &ToolRegistry) -> Self {
        let mut commands = Vec::new();

        for tool in registry.list_tools() {
            commands.push(CommandDefinition {
                command: tool.name.clone(),
                description: tool.description.clone(),
                visible_for: AccessLevel::Member,
                usable_for: AccessLevel::Member,
                aliases: Vec::new(),
            });
        }

        Self { commands }
    }

    pub fn to_bot_commands(&self, access_level: AccessLevel) -> Vec<BotCommand> {
        self.commands
            .iter()
            .filter(|cmd| cmd.visible_for <= access_level)
            .map(|cmd| BotCommand {
                command: cmd.command.clone(),
                description: cmd.description.clone(),
            })
            .collect()
    }
}
```

- [ ] **Step 2: Add set_my_commands to BotInstance**

In `bot_instance.rs`:
```rust
use teloxide::requests::Requester;

impl BotInstance {
    pub async fn set_my_commands(&self, commands: Vec<teloxide::types::BotCommand>) -> Result<(), BotError> {
        self.bot.set_my_commands(commands).await?;
        Ok(())
    }
}
```

- [ ] **Step 3: Wire up command registration in TelegramChannel::start**

In `mod.rs`:
```rust
pub async fn start(&mut self) -> Result<(), ChannelError> {
    // ... existing startup code ...

    // Register command menu
    if let Some(registry) = &self.tool_registry {
        let menu = CommandMenu::from_tool_registry(registry);
        let bot_commands = menu.to_bot_commands(AccessLevel::Member);
        self.bot_instances[0].set_my_commands(bot_commands).await?;
    }

    Ok(())
}
```

- [ ] **Step 4: Run cargo check to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/telegram/command_menu.rs src/gateway/interfaces/telegram/mod.rs src/gateway/interfaces/telegram/bot_instance.rs
git commit -m "telegram: add native command menu with Bot API integration"
```

---

## Task 6: Create inline_keyboard.rs - Inline Keyboard Support

**Files:**
- Create: `src/gateway/interfaces/telegram/inline_keyboard.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Create inline_keyboard.rs**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineKeyboardMarkup {
    pub buttons: Vec<Vec<InlineButton>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InlineButton {
    Callback {
        text: String,
        data: String,
    },
    Url {
        text: String,
        url: String,
    },
}

impl InlineKeyboardMarkup {
    pub fn new(buttons: Vec<Vec<InlineButton>>) -> Self {
        Self { buttons }
    }

    pub fn callback_row(text: &str, data: &str) -> Vec<InlineButton> {
        vec![InlineButton::Callback {
            text: text.to_string(),
            data: data.to_string(),
        }]
    }

    pub fn url_row(text: &str, url: &str) -> Vec<InlineButton> {
        vec![InlineButton::Url {
            text: text.to_string(),
            url: url.to_string(),
        }]
    }

    pub fn to_teloxide(&self) -> teloxide::types::InlineKeyboardMarkup {
        let rows: Vec<Vec<teloxide::types::InlineKeyboardButton>> = self.buttons
            .iter()
            .map(|row| {
                row.iter()
                    .map(|btn| match btn {
                        InlineButton::Callback { text, data } => {
                            teloxide::types::InlineKeyboardButton::Callback(
                                teloxide::types::CallbackButton {
                                    text: text.clone(),
                                    data: data.clone().into(),
                                }
                            )
                        }
                        InlineButton::Url { text, url } => {
                            teloxide::types::InlineKeyboardButton::Url(
                                teloxide::types::UrlButton {
                                    text: text.clone(),
                                    url: url.parse().unwrap(),
                                }
                            )
                        }
                    })
                    .collect()
            })
            .collect();

        teloxide::types::InlineKeyboardMarkup::new(rows)
    }
}
```

- [ ] **Step 2: Add mod.rs exports**

```rust
pub mod inline_keyboard;
pub use inline_keyboard::{InlineKeyboardMarkup, InlineButton};
```

- [ ] **Step 3: Run cargo check to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/telegram/inline_keyboard.rs src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: add inline keyboard support for message interactions"
```

---

## Task 7: Create acp_binding.rs - ACP Cross-Channel Bindings

**Files:**
- Create: `src/gateway/interfaces/telegram/acp_binding.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Create acp_binding.rs**

```rust
use crate::gateway::interfaces::telegram::context::ConversationKey;
use crate::resilience::StateDatabase;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::sync::Arc;

const ACP_BINDING_TABLE: &str = "telegram_acp_bindings";

#[derive(Debug, Clone)]
pub struct AcpBinding {
    pub platform: String,  // "telegram"
    pub chat_id: i64,
    pub thread_id: Option<i64>,
    pub aleph_session_id: uuid::Uuid,
    pub created_at: DateTime<Utc>,
}

pub struct AcpBindingStore {
    db: Arc<StateDatabase>,
}

impl AcpBindingStore {
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self { db }
    }

    pub async fn init(&self) -> Result<()> {
        self.db.execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS {} (
                    platform TEXT NOT NULL,
                    chat_id INTEGER NOT NULL,
                    thread_id INTEGER,
                    aleph_session_id TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    PRIMARY KEY (platform, chat_id, thread_id)
                )",
                ACP_BINDING_TABLE
            ),
        ).await?;
        Ok(())
    }

    pub async fn get(&self, key: &ConversationKey) -> Result<Option<AcpBinding>> {
        let result = self.db.query_row(
            &format!(
                "SELECT aleph_session_id, created_at FROM {} WHERE platform = 'telegram' AND chat_id = ? AND thread_id IS ?",
                ACP_BINDING_TABLE
            ),
            rusqlite::params![key.chat_id, key.thread_id],
        ).await;

        match result {
            Ok(row) => {
                let session_id: String = row.get(0)?;
                let created_ts: i64 = row.get(1)?;
                Ok(Some(AcpBinding {
                    platform: "telegram".to_string(),
                    chat_id: key.chat_id,
                    thread_id: key.thread_id,
                    aleph_session_id: uuid::Uuid::parse_str(&session_id)?,
                    created_at: DateTime::from_timestamp(created_ts, 0).unwrap_or_else(Utc::now),
                }))
            }
            Err(_) => Ok(None),
        }
    }

    pub async fn set(&self, binding: &AcpBinding) -> Result<()> {
        self.db.execute(
            &format!(
                "INSERT OR REPLACE INTO {} (platform, chat_id, thread_id, aleph_session_id, created_at)
                 VALUES ('telegram', ?, ?, ?, ?)",
                ACP_BINDING_TABLE
            ),
            rusqlite::params![
                binding.chat_id,
                binding.thread_id,
                binding.aleph_session_id.to_string(),
                binding.created_at.timestamp()
            ],
        ).await?;
        Ok(())
    }

    pub async fn delete(&self, key: &ConversationKey) -> Result<()> {
        self.db.execute(
            &format!("DELETE FROM {} WHERE platform = 'telegram' AND chat_id = ? AND thread_id IS ?", ACP_BINDING_TABLE),
            rusqlite::params![key.chat_id, key.thread_id],
        ).await?;
        Ok(())
    }
}
```

- [ ] **Step 2: Add mod.rs exports**

```rust
pub mod acp_binding;
pub use acp_binding::{AcpBindingStore, AcpBinding};
```

- [ ] **Step 3: Run cargo check to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/telegram/acp_binding.rs src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: add ACP binding store for cross-channel session sharing"
```

---

## Task 8: Add Update Deduplication in handlers.rs

**Files:**
- Modify: `src/gateway/interfaces/telegram/handlers.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Add MessageDeduplicator to handlers.rs**

```rust
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

const DEDUP_CACHE_SIZE: usize = 10000;

pub struct MessageDeduplicator {
    cache: Arc<RwLock<HashSet<(i64, i64)>>>,  // (chat_id, message_id)
}

impl MessageDeduplicator {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashSet::with_capacity(DEDUP_CACHE_SIZE))),
        }
    }

    pub async fn is_duplicate(&self, chat_id: i64, message_id: i64) -> bool {
        let key = (chat_id, message_id);
        let mut cache = self.cache.write().await;

        if cache.contains(&key) {
            return true;
        }

        // Evict oldest if at capacity
        if cache.len() >= DEDUP_CACHE_SIZE {
            let oldest = cache.iter().next().cloned();
            if let Some(old) = oldest {
                cache.remove(&old);
            }
        }

        cache.insert(key);
        false
    }

    pub async fn clear(&self) {
        self.cache.write().await.clear();
    }
}
```

- [ ] **Step 2: Wire deduplicator into handler**

In `handlers.rs`, update `TelegramHandler` to use deduplicator.

- [ ] **Step 3: Run cargo check to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git commit -m "telegram: add update deduplication to handlers"
```

---

## Task 9: Add Webhook Support

**Files:**
- Modify: `src/gateway/interfaces/telegram/config_v2.rs`
- Create: `src/gateway/interfaces/telegram/webhook.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Add Webhook mode to config_v2.rs**

```rust
#[derive(Debug, Clone)]
pub enum TelegramMode {
    Polling,
    Webhook {
        url: String,
        secret: Option<String>,
    },
}
```

- [ ] **Step 2: Create webhook.rs**

```rust
use teloxide::Bot;
use anyhow::Result;

pub struct WebhookManager {
    bot: Bot,
    url: String,
    secret: Option<String>,
}

impl WebhookManager {
    pub fn new(bot: Bot, url: String, secret: Option<String>) -> Self {
        Self { bot, url, secret }
    }

    pub async fn setup(&self) -> Result<()> {
        use teloxide::requests::Requester;
        let mut params = teloxide::payloads::SetWebhook::new(&self.url);
        if let Some(ref secret) = self.secret {
            params.secret_token(secret.clone());
        }
        self.bot.set_webhook(params).await?;
        Ok(())
    }

    pub async fn teardown(&self) -> Result<()> {
        use teloxide::requests::Requester;
        self.bot.delete_webhook().await?;
        Ok(())
    }
}
```

- [ ] **Step 3: Update TelegramChannel::start for webhook mode**

- [ ] **Step 4: Run cargo check to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/telegram/config_v2.rs src/gateway/interfaces/telegram/webhook.rs src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: add webhook support as alternative to polling"
```

---

## Task 10: Integration - Wire Context Through Pipeline

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs`
- Modify: `src/gateway/interfaces/telegram/handlers.rs`
- Modify: `src/gateway/interfaces/telegram/polling.rs`

- [ ] **Step 1: Update polling.rs to build context**

- [ ] **Step 2: Update handlers to accept context**

- [ ] **Step 3: Run cargo check and tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git commit -m "telegram: wire TelegramInboundContext through message pipeline"
```

---

## Task 11: Cleanup Legacy Code

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Remove legacy config.rs complexity**

- [ ] **Step 2: Clean up mod.rs**

Reduce `mod.rs` size by extracting to submodules.

- [ ] **Step 3: Run cargo check and tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git commit -m "telegram: clean up legacy code, reduce mod.rs complexity"
```

---

## Plan Summary

| Task | Description | Status |
|------|-------------|--------|
| 1 | TelegramInboundContext | pending |
| 2 | Session state management | pending |
| 3 | Multi-account routing fix | pending |
| 4 | Group policy engine | pending |
| 5 | Native command menu | pending |
| 6 | Inline keyboard | pending |
| 7 | ACP bindings | pending |
| 8 | Update deduplication | pending |
| 9 | Webhook support | pending |
| 10 | Integration - wire context | pending |
| 11 | Cleanup legacy code | pending |

## Dependencies

- Task 1 (context.rs) is prerequisite for Tasks 2, 4, 5, 6, 7, 8, 10
- Task 2 (session.rs) is used by Task 10
- Task 10 (integration) requires all prior tasks
