# Discord Channel Trait Unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix Discord's broken Channel trait `edit/react/delete` methods and remove the dead `MessageOperations` system.

**Architecture:** Move working logic from `DiscordMessageOps` into `DiscordChannel`'s Channel trait methods, unify `delete()` signature to include `conversation_id`, then delete the entire dead `MessageOperations` trait system.

**Tech Stack:** Rust, serenity (Discord), async-trait

**Spec:** `docs/superpowers/specs/2026-03-27-discord-channel-trait-unification-design.md`

---

## File Map

| File | Action | Purpose |
|------|--------|---------|
| `src/gateway/channel.rs:518` | Modify | Change `delete()` signature to add `conversation_id` |
| `src/gateway/interfaces/discord/mod.rs:465-487` | Modify | Implement real `edit()/react()/delete()` |
| `src/gateway/interfaces/telegram/mod.rs:1111` | Modify | Update `delete()` signature |
| `src/gateway/interfaces/slack/mod.rs:214` | Modify | Update `delete()` signature |
| `src/gateway/interfaces/mattermost/mod.rs:263` | Modify | Update `delete()` signature |
| `src/builtin_tools/message/` | Delete | Dead module (types.rs, tool.rs, mod.rs) |
| `src/builtin_tools/mod.rs:47,160-164` | Modify | Remove message module + re-exports |
| `src/gateway/interfaces/discord/message_ops.rs` | Delete | Dead `DiscordMessageOps` |
| `src/gateway/interfaces/telegram/message_ops.rs` | Delete | Dead `TelegramMessageOps` |
| `src/gateway/interfaces/imessage/message_ops.rs` | Delete | Dead `IMessageMessageOps` |
| `src/gateway/interfaces/discord/mod.rs:30,34` | Modify | Remove `pub mod message_ops` + `pub use` |
| `src/gateway/interfaces/telegram/mod.rs:20,23` | Modify | Remove `pub mod message_ops` + `pub use` |
| `src/gateway/interfaces/imessage/mod.rs:26,32` | Modify | Remove `pub mod message_ops` + `pub use` |

---

### Task 1: Unify `delete()` signature in Channel trait

**Files:**
- Modify: `src/gateway/channel.rs:518-525`
- Modify: `src/gateway/interfaces/discord/mod.rs:473` (signature only — real implementation comes in Task 2)

- [ ] **Step 1: Update Channel trait default `delete()` to include `conversation_id`**

In `src/gateway/channel.rs`, change the `delete` method signature:

```rust
// Before (line 518):
async fn delete(&self, message_id: &MessageId) -> ChannelResult<()> {
    if !self.capabilities().deletion {
        return Err(ChannelError::UnsupportedFeature("deletion".to_string()));
    }
    let _ = message_id;
    Ok(())
}

// After:
async fn delete(&self, conversation_id: &ConversationId, message_id: &MessageId) -> ChannelResult<()> {
    if !self.capabilities().deletion {
        return Err(ChannelError::UnsupportedFeature("deletion".to_string()));
    }
    let _ = (conversation_id, message_id);
    Ok(())
}
```

- [ ] **Step 2: Update Telegram `delete()` signature**

In `src/gateway/interfaces/telegram/mod.rs` around line 1111, add `conversation_id` parameter. Read the current implementation first to preserve logic, only change the signature.

```rust
// Before:
async fn delete(&self, message_id: &MessageId) -> ChannelResult<()> {

// After:
async fn delete(&self, _conversation_id: &ConversationId, message_id: &MessageId) -> ChannelResult<()> {
```

- [ ] **Step 3: Update Slack `delete()` signature**

In `src/gateway/interfaces/slack/mod.rs` around line 214, same pattern:

```rust
// Before:
async fn delete(&self, message_id: &MessageId) -> ChannelResult<()> {

// After:
async fn delete(&self, _conversation_id: &ConversationId, message_id: &MessageId) -> ChannelResult<()> {
```

- [ ] **Step 4: Update Mattermost `delete()` signature**

In `src/gateway/interfaces/mattermost/mod.rs` around line 263, same pattern:

```rust
// Before:
async fn delete(&self, message_id: &MessageId) -> ChannelResult<()> {

// After:
async fn delete(&self, _conversation_id: &ConversationId, message_id: &MessageId) -> ChannelResult<()> {
```

- [ ] **Step 5: Update Discord `delete()` signature (implementation comes in Task 2)**

In `src/gateway/interfaces/discord/mod.rs` around line 473, update the signature to match the new trait. Keep the stub body for now — Task 2 will replace it with real logic:

```rust
// Before:
async fn delete(&self, message_id: &MessageId) -> ChannelResult<()> {
    let _ = message_id;
    Err(ChannelError::UnsupportedFeature(
        "Message deletion requires channel context".to_string(),
    ))
}

// After:
async fn delete(&self, conversation_id: &ConversationId, message_id: &MessageId) -> ChannelResult<()> {
    let _ = (conversation_id, message_id);
    Err(ChannelError::UnsupportedFeature(
        "Message deletion requires channel context".to_string(),
    ))
}
```

