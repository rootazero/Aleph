# Telegram Channel Enhancement Design

**Date:** 2026-04-16
**Status:** Approved
**Supersedes:** 2026-03-27-telegram-channel-overhaul-design

## Overview

This design specifies improvements to Aleph's Telegram channel implementation, addressing missing functionality and architectural deficiencies identified through comparison with OpenClaw. The goal is feature alignment while leveraging Rust's type safety and Aleph's existing architecture.

## Motivation

OpenClaw's Telegram implementation provides several features absent or incomplete in Aleph:

| Feature | OpenClaw | Aleph | Priority |
|---------|----------|-------|----------|
| Multi-account routing | accounts.ts | send() uses first account only | P0 - Blocked |
| Session state | bot-message-context.session.ts | None | P1 |
| Group policy engine | group-policy.ts | Rendering only | P1 |
| Native command menu | bot-native-commands.ts | None | P2 |
| Inline Keyboard | inline-buttons.ts | None | P2 |
| ACP bindings | ACP mechanism | None | P3 |
| Update deduplication | bot-updates.ts | None | P4 |
| Webhook support | webhook.ts | Polling only | P4 |

## Architecture

### Module Structure

```
src/gateway/interfaces/telegram/
├── mod.rs                      # TelegramChannel entry (refactored to ~200 LOC)
├── context.rs                  # NEW: TelegramInboundContext
├── session.rs                 # NEW: Session state management
├── group_policy.rs            # NEW: Group policy engine
├── command_menu.rs            # NEW: Native command menu
├── inline_keyboard.rs         # NEW: Inline keyboard support
├── acp_binding.rs             # NEW: ACP cross-channel bindings
├── delivery.rs                # Existing: Outbound delivery
├── polling.rs                 # Existing: Long polling
├── handlers.rs                # Existing: Message handling
├── bot_instance.rs            # Existing: Per-account bot wrapper
├── offset.rs                  # Existing: Offset tracking
├── config_v2.rs               # Existing: Multi-account config
├── config_resolver.rs          # Existing: Config resolution
├── group_chat.rs              # Existing: Group chat rendering
├── reaction_handler.rs        # Existing: Reactions
├── chunking.rs                # Existing: Message chunking
└── streaming/                 # Existing: Streaming support
```

### Core Context Object

```rust
// context.rs
pub struct TelegramInboundContext {
    pub chat_type: ChatType,
    pub chat_id: i64,
    pub thread_id: Option<i64>,
    pub sender: UserId,
    pub message: Message,
    pub media: Vec<MediaItem>,
    pub session: SessionState,
    pub access: AccessLevel,
    pub conversation_key: ConversationKey,
}

#[derive(Clone)]
pub struct SessionState {
    pub session_id: Uuid,
    pub history: Vec<Message>,
    pub bindings: ConversationBindings,
    pub last_activity: DateTime<Utc>,
    pub metadata: HashMap<String, String>,
}

pub struct ConversationKey {
    pub chat_id: i64,
    pub thread_id: Option<i64>,
    pub route: ConversationRoute,
}
```

**Design Decision:** `TelegramInboundContext` is constructed once per inbound message and passed through the processing pipeline. This provides:
- Type safety for message data
- Clear data flow visibility
- Easy testing with mock contexts

## Feature Specifications

### 1. Multi-Account Routing (P0)

**Problem:** `send()` uses only first account regardless of target chat.

**Solution:** Route based on target chat_id.

```rust
impl TelegramChannel {
    pub async fn send(&mut self, target: &ChatId, message: OutboundMessage) -> SendResult {
        // Resolve which account owns this chat_id
        let account = self.config_resolver.resolve_account_for_chat(target.0)?;
        let bot = self.bot_for_account(&account)?;
        TelegramDelivery::new(bot).send(message).await
    }
}
```

### 2. Session State Management (P1)

**Storage:** SQLite via existing `StateDatabase`.

```rust
// session.rs
pub struct SessionStore {
    db: Arc<StateDatabase>,
}

impl SessionStore {
    pub async fn get(&self, key: &ConversationKey) -> Result<Option<SessionState>>;
    pub async fn set(&self, key: &ConversationKey, state: &SessionState) -> Result<()>;
    pub async fn delete(&self, key: &ConversationKey) -> Result<()>;
    pub async fn cleanup_expired(&self, ttl: Duration) -> Result<u64>;
}
```

**Session Index:** `(chat_id, thread_id)` tuple.

