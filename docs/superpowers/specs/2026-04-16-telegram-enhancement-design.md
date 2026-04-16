# Telegram Channel Enhancement Design

**Date:** 2026-04-16
**Status:** Approved
**Supersedes:** 2026-03-27-telegram-channel-overhaul

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

**Design Decision:** `TelegramInboundContext` is constructed once per inbound message and passed through the processing pipeline.

## Feature Specifications

### 1. Multi-Account Routing (P0)

**Problem:** `send()` uses only first account regardless of target chat.

**Solution:** Route based on target chat_id.

### 2. Session State Management (P1)

**Storage:** SQLite via existing `StateDatabase`.

**Session Index:** `(chat_id, thread_id)` tuple.

**Timeout:** Configurable, default 30 minutes.

### 3. Group Policy Engine (P1)

**Hierarchy:** TopicOverride > GroupOverride > Global

**Config Integration:** Use existing `config_v2.rs` GroupPolicy structures.

### 4. Native Command Menu (P2)

**Implementation:** Telegram Bot API `/setcommands`.

### 5. Inline Keyboard (P2)

**Structure:** Callback buttons and URL buttons.

### 6. ACP Bindings (P3)

**Purpose:** Enable cross-channel session sharing.

### 7. Update Deduplication (P4)

**Implementation:** HashSet-based deduplication in handler.

### 8. Webhook Support (P4)

**Mode:** Optional. Polling is default.

## Configuration

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

1. context.rs - New context object (foundation)
2. session.rs - Session state management
3. Multi-account routing - Fix send() routing
4. group_policy.rs - Group policy engine
5. command_menu.rs - Native command menu
6. inline_keyboard.rs - Inline keyboard
7. acp_binding.rs - ACP bindings
8. Update deduplication - Handler improvements
9. Webhook support - Optional enhancement
10. Integration - Wire context through pipeline
11. Cleanup - Reduce mod.rs complexity
