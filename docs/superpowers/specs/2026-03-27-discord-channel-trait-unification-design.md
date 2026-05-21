# Discord Channel Trait Unification

**Date:** 2026-03-27
**Status:** Approved
**Scope:** Fix Discord Channel trait abstraction break + remove dead MessageOperations system

## Problem

Two parallel trait systems exist for message operations:

| System | Location | Status |
|--------|----------|--------|
| `Channel` trait (`edit/react/delete`) | `gateway/channel.rs` | **Live** — called by ReplyEmitter typewriter mode |
| `MessageOperations` trait + `MessageTool` + `MessageOperationsRegistry` | `builtin_tools/message/` | **Dead** — never wired into runtime |

**Critical bug:** ReplyEmitter calls `channel_registry.edit()` for Discord typewriter streaming, but `DiscordChannel`'s `Channel::edit()` returns `UnsupportedFeature`. The working edit logic exists in `DiscordMessageOps` but is never registered or called.

Result: **Discord typewriter streaming is broken.**

## Solution

### 1. Fix Discord Channel trait implementation

Move edit/react/delete logic from `DiscordMessageOps` into `DiscordChannel`'s `Channel` trait methods.

`DiscordChannel` already holds `http: Option<Arc<serenity::http::Http>>` — no new dependencies needed. Access pattern: `self.http.as_ref().ok_or(NotConnected)?`.

**edit():**
- Parse `conversation_id` → `SerenityChannelId` (handle `dm:user_id` prefix by creating DM channel)
- Parse `message_id` → `SerenityMessageId`
- Call `channel_id.edit_message(http, message_id, EditMessage::new().content(new_text))`

**react():**
- Parse emoji (Unicode vs custom `<:name:id>`)
- Call `http.create_reaction(channel_id, message_id, &reaction_type)`
- `remove` not supported yet (requires bot_user_id propagation — separate follow-up)

**delete():**
- Parse `conversation_id` → `SerenityChannelId`
- Parse `message_id` → `SerenityMessageId`
- Call `channel_id.delete_message(http, message_id)`

### 2. Unify Channel trait `delete()` signature

Current signature is inconsistent:

```rust
// Before — delete missing conversation_id
async fn edit(&self, conversation_id, message_id, new_text) -> ChannelResult<()>;
async fn react(&self, conversation_id, message_id, reaction) -> ChannelResult<()>;
async fn delete(&self, message_id) -> ChannelResult<()>;  // inconsistent

// After — unified
async fn delete(&self, conversation_id: &ConversationId, message_id: &MessageId) -> ChannelResult<()>;
```

Affected implementations (signature-only change, logic unchanged):
- `channel.rs` default implementation
- `telegram/mod.rs`
- `discord/mod.rs`
- `slack/mod.rs`
- `mattermost/mod.rs`

No callers of `channel.delete()` exist — zero runtime impact.

### 3. Delete dead code

**Delete entire module:**
- `src/builtin_tools/message/` (types.rs, tool.rs, mod.rs)

**Delete files:**
- `src/gateway/interfaces/discord/message_ops.rs`
- `src/gateway/interfaces/telegram/message_ops.rs`
- `src/gateway/interfaces/imessage/message_ops.rs`

**Clean up re-exports:**
- `src/gateway/interfaces/discord/mod.rs` — remove `pub mod message_ops` and `pub use`
- `src/gateway/interfaces/telegram/mod.rs` — remove `pub mod message_ops` and `pub use`
- `src/gateway/interfaces/imessage/mod.rs` — remove `pub mod message_ops` and `pub use`
- `src/builtin_tools/mod.rs` — remove `pub mod message` and related re-exports

**Do NOT delete** other `message_ops` modules (slack, email, nostr, matrix, signal, xmpp, irc, mattermost, webhook). These are internal utility modules that do NOT implement the `MessageOperations` trait — they just share the name. Only 3 files implement `impl MessageOperations for`: discord, telegram, imessage.

## Not in scope

- react remove (needs bot_user_id propagation chain — separate task)
- New abstractions or adapter patterns
- Changes to ReplyEmitter / ChannelRegistry calling patterns (already correct)
- Other channels (Telegram/Slack Channel trait edit/react already work)

## Verification

- `cargo check -p alephcore` compiles
- `cargo test -p alephcore --lib` passes
- Discord typewriter streaming edit works at runtime