**Timeout:** Configurable, default 30 minutes. Last activity updates on each message.

### 3. Group Policy Engine (P1)

**Hierarchy:** TopicOverride > GroupOverride > Global

```rust
// group_policy.rs
pub enum AccessDecision {
    Allow,
    Deny,
    Challenge,  // Require @mention
}

pub struct GroupPolicy {
    pub access: AccessDecision,
    pub require_mention: bool,
    pub allow_commands: bool,
}

impl GroupPolicy {
    pub fn evaluate(&self, ctx: &TelegramInboundContext) -> AccessDecision {
        // White-list mode: default Deny
        // Check mention requirement
        // Check command access
    }
}
```

**Config Integration:** Use existing `config_v2.rs` GroupPolicy structures.

### 4. Native Command Menu (P2)

**Implementation:** Telegram Bot API `/setcommands`.

```rust
// command_menu.rs
pub struct CommandMenu {
    pub commands: Vec<BotCommand>,
}

pub struct BotCommand {
    pub command: String,
    pub description: String,
}

impl TelegramChannel {
    pub async fn register_commands(&self, menu: CommandMenu) -> Result<()> {
        let registry = self.tool_registry.as_ref().ok_or_else(|| anyhow!("No registry"))?;
        let commands = registry.build_telegram_commands(menu);
        self.bot_instances[0].set_my_commands(commands).await?;
        Ok(())
    }
}
```

**Permissions:** Visible/usable determined by user's `AccessLevel` in session.

### 5. Inline Keyboard (P2)

**Structure:**

```rust
// inline_keyboard.rs
pub struct InlineKeyboardMarkup {
    pub buttons: Vec<Vec<InlineButton>>,
}

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
```

**Callback Routing:** Callback queries routed via `CallbackQuery` channel to handler.

### 6. ACP Bindings (P3)

**Purpose:** Enable cross-channel session sharing.

```rust
// acp_binding.rs
pub struct AcpBinding {
    pub platform: Platform,
    pub chat_id: i64,
    pub thread_id: Option<i64>,
    pub aleph_session_id: Uuid,
    pub created_at: DateTime<Utc>,
}

pub struct AcpBindingStore {
    db: Arc<StateDatabase>,
}
```

**Usage:** On message, resolve `ConversationKey` from binding. On first message from new context, create binding.

### 7. Update Deduplication (P4)

**Implementation:** Bloom filter + recent message cache.

```rust
// handlers.rs
pub struct MessageDeduplicator {
    cache: DashMap<(i64, i64), ()>,  // (chat_id, message_id)
}

impl MessageDeduplicator {
    pub fn is_duplicate(&self, chat_id: i64, message_id: i64) -> bool {
        // Check cache, add if not present
        // Cache bounded size with LRU eviction
    }
}
```

### 8. Webhook Support (P4)

**Mode:** Optional. Polling is default (development-friendly). Webhook recommended for production.

```rust
// config_v2.rs
pub enum Mode {
    Polling,
    Webhook {
        url: String,
        secret: Option<String>,
    },
}
```

**Startup:** If webhook mode configured, call `set_webhook`. Otherwise, start polling.

## Configuration

**New config keys in `TelegramConfigV2`:**

```toml
[channels.telegram]
mode = "polling"  # or "webhook"

[channels.telegram.session]
timeout_minutes = 30

[channels.telegram.groups.group_id_here]
require_mention = false
allow_from = ["user_id_1", "user_id_2"]
```

## Implementation Order

1. **context.rs** - New context object (foundation for everything)
2. **session.rs** - Session state management
3. **Multi-account routing** - Fix send() routing
4. **group_policy.rs** - Group policy engine
5. **command_menu.rs** - Native command menu
6. **inline_keyboard.rs** - Inline keyboard
7. **acp_binding.rs** - ACP bindings
8. **Update deduplication** - Handler improvements
9. **Webhook support** - Optional enhancement

## Backward Compatibility

- All changes are additive
- Existing `TelegramConfigV2` structures preserved
- Session data stored in new tables (no migration needed)
- Webhook/polling mode configurable, defaults to existing behavior

## Testing

- Unit tests for each new module
- Integration tests with mock Telegram API
- Property-based tests for deduplication logic
- Session timeout expiration tests

## References

- OpenClaw Telegram implementation: `/Volumes/TBU4/Github/openclaw/extensions/telegram/src/`
- Existing Aleph Telegram: `src/gateway/interfaces/telegram/`
- OpenClaw docs: `docs/channels/telegram.md`