- [ ] **Step 6: Run `cargo check -p alephcore`**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: compiles with no errors

- [ ] **Step 7: Commit**

```bash
git add src/gateway/channel.rs src/gateway/interfaces/discord/mod.rs src/gateway/interfaces/telegram/mod.rs src/gateway/interfaces/slack/mod.rs src/gateway/interfaces/mattermost/mod.rs
git commit -m "channel: unify delete() signature to include conversation_id"
```

---

### Task 2: Implement Discord `edit()/react()/delete()` in Channel trait

**Files:**
- Modify: `src/gateway/interfaces/discord/mod.rs:46-52,465-487`

**Reference:** Logic ported from `src/gateway/interfaces/discord/message_ops.rs` (will be deleted in Task 3).

- [ ] **Step 1: Add `EditMessage` import**

In `src/gateway/interfaces/discord/mod.rs`, add `EditMessage` and `MessageId as SerenityMessageId` to the serenity imports (around line 46-52):

```rust
use serenity::{
    all::{
        ChannelId as SerenityChannelId, Context, CreateAttachment, CreateMessage,
        EditMessage, EventHandler, GatewayIntents, Message, MessageId as SerenityMessageId, Ready,
    },
    Client,
};
```

- [ ] **Step 2: Add helper methods to `DiscordChannel`**

Add two helpers inside the **inherent** `impl DiscordChannel` block (lines 68–107, after `fn capabilities()`, before the `}`). NOT in the `impl Channel for DiscordChannel` block (line 264+):

```rust
    /// Parse a conversation_id into a SerenityChannelId, handling dm:user_id format.
    async fn resolve_channel_id(
        &self,
        conversation_id: &ConversationId,
    ) -> ChannelResult<SerenityChannelId> {
        let http = self.http.as_ref().ok_or_else(|| {
            ChannelError::NotConnected("HTTP client not initialized".to_string())
        })?;

        if conversation_id.as_str().starts_with("dm:") {
            let user_id: u64 = conversation_id
                .as_str()
                .strip_prefix("dm:")
                .unwrap()
                .parse()
                .map_err(|e| ChannelError::Internal(format!("Invalid user ID: {}", e)))?;

            let user = serenity::all::UserId::new(user_id);
            let dm = user.create_dm_channel(http).await.map_err(|e| {
                ChannelError::Internal(format!("Failed to create DM channel: {}", e))
            })?;
            Ok(dm.id)
        } else {
            conversation_id
                .as_str()
                .parse::<u64>()
                .map(SerenityChannelId::new)
                .map_err(|e| ChannelError::Internal(format!("Invalid channel ID: {}", e)))
        }
    }

    /// Parse a MessageId string into a SerenityMessageId.
    fn parse_message_id(message_id: &MessageId) -> ChannelResult<SerenityMessageId> {
        message_id
            .as_str()
            .parse::<u64>()
            .map(SerenityMessageId::new)
            .map_err(|e| ChannelError::Internal(format!("Invalid message ID: {}", e)))
    }
```

- [ ] **Step 3: Implement `edit()`**

Replace the `edit` method in `impl Channel for DiscordChannel` (line 465-471):

```rust
    async fn edit(&self, conversation_id: &ConversationId, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        let http = self.http.as_ref().ok_or_else(|| {
            ChannelError::NotConnected("HTTP client not initialized".to_string())
        })?;

        let channel_id = self.resolve_channel_id(conversation_id).await?;
        let msg_id = Self::parse_message_id(message_id)?;

        let builder = EditMessage::new().content(new_text);
        channel_id
            .edit_message(http, msg_id, builder)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to edit message: {}", e)))?;

        Ok(())
    }
```

- [ ] **Step 4: Implement `react()`**

Replace the `react` method (line 481-487):

```rust
    async fn react(&self, conversation_id: &ConversationId, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
        let http = self.http.as_ref().ok_or_else(|| {
            ChannelError::NotConnected("HTTP client not initialized".to_string())
        })?;

        let channel_id = self.resolve_channel_id(conversation_id).await?;
        let msg_id = Self::parse_message_id(message_id)?;

        // Parse emoji — custom format <:name:id> or <a:name:id>, otherwise Unicode
        let reaction_type = if reaction.starts_with('<') {
            serenity::all::ReactionType::try_from(reaction).map_err(|e| {
                ChannelError::Internal(format!("Invalid emoji format: {}", e))
            })?
        } else {
            serenity::all::ReactionType::Unicode(reaction.to_string())
        };

        http.create_reaction(channel_id, msg_id, &reaction_type)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to add reaction: {}", e)))?;

        Ok(())
    }
```

- [ ] **Step 5: Implement `delete()`**

Replace the `delete` method (line 473-479) — note the new signature with `conversation_id`:

```rust
    async fn delete(&self, conversation_id: &ConversationId, message_id: &MessageId) -> ChannelResult<()> {
        let http = self.http.as_ref().ok_or_else(|| {
            ChannelError::NotConnected("HTTP client not initialized".to_string())
        })?;

        let channel_id = self.resolve_channel_id(conversation_id).await?;
        let msg_id = Self::parse_message_id(message_id)?;

        channel_id
            .delete_message(http, msg_id)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to delete message: {}", e)))?;

        Ok(())
    }
```

- [ ] **Step 6: Run `cargo check -p alephcore`**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: compiles with no errors

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -10`
Expected: all tests pass (Discord tests are compile-time only, no serenity mock)

- [ ] **Step 8: Commit**

```bash
git add src/gateway/interfaces/discord/mod.rs
git commit -m "discord: implement edit/react/delete in Channel trait"
```

---

### Task 3: Delete dead MessageOperations system

**Files:**
- Delete: `src/builtin_tools/message/types.rs`
- Delete: `src/builtin_tools/message/tool.rs`
- Delete: `src/builtin_tools/message/mod.rs`
- Delete: `src/gateway/interfaces/discord/message_ops.rs`
- Delete: `src/gateway/interfaces/telegram/message_ops.rs`
- Delete: `src/gateway/interfaces/imessage/message_ops.rs`
- Modify: `src/builtin_tools/mod.rs`
- Modify: `src/gateway/interfaces/discord/mod.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`
- Modify: `src/gateway/interfaces/imessage/mod.rs`

- [ ] **Step 1: Delete the builtin_tools/message module directory**

```bash
rm src/builtin_tools/message/types.rs
rm src/builtin_tools/message/tool.rs
rm src/builtin_tools/message/mod.rs
rmdir src/builtin_tools/message
```

- [ ] **Step 2: Remove message module from builtin_tools/mod.rs**

In `src/builtin_tools/mod.rs`:

Remove the doc comment reference (line 15):
```
//! - [`MessageTool`] - Cross-channel message operations
```

Remove the module declaration (line 47):
```
pub mod message;
```

Remove the re-export block (lines 159-164):
```rust
// Message tool re-exports
pub use message::{
    ChannelCapabilities, DeleteParams, EditParams, MessageAction, MessageOperations,
    MessageResult, MessageTool, MessageToolArgs, MessageToolOutput, ReactParams, ReplyParams,
    SendParams,
};
```

- [ ] **Step 3: Delete Discord message_ops.rs**

```bash
rm src/gateway/interfaces/discord/message_ops.rs
```

- [ ] **Step 4: Clean Discord mod.rs re-exports**

In `src/gateway/interfaces/discord/mod.rs`, remove:

Line 30: `pub mod message_ops;`
Line 34: `pub use message_ops::DiscordMessageOps;`

- [ ] **Step 5: Delete Telegram message_ops.rs**

```bash
rm src/gateway/interfaces/telegram/message_ops.rs
```

- [ ] **Step 6: Clean Telegram mod.rs re-exports**

In `src/gateway/interfaces/telegram/mod.rs`, remove:

Line 20: `pub mod message_ops;`
Line 23: `pub use message_ops::TelegramMessageOps;`

- [ ] **Step 7: Delete iMessage message_ops.rs**

```bash
rm src/gateway/interfaces/imessage/message_ops.rs
```

- [ ] **Step 8: Clean iMessage mod.rs re-exports**

In `src/gateway/interfaces/imessage/mod.rs`, remove:

Line 26: `pub mod message_ops;`
Line 32: `pub use message_ops::IMessageMessageOps;`

- [ ] **Step 9: Run `cargo check -p alephcore`**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: compiles. If there are dangling references, fix them (should be none per our analysis).

- [ ] **Step 10: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -15`
Expected: all tests pass. Tests in deleted files are gone. No other tests depend on deleted code.

- [ ] **Step 11: Commit**

```bash
git add -A src/builtin_tools/message src/builtin_tools/mod.rs \
  src/gateway/interfaces/discord/message_ops.rs src/gateway/interfaces/discord/mod.rs \
  src/gateway/interfaces/telegram/message_ops.rs src/gateway/interfaces/telegram/mod.rs \
  src/gateway/interfaces/imessage/message_ops.rs src/gateway/interfaces/imessage/mod.rs
git commit -m "channel: remove dead MessageOperations system

Delete MessageTool, MessageOperationsRegistry, MessageOperations trait,
and three XxxMessageOps adapters (Discord, Telegram, iMessage).
None of these were wired into runtime."
```

---

### Task 4: Final verification

- [ ] **Step 1: Clean build check**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: clean compile, zero warnings related to our changes

- [ ] **Step 2: Full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -15`
Expected: all tests pass

- [ ] **Step 3: Clippy**

Run: `cargo clippy -p alephcore 2>&1 | tail -15`
Expected: no new warnings from our changes
