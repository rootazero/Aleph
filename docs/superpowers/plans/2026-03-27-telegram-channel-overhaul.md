# Telegram Channel Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor Aleph's Telegram channel from a 1355-line monolith into 7 focused modules, then upgrade message chunking, access control, network resilience, and streaming.

**Architecture:** Incremental refactor — split `mod.rs` first (pure code-move, zero behavior change), then enhance each new module independently. Each task produces a compilable, testable state.

**Tech Stack:** Rust, teloxide 0.13, tokio, async-trait

**Worktree:** `WT=/Users/zouguojun/Workspace/Aleph-telegram-overhaul` (branch `feat/telegram-overhaul`, already created)

**Spec:** `docs/superpowers/specs/2026-03-27-telegram-channel-overhaul-design.md`

**Key paths (all under WT):**
- `src/gateway/interfaces/telegram/` — the target directory
- `src/gateway/reply_emitter.rs` — streaming integration
- `src/gateway/channel.rs` — Channel trait (read-only reference)

**Test command:** `(cd $WT && cargo test -p alephcore --lib telegram)`
**Check command:** `(cd $WT && cargo check -p alephcore)`

---

## Task 1: Extract `handlers.rs` — message/callback handler closures

**Files:**
- Create: `src/gateway/interfaces/telegram/handlers.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

This extracts `convert_message()`, `extract_attachments()`, and the pairing flow logic from the message_handler closure into a standalone module. The handler closures in `start()` will call these functions instead of inlining the logic.

- [ ] **Step 1: Create `handlers.rs` with `convert_message` and `extract_attachments`**

Move lines 142-358 from `mod.rs` into `handlers.rs` as standalone `pub(crate) async fn` functions. They currently live as `impl TelegramChannel` associated methods — convert them to free functions that take their dependencies as parameters:

```rust
// handlers.rs
use crate::gateway::channel::{
    Attachment, ChannelId, ConversationId, InboundMessage, MessageId, UserId,
};
use crate::gateway::interfaces::telegram::config::TelegramConfig;
use chrono::{TimeZone, Utc};
use std::sync::Arc;
use tokio::sync::RwLock;
use teloxide::prelude::*;
use teloxide::types::{MediaKind, MessageKind};

/// Convert a Telegram message to InboundMessage.
/// Returns None if the message should be filtered (unauthorized user, service message, etc.)
pub(crate) async fn convert_message(
    msg: &teloxide::types::Message,
    bot: &Bot,
    config: &TelegramConfig,
    channel_id: &ChannelId,
    runtime_users: &Arc<RwLock<Vec<i64>>>,
) -> Option<InboundMessage> {
    // ... (exact code from current mod.rs lines 152-252, unchanged)
}

/// Extract attachments from Telegram message, resolving file URLs via Bot API.
pub(crate) async fn extract_attachments(
    msg: &teloxide::types::Message,
    bot: &Bot,
) -> Vec<Attachment> {
    // ... (exact code from current mod.rs lines 256-358, unchanged)
}
```

- [ ] **Step 2: Update `mod.rs` — declare module, replace method calls**

Add `pub mod handlers;` to the module declarations. Remove the two methods from `impl TelegramChannel` and update call sites in `start()`:

```rust
// In start() message_handler closure, change:
//   TelegramChannel::convert_message(&msg, &bot, &config, &channel_id, &runtime_users)
// to:
//   handlers::convert_message(&msg, &bot, &config, &channel_id, &runtime_users)
```

- [ ] **Step 3: Run check**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo check -p alephcore)`
Expected: Compiles with zero errors

- [ ] **Step 4: Run tests**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib telegram)`
Expected: All existing tests pass (convert_message/extract_attachments are async and not directly tested, but channel creation and parse tests remain)

- [ ] **Step 5: Commit**

```bash
(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && git add src/gateway/interfaces/telegram/handlers.rs src/gateway/interfaces/telegram/mod.rs && git commit -m "telegram: extract handlers.rs — convert_message + extract_attachments")
```

---

## Task 2: Extract `delivery.rs` — send logic, chunking, retry, attachments

**Files:**
- Create: `src/gateway/interfaces/telegram/delivery.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

This extracts `ErrorClass`, `classify_error()`, the `send()` chunking+retry loop, `send_typing()`, `react()`, `send_attachment()`, and `edit_message()` into `delivery.rs`.

- [ ] **Step 1: Create `delivery.rs` with all send-related code**

Move from mod.rs:
- `ErrorClass` enum + `classify_error()` fn (lines 366-394)
- The core send logic (lines 856-1049) as a `pub(crate) async fn send_message()`
- `send_typing()` logic (lines 1051-1069) as `pub(crate) async fn send_typing()`
- `react()` logic (lines 1071-1105) as `pub(crate) async fn send_reaction()`
- `send_attachment()` (lines 1127-1193) as `pub(crate) async fn send_attachment()`
- `edit_message()` (lines 1202-1280) as `pub(crate) async fn edit_message()`
- The `with_thread!` macro (lines 1149-1159)

All functions take `bot: &Bot` and other needed params explicitly instead of `&self`.

```rust
// delivery.rs — key signatures
pub(crate) async fn send_message(
    bot: &Bot,
    config: &TelegramConfig,
    conversation_id: &str,
    message: &OutboundMessage,
) -> ChannelResult<SendResult> { ... }

pub(crate) async fn send_typing(
    bot: &Bot,
    conversation_id: &str,
) -> ChannelResult<()> { ... }

pub(crate) async fn send_reaction(
    bot: &Bot,
    conversation_id: &str,
    message_id: &MessageId,
    reaction: &str,
) -> ChannelResult<()> { ... }

pub(crate) async fn send_attachment(
    bot: &Bot,
    chat_id: ChatId,
    thread_id: Option<i32>,
    attachment: &Attachment,
) -> ChannelResult<()> { ... }

pub(crate) async fn edit_message(
    bot: &Bot,
    conversation_id: &str,
    message_id: &MessageId,
    new_text: Option<&str>,
    keyboard: Option<&InlineKeyboard>,
) -> ChannelResult<()> { ... }

/// Shared helper: parse conversation_id into (ChatId, Option<thread_id>)
pub(crate) fn parse_conversation_id(conv_id: &str) -> (ChatId, Option<i32>) { ... }
```

Note: `parse_conversation_id` moves here since delivery is its primary consumer. `mod.rs` can re-export or call `delivery::parse_conversation_id`.

- [ ] **Step 2: Update `mod.rs` — delegate Channel trait methods to delivery**

```rust
// mod.rs Channel impl becomes delegation:
async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
    let bot = self.bot.as_ref()
        .ok_or_else(|| ChannelError::NotConnected("Bot not initialized".into()))?;
    delivery::send_message(bot, &self.config, message.conversation_id.as_str(), &message).await
}

async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
    let bot = self.bot.as_ref()
        .ok_or_else(|| ChannelError::NotConnected("Bot not initialized".into()))?;
    delivery::send_typing(bot, conversation_id.as_str()).await
}
// ... etc for react, edit, delete
```

- [ ] **Step 3: Run check**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo check -p alephcore)`
Expected: Compiles

- [ ] **Step 4: Run tests**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib telegram)`
Expected: All existing tests pass. `test_parse_conversation_id_*` tests move to `delivery.rs` or call `delivery::parse_conversation_id`.

- [ ] **Step 5: Commit**

```bash
(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && git add src/gateway/interfaces/telegram/delivery.rs src/gateway/interfaces/telegram/mod.rs && git commit -m "telegram: extract delivery.rs — send, retry, chunking, attachments")
```

---

## Task 3: Extract `polling.rs` — polling lifecycle + watchdog

**Files:**
- Create: `src/gateway/interfaces/telegram/polling.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

This extracts the polling loop from `start()` (lines 564-836) into a standalone async function.

- [ ] **Step 1: Create `polling.rs`**

```rust
// polling.rs
use std::time::{Duration, Instant};
use teloxide::prelude::*;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio_util::sync::CancellationToken;
use crate::gateway::channel::{ChannelStatus, InboundMessage, CallbackQuery};

/// Run the Telegram long-polling loop with health check watchdog.
///
/// This function blocks until shutdown is requested or an unrecoverable error occurs.
/// On transient failures, it auto-restarts with exponential backoff.
pub(crate) async fn run_polling_loop(
    bot: Bot,
    handler: teloxide::dispatching::UpdateHandler<std::convert::Infallible>,
    status: std::sync::Arc<RwLock<ChannelStatus>>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    // ... (exact code from current mod.rs lines 564-836, with the handler
    //      construction moved to the caller — this fn receives the built handler)
}
```

The key insight: the `message_handler` and `callback_handler` closures are built in `start()` using handler-specific Arc clones. We keep closure construction in `mod.rs::start()` (or `handlers.rs`) and pass the composed `dptree` handler to `run_polling_loop`.

- [ ] **Step 2: Refactor `start()` in mod.rs**

`start()` now:
1. Creates bot, verifies token, registers commands (unchanged)
2. Builds handler closures (calling `handlers::convert_message` etc.)
3. Composes the dptree handler
4. Calls `polling::run_polling_loop(bot, handler, status, shutdown_rx)` in a spawned task

```rust
async fn start(&mut self) -> ChannelResult<()> {
    // ... validation, bot creation, slash commands (unchanged) ...

    let handler = dptree::entry()
        .branch(/* message_handler */)
        .branch(/* callback_handler */);

    tokio::spawn(polling::run_polling_loop(
        bot.clone(), handler, status, shutdown_rx,
    ));

    self.set_status(ChannelStatus::Connected).await;
    Ok(())
}
```

- [ ] **Step 3: Run check + tests**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo check -p alephcore && cargo test -p alephcore --lib telegram)`
Expected: Compiles and all tests pass

- [ ] **Step 4: Commit**

```bash
(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && git add src/gateway/interfaces/telegram/polling.rs src/gateway/interfaces/telegram/mod.rs && git commit -m "telegram: extract polling.rs — polling lifecycle + watchdog")
```

---

## Task 4: Verify mod.rs is now slim + cleanup

**Files:**
- Modify: `src/gateway/interfaces/telegram/mod.rs`

After Tasks 1-3, `mod.rs` should contain only:
- Module declarations (`pub mod config/handlers/delivery/polling/...`)
- `TelegramChannel` struct definition (~30 lines)
- `Channel` trait impl with one-line delegations (~80 lines)
- `TelegramChannelFactory` (~20 lines)
- `take_callback_receiver()` (~5 lines)

- [ ] **Step 1: Audit mod.rs line count**

Run: `wc -l /Users/zouguojun/Workspace/Aleph-telegram-overhaul/src/gateway/interfaces/telegram/mod.rs`
Expected: Under 200 lines. If over, identify remaining code that should be in submodules.

- [ ] **Step 2: Move tests to their respective modules**

Move `test_parse_conversation_id_*` tests to `delivery.rs` (since `parse_conversation_id` lives there now). Keep `test_channel_capabilities` and `test_channel_creation` in `mod.rs`.

- [ ] **Step 3: Run full test suite**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib telegram)`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && git add -u src/gateway/interfaces/telegram/ && git commit -m "telegram: finalize module split — mod.rs is now delegation-only")
```

---

## Task 5: Upgrade message chunking — HTML-safe splitting

**Files:**
- Modify: `src/gateway/interfaces/telegram/delivery.rs`

Replace the simple `MessageFormatter::split()` call with HTML-aware chunking.

- [ ] **Step 1: Write failing tests for HTML-safe splitting**

Add to `delivery.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_html_safe_short_text() {
        let chunks = split_html_safe("<b>hello</b>", 4096);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "<b>hello</b>");
    }

    #[test]
    fn test_split_html_safe_balances_bold() {
        // Create text that exceeds limit inside a <b> tag
        let inner = "x".repeat(100);
        let html = format!("<b>{}</b>", inner);
        let chunks = split_html_safe(&html, 60);
        assert!(chunks.len() >= 2);
        // First chunk should close the <b> tag
        assert!(chunks[0].ends_with("</b>"));
        // Second chunk should reopen <b>
        assert!(chunks[1].starts_with("<b>"));
    }

    #[test]
    fn test_split_html_safe_prefers_newline() {
        let html = format!("{}\n{}", "a".repeat(50), "b".repeat(50));
        let chunks = split_html_safe(&html, 60);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].trim(), &"a".repeat(50));
    }

    #[test]
    fn test_split_html_safe_nested_tags() {
        let inner = "x".repeat(100);
        let html = format!("<b><i>{}</i></b>", inner);
        let chunks = split_html_safe(&html, 60);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].ends_with("</i></b>"));
        assert!(chunks[1].starts_with("<b><i>"));
    }

    #[test]
    fn test_split_html_safe_utf8_safety() {
        // Chinese characters are 3 bytes each, 4 chars = 12 bytes
        let text = "你好世界".repeat(30); // 120 chars, 360 bytes
        let chunks = split_html_safe(&text, 50);
        assert!(chunks.len() >= 2);
        // All chunks must be valid UTF-8 and within char limit (not byte limit)
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 55); // 50 + small overhead
        }
    }

    #[test]
    fn test_balance_html_tags_no_tags() {
        let (close, open) = balance_html_tags("hello world");
        assert!(close.is_empty());
        assert!(open.is_empty());
    }

    #[test]
    fn test_balance_html_tags_unclosed() {
        let (close, open) = balance_html_tags("<b>hello");
        assert_eq!(close, vec!["b"]);
        assert_eq!(open, vec!["b"]);
    }

    #[test]
    fn test_balance_html_tags_closed() {
        let (close, open) = balance_html_tags("<b>hello</b>");
        assert!(close.is_empty());
        assert!(open.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib telegram::delivery)`
Expected: FAIL — `split_html_safe` and `balance_html_tags` not defined yet

- [ ] **Step 3: Implement `balance_html_tags` and `split_html_safe`**

```rust
/// Telegram-supported HTML tags (self-closing tags excluded)
const TG_TAGS: &[&str] = &["b", "i", "s", "u", "code", "pre", "blockquote", "tg-spoiler"];

/// Analyze a chunk for unclosed HTML tags.
/// Returns (tags_to_close, tags_to_reopen) — both in correct order.
fn balance_html_tags(chunk: &str) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut stack: Vec<&'static str> = Vec::new();

    // Simple state machine: find < ... > sequences
    let bytes = chunk.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let is_closing = i + 1 < bytes.len() && bytes[i + 1] == b'/';
            let start = if is_closing { i + 2 } else { i + 1 };
            // Find tag name end (space or >)
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'>' && bytes[end] != b' ' {
                end += 1;
            }
            if end <= bytes.len() {
                let tag_name = &chunk[start..end];
                if let Some(&canonical) = TG_TAGS.iter().find(|&&t| t.eq_ignore_ascii_case(tag_name)) {
                    if is_closing {
                        // Pop matching open tag
                        if let Some(pos) = stack.iter().rposition(|&t| t == canonical) {
                            stack.remove(pos);
                        }
                    } else {
                        stack.push(canonical);
                    }
                }
            }
            // Skip to after >
            while i < bytes.len() && bytes[i] != b'>' {
                i += 1;
            }
        }
        i += 1;
    }

    // stack contains unclosed tags in open order
    // Close in reverse order, reopen in original order
    let reopen = stack.clone();
    let mut close = stack;
    close.reverse();
    (close, reopen)
}

/// Split HTML text into chunks respecting tag integrity and Telegram's char limit.
/// `max_len` is measured in chars (Rust char count), conservative for Telegram's 4096 UTF-16 limit.
/// Reserves TAG_OVERHEAD chars for closing/reopening HTML tags at chunk boundaries.
const TAG_OVERHEAD: usize = 200;

pub(crate) fn split_html_safe(html: &str, max_len: usize) -> Vec<String> {
    if html.chars().count() <= max_len {
        return vec![html.to_string()];
    }

    let effective_limit = max_len.saturating_sub(TAG_OVERHEAD);
    let mut chunks = Vec::new();
    let mut remaining = html.to_string();

    while !remaining.is_empty() {
        if remaining.chars().count() <= max_len {
            chunks.push(remaining);
            break;
        }

        // Find split point using priority: \n\n > \n > space > hard cut
        // Use effective_limit (max_len minus tag overhead) for the split point
        let char_indices: Vec<(usize, char)> = remaining.char_indices().collect();
        let limit_byte = if effective_limit < char_indices.len() {
            char_indices[effective_limit].0
        } else {
            remaining.len()
        };
        let search = &remaining[..limit_byte];

        let split_byte = if let Some(pos) = search.rfind("\n\n") {
            if pos > limit_byte / 4 { pos + 1 } else { find_any_split(search, limit_byte) }
        } else {
            find_any_split(search, limit_byte)
        };

        // Helper: find fallback split point (newline > space > hard cut)
        fn find_any_split(search: &str, limit: usize) -> usize {
            search.rfind('\n')
                .filter(|&p| p > limit / 4)
                .or_else(|| search.rfind(' ').filter(|&p| p > limit / 4))
                .unwrap_or(limit)
        }

        // Split at split_byte, balance tags, push chunk
        let raw_chunk = &remaining[..split_byte];
        let rest = remaining[split_byte..].trim_start_matches('\n');

        let (close_tags, reopen_tags) = balance_html_tags(raw_chunk);

        let mut chunk = raw_chunk.to_string();
        for tag in &close_tags {
            chunk.push_str(&format!("</{}>", tag));
        }
        chunks.push(chunk);

        let mut next = String::new();
        for tag in &reopen_tags {
            next.push_str(&format!("<{}>", tag));
        }
        next.push_str(rest);
        remaining = next;
    }

    chunks
}
```

- [ ] **Step 4: Run tests**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib telegram::delivery)`
Expected: All chunking tests PASS

- [ ] **Step 5: Update `send_message` to use `split_html_safe`**

In `delivery.rs`, change the send flow:
```rust
// Old:
let chunks = MessageFormatter::split(&message.text, SPLIT_LIMIT);
// ... for each chunk: format to HTML then send

// New:
let html_text = MessageFormatter::format(&message.text, MarkupFormat::TelegramHtml);
let chunks = split_html_safe(&html_text, 4000);
// ... for each chunk: send directly (already HTML)
```

- [ ] **Step 6: Run check + tests**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo check -p alephcore && cargo test -p alephcore --lib telegram)`
Expected: Compiles and all tests pass

- [ ] **Step 7: Commit**

```bash
(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && git add src/gateway/interfaces/telegram/delivery.rs && git commit -m "telegram: add HTML-safe message chunking with tag balancing")
```

---

## Task 6: Upgrade access control — DmPolicy/GroupPolicy + AccessController

**Files:**
- Modify: `src/gateway/interfaces/telegram/config.rs`
- Create: `src/gateway/interfaces/telegram/access.rs`
- Modify: `src/gateway/interfaces/telegram/handlers.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs`

- [ ] **Step 1: Add DmPolicy and GroupPolicy enums to config.rs**

```rust
// config.rs — add before TelegramConfig struct

/// DM access policy
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    Disabled,
    #[default]
    Pairing,
    Allowlist,
    Open,
}

/// Group access policy
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    Disabled,
    #[default]
    Allowlist,
    Open,
}
```

Add fields to `TelegramConfig`:
```rust
/// DM access policy (default: pairing)
#[serde(default)]
pub dm_policy: DmPolicy,

/// Group access policy (default: allowlist)
#[serde(default)]
pub group_policy: GroupPolicy,
```

Update `Default` impl and add backward-compat method:
```rust
impl TelegramConfig {
    /// Resolve effective DM policy considering backward compatibility.
    /// If dm_policy is explicitly set, use it.
    /// Otherwise infer from allowed_users: non-empty → Allowlist, empty → Pairing.
    pub fn effective_dm_policy(&self) -> DmPolicy {
        // If dm_policy was explicitly set (not default), use it
        // Since serde defaults to Pairing, check if allowed_users suggests otherwise
        if !self.allowed_users.is_empty() && self.dm_policy == DmPolicy::Pairing {
            DmPolicy::Allowlist
        } else {
            self.dm_policy.clone()
        }
    }

    /// Resolve effective group policy.
    pub fn effective_group_policy(&self) -> GroupPolicy {
        if !self.groups_allowed {
            return GroupPolicy::Disabled;
        }
        if !self.allowed_groups.is_empty() && self.group_policy == GroupPolicy::Allowlist {
            GroupPolicy::Allowlist
        } else if self.group_policy == GroupPolicy::Allowlist && self.allowed_groups.is_empty() {
            GroupPolicy::Open
        } else {
            self.group_policy.clone()
        }
    }
}
```

- [ ] **Step 2: Write tests for policy resolution**

```rust
#[test]
fn test_effective_dm_policy_default() {
    let config = TelegramConfig::default();
    assert_eq!(config.effective_dm_policy(), DmPolicy::Pairing);
}

#[test]
fn test_effective_dm_policy_with_allowlist() {
    let mut config = TelegramConfig::default();
    config.allowed_users = vec![123];
    assert_eq!(config.effective_dm_policy(), DmPolicy::Allowlist);
}

#[test]
fn test_effective_dm_policy_explicit_open() {
    let mut config = TelegramConfig::default();
    config.dm_policy = DmPolicy::Open;
    assert_eq!(config.effective_dm_policy(), DmPolicy::Open);
}

#[test]
fn test_effective_group_policy_disabled() {
    let mut config = TelegramConfig::default();
    config.groups_allowed = false;
    assert_eq!(config.effective_group_policy(), GroupPolicy::Disabled);
}
```

- [ ] **Step 3: Run tests**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib telegram::config)`
Expected: PASS

- [ ] **Step 4: Create `access.rs` with AccessController**

```rust
// access.rs
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use super::config::{DmPolicy, GroupPolicy, PairingEntry, TelegramConfig};

#[derive(Debug, PartialEq)]
pub enum AccessDecision {
    Allowed,
    NeedsPairing,
    Denied,
}

#[derive(Debug, PartialEq)]
pub enum PairingResult {
    Success,
    Expired,
    InvalidCode,
}

pub struct AccessController {
    config: TelegramConfig,
    runtime_users: Arc<RwLock<Vec<i64>>>,
    pairing_codes: Arc<RwLock<HashMap<String, PairingEntry>>>,
    prompt_times: Arc<RwLock<HashMap<i64, Instant>>>,
}

impl AccessController {
    pub fn new(config: TelegramConfig) -> Self { ... }

    /// Check whether a message from user_id in chat_id should be allowed.
    pub async fn check_message(&self, user_id: i64, chat_id: i64, is_group: bool) -> AccessDecision {
        if is_group {
            self.check_group(chat_id)
        } else {
            self.check_dm(user_id).await
        }
    }

    async fn check_dm(&self, user_id: i64) -> AccessDecision {
        match self.config.effective_dm_policy() {
            DmPolicy::Disabled => AccessDecision::Denied,
            DmPolicy::Open => AccessDecision::Allowed,
            DmPolicy::Allowlist => {
                if self.config.allowed_users.contains(&user_id)
                    || self.runtime_users.read().await.contains(&user_id)
                {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::Denied
                }
            }
            DmPolicy::Pairing => {
                if self.config.allowed_users.contains(&user_id)
                    || self.runtime_users.read().await.contains(&user_id)
                {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::NeedsPairing
                }
            }
        }
    }

    fn check_group(&self, chat_id: i64) -> AccessDecision {
        match self.config.effective_group_policy() {
            GroupPolicy::Disabled => AccessDecision::Denied,
            GroupPolicy::Open => AccessDecision::Allowed,
            GroupPolicy::Allowlist => {
                if self.config.allowed_groups.contains(&chat_id) {
                    AccessDecision::Allowed
                } else {
                    AccessDecision::Denied
                }
            }
        }
    }

    pub async fn try_pair(&self, user_id: i64, code: &str) -> PairingResult { ... }
    pub async fn generate_code(&self) -> String { ... }
    pub async fn list_codes(&self) -> Vec<(String, u64)> { ... }

    // Expose Arc handles for handler closures
    pub fn runtime_users(&self) -> &Arc<RwLock<Vec<i64>>> { &self.runtime_users }
    pub fn pairing_codes(&self) -> &Arc<RwLock<HashMap<String, PairingEntry>>> { &self.pairing_codes }
}
```

- [ ] **Step 5: Write tests for AccessController**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(dm: DmPolicy, users: Vec<i64>) -> TelegramConfig {
        TelegramConfig { dm_policy: dm, allowed_users: users, ..Default::default() }
    }

    #[tokio::test]
    async fn test_dm_disabled() {
        let ctrl = AccessController::new(make_config(DmPolicy::Disabled, vec![]));
        assert_eq!(ctrl.check_message(123, 123, false).await, AccessDecision::Denied);
    }

    #[tokio::test]
    async fn test_dm_open() {
        let ctrl = AccessController::new(make_config(DmPolicy::Open, vec![]));
        assert_eq!(ctrl.check_message(123, 123, false).await, AccessDecision::Allowed);
    }

    #[tokio::test]
    async fn test_dm_pairing_unknown_user() {
        let ctrl = AccessController::new(make_config(DmPolicy::Pairing, vec![]));
        assert_eq!(ctrl.check_message(123, 123, false).await, AccessDecision::NeedsPairing);
    }

    #[tokio::test]
    async fn test_dm_pairing_after_pair() {
        let ctrl = AccessController::new(make_config(DmPolicy::Pairing, vec![]));
        let code = ctrl.generate_code().await;
        assert_eq!(ctrl.try_pair(123, &code).await, PairingResult::Success);
        assert_eq!(ctrl.check_message(123, 123, false).await, AccessDecision::Allowed);
    }

    #[tokio::test]
    async fn test_dm_allowlist_allowed() {
        let ctrl = AccessController::new(make_config(DmPolicy::Allowlist, vec![123]));
        assert_eq!(ctrl.check_message(123, 123, false).await, AccessDecision::Allowed);
    }

    #[tokio::test]
    async fn test_dm_allowlist_denied() {
        let ctrl = AccessController::new(make_config(DmPolicy::Allowlist, vec![123]));
        assert_eq!(ctrl.check_message(999, 999, false).await, AccessDecision::Denied);
    }
}
```

- [ ] **Step 6: Run tests**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib telegram::access)`
Expected: All access control tests PASS

- [ ] **Step 7: Integrate AccessController into handlers.rs and mod.rs**

Replace `TelegramChannel`'s three Arc fields (`pairing_codes`, `pairing_prompt_times`, `runtime_allowed_users`) with a single `access: Arc<AccessController>`.

Update `handlers::convert_message` to take `&AccessController` instead of separate `config + runtime_users`.

Update the pairing logic in the message_handler closure to call `access.try_pair()`.

Update `Channel::get_pairing_data()` and `list_active_pairing_codes()` to delegate to `access`.

- [ ] **Step 8: Run check + full tests**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo check -p alephcore && cargo test -p alephcore --lib telegram)`
Expected: Compiles and all tests pass

- [ ] **Step 9: Commit**

```bash
(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && git add src/gateway/interfaces/telegram/config.rs src/gateway/interfaces/telegram/access.rs src/gateway/interfaces/telegram/handlers.rs src/gateway/interfaces/telegram/mod.rs && git commit -m "telegram: add policy-based access control — DmPolicy/GroupPolicy + AccessController")
```

---

## Task 7: Upgrade network resilience — enhanced error classification

**Files:**
- Modify: `src/gateway/interfaces/telegram/delivery.rs`
- Modify: `src/gateway/interfaces/telegram/polling.rs`

- [ ] **Step 1: Write tests for new error classification**

```rust
// In delivery.rs tests
#[test]
fn test_classify_rate_limited() {
    // Note: teloxide_core::types::Seconds wraps Duration.
    // Verify the From<Duration> impl exists at compile time; if not,
    // use the Seconds constructor directly from teloxide_core.
    use teloxide::ApiError;
    let seconds = teloxide::types::Seconds::from_seconds(30);
    let err = teloxide::RequestError::Api(ApiError::RetryAfter(seconds));
    match classify_error(&err) {
        ErrorClass::RateLimited(secs) => assert_eq!(secs, 30),
        other => panic!("Expected RateLimited, got {:?}", other),
    }
}

#[test]
fn test_classify_bot_blocked() {
    use teloxide::ApiError;
    let err = teloxide::RequestError::Api(ApiError::BotBlocked);
    match classify_error(&err) {
        ErrorClass::Rejected(_) => {}
        other => panic!("Expected Rejected, got {:?}", other),
    }
}
```

- [ ] **Step 2: Replace ErrorClass with enhanced version**

```rust
#[derive(Debug)]
pub(crate) enum ErrorClass {
    PreConnect,
    PostConnect,
    Rejected(String),
    RateLimited(u64),
}

pub(crate) fn classify_error(err: &teloxide::RequestError) -> ErrorClass {
    match err {
        teloxide::RequestError::Api(api_err) => {
            use teloxide::ApiError;
            match api_err {
                ApiError::RetryAfter(seconds) => ErrorClass::RateLimited(seconds.as_secs()),
                ApiError::BotBlocked
                | ApiError::ChatNotFound
                | ApiError::UserNotFound => ErrorClass::Rejected(api_err.to_string()),
                _ => {
                    let msg = api_err.to_string();
                    if msg.contains("Unauthorized") || msg.contains("Bad Request") {
                        ErrorClass::Rejected(msg)
                    } else {
                        ErrorClass::PostConnect
                    }
                }
            }
        }
        teloxide::RequestError::Network(reqwest_err) => {
            if reqwest_err.is_connect() {
                ErrorClass::PreConnect  // DNS/TCP failure — data never sent
            } else {
                ErrorClass::PostConnect // timeout, reset, etc. — data may have been sent
            }
        }
        _ => ErrorClass::PostConnect,
    }
}
```

- [ ] **Step 3: Update retry loop in `send_message` to use new classification**

Apply the retry strategy matrix from the spec:
- PreConnect: immediate retry with backoff
- PostConnect: max 2 retries
- Rejected: fallback to plain text, no retry
- RateLimited(n): sleep exact `n` seconds

- [ ] **Step 4: Add `PollingState` to polling.rs**

```rust
struct PollingState {
    attempt: u32,
    healthy_since: Option<Instant>,
    last_update_at: Instant,
}
```

Use `last_update_at` in the watchdog: if no updates for 90s AND health check fails, trigger restart.

- [ ] **Step 5: Run tests**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib telegram)`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && git add src/gateway/interfaces/telegram/delivery.rs src/gateway/interfaces/telegram/polling.rs && git commit -m "telegram: upgrade error classification — pre/post connect + precise 429 handling")
```

---

## Task 8: Create `streaming.rs` — StreamingController (pure logic)

**Files:**
- Create: `src/gateway/streaming.rs` (in gateway/, NOT telegram/ — this is channel-agnostic)
- Modify: `src/gateway/mod.rs` (add `pub mod streaming;`)

Note: StreamingController is pure logic with zero Telegram dependency. Placing it in `gateway/` avoids coupling the generic ReplyEmitter to a specific channel (P1 low coupling).

- [ ] **Step 1: Write comprehensive tests for StreamingController**

```rust
// streaming.rs
#[cfg(test)]
mod tests {
    use super::*;

    fn make_controller(enabled: bool) -> StreamingController {
        StreamingController::new(StreamingConfig {
            min_initial_chars: 30,
            debounce_interval: Duration::from_millis(300),
            enabled,
        })
    }

    #[test]
    fn test_wait_until_threshold() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk("Hi");
        assert!(matches!(ctrl.poll_action(), StreamAction::Wait));
    }

    #[test]
    fn test_send_initial_at_threshold() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        match ctrl.poll_action() {
            StreamAction::SendInitial(text) => assert_eq!(text.len(), 35),
            other => panic!("Expected SendInitial, got {:?}", other),
        }
    }

    #[test]
    fn test_edit_after_debounce() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        let _ = ctrl.poll_action(); // SendInitial
        ctrl.record_sent(MessageId::new("1"));

        ctrl.push_chunk("more");
        // Before debounce: Wait
        assert!(matches!(ctrl.poll_action(), StreamAction::Wait));

        // Simulate debounce elapsed
        ctrl.last_edit_at = Instant::now() - Duration::from_millis(400);
        match ctrl.poll_action() {
            StreamAction::Edit(text) => assert!(text.contains("more")),
            other => panic!("Expected Edit, got {:?}", other),
        }
    }

    #[test]
    fn test_finalize_without_send() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk("short");
        match ctrl.finalize() {
            StreamAction::SendFinal(text) => assert_eq!(text, "short"),
            other => panic!("Expected SendFinal, got {:?}", other),
        }
    }

    #[test]
    fn test_finalize_with_pending_edit() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        let _ = ctrl.poll_action();
        ctrl.record_sent(MessageId::new("1"));
        ctrl.push_chunk("tail");
        match ctrl.finalize() {
            StreamAction::EditFinal(text) => assert!(text.ends_with("tail")),
            other => panic!("Expected EditFinal, got {:?}", other),
        }
    }

    #[test]
    fn test_disabled_mode() {
        let mut ctrl = make_controller(false);
        ctrl.push_chunk(&"x".repeat(100));
        // Disabled: poll_action always Wait
        assert!(matches!(ctrl.poll_action(), StreamAction::Wait));
        // Finalize: SendFinal (one-shot)
        match ctrl.finalize() {
            StreamAction::SendFinal(text) => assert_eq!(text.len(), 100),
            other => panic!("Expected SendFinal, got {:?}", other),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib gateway::streaming)`
Expected: FAIL — StreamingController not defined

- [ ] **Step 3: Implement StreamingController**

```rust
// src/gateway/streaming.rs
use std::time::{Duration, Instant};
use crate::gateway::channel::MessageId;

#[derive(Debug, Clone)]
pub struct StreamingConfig {
    pub min_initial_chars: usize,
    pub debounce_interval: Duration,
    pub enabled: bool,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            min_initial_chars: 30,
            debounce_interval: Duration::from_millis(300),
            enabled: false,
        }
    }
}

#[derive(Debug)]
pub enum StreamAction {
    Wait,
    SendInitial(String),
    Edit(String),
    SendFinal(String),
    EditFinal(String),
    Done,
}

pub struct StreamingController {
    buffer: String,
    sent_message_id: Option<MessageId>,
    pub(crate) last_edit_at: Instant,  // pub(crate) for test manipulation
    last_edit_len: usize,
    config: StreamingConfig,
}

impl StreamingController {
    pub fn new(config: StreamingConfig) -> Self {
        Self {
            buffer: String::new(),
            sent_message_id: None,
            last_edit_at: Instant::now(),
            last_edit_len: 0,
            config,
        }
    }

    pub fn push_chunk(&mut self, text: &str) {
        self.buffer.push_str(text);
    }

    pub fn record_sent(&mut self, msg_id: MessageId) {
        self.sent_message_id = Some(msg_id);
        self.last_edit_at = Instant::now();
        self.last_edit_len = self.buffer.len();
    }

    pub fn record_edit(&mut self) {
        self.last_edit_at = Instant::now();
        self.last_edit_len = self.buffer.len();
    }

    pub fn message_id(&self) -> Option<&MessageId> {
        self.sent_message_id.as_ref()
    }

    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    pub fn poll_action(&mut self) -> StreamAction {
        if !self.config.enabled {
            return StreamAction::Wait; // Disabled: wait for finalize()
        }

        if self.sent_message_id.is_none() {
            if self.buffer.chars().count() >= self.config.min_initial_chars {
                StreamAction::SendInitial(self.buffer.clone())
            } else {
                StreamAction::Wait
            }
        } else if self.last_edit_at.elapsed() >= self.config.debounce_interval
            && self.buffer.len() > self.last_edit_len
        {
            StreamAction::Edit(self.buffer.clone())
        } else {
            StreamAction::Wait
        }
    }

    pub fn finalize(&mut self) -> StreamAction {
        if self.buffer.is_empty() {
            return StreamAction::Done;
        }

        if self.sent_message_id.is_none() {
            StreamAction::SendFinal(self.buffer.clone())
        } else if self.buffer.len() > self.last_edit_len {
            StreamAction::EditFinal(self.buffer.clone())
        } else {
            StreamAction::Done
        }
    }
}
```

- [ ] **Step 4: Add module declaration, run tests**

Add `pub mod streaming;` in `src/gateway/mod.rs`.

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib gateway::streaming)`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && git add src/gateway/streaming.rs src/gateway/mod.rs && git commit -m "gateway: add StreamingController — pure logic real-time streaming state machine")
```

---

## Task 9: Integrate StreamingController into ReplyEmitter

**Files:**
- Modify: `src/gateway/reply_emitter.rs`

This is the highest-impact change. Replace the typewriter edit loop with StreamingController-driven real-time streaming.

- [ ] **Step 1: Add StreamingController field to ReplyEmitter**

```rust
// In ReplyEmitter struct, add:
use crate::gateway::streaming::{StreamingController, StreamingConfig, StreamAction};

pub struct ReplyEmitter {
    // ... existing fields ...
    /// Real-time streaming controller (replaces typewriter mode)
    streaming: Mutex<StreamingController>,
}
```

Initialize in `new()` and `with_config()`:
```rust
streaming: Mutex::new(StreamingController::new(StreamingConfig {
    min_initial_chars: 30,
    debounce_interval: Duration::from_millis(300),
    enabled: config.stream_enabled,
})),
```

- [ ] **Step 2: Replace typewriter path in `emit()` with streaming**

In `StreamEvent::ResponseChunk` (non-intermediate):
```rust
// After buffering content into self.buffer:
if self.config.stream_enabled {
    let mut ctrl = self.streaming.lock().await;
    ctrl.push_chunk(&content);
    match ctrl.poll_action() {
        StreamAction::SendInitial(text) => {
            let msg = OutboundMessage::text(
                self.route.conversation_id.as_str(), &text
            ).with_reply_to_opt(self.route.reply_to.clone());
            if let Ok(result) = self.channel_registry.send(&self.route.channel_id, msg).await {
                ctrl.record_sent(result.message_id);
                self.has_sent.store(true, Ordering::SeqCst);
            }
        }
        StreamAction::Edit(text) => {
            if let Some(msg_id) = ctrl.message_id() {
                if self.channel_registry.edit(
                    &self.route.channel_id,
                    &self.route.conversation_id,
                    msg_id, &text,
                ).await.is_ok() {
                    ctrl.record_edit(); // Only reset debounce on successful edit
                }
            }
        }
        StreamAction::Wait => {}
        _ => {}
    }
}
```

In `StreamEvent::RunComplete`:
```rust
if self.config.stream_enabled {
    let mut ctrl = self.streaming.lock().await;
    match ctrl.finalize() {
        StreamAction::SendFinal(text) => {
            // Voice check, then send
            if self.should_voice().await {
                self.send_as_voice(&text).await;
            } else {
                self.send_to_channel(&text).await;
            }
        }
        StreamAction::EditFinal(text) => {
            if let Some(msg_id) = ctrl.message_id() {
                let _ = self.channel_registry.edit(
                    &self.route.channel_id,
                    &self.route.conversation_id,
                    msg_id, &text,
                ).await;
            }
        }
        StreamAction::Done => {}
        _ => {}
    }
    // Send media
    let media = self.drain_and_send_media().await;
    self.send_media_standalone(media).await;
} else {
    // Instant mode (unchanged)
    // ...
}
```

- [ ] **Step 3: Delete old typewriter code**

Remove from `reply_emitter.rs`:
- `const TYPEWRITER_CHARS_PER_STEP`
- `async fn send_typewriter()` (entire method, ~160 lines)
- All `self.send_typewriter()` call sites (replace with streaming path)
- Note: keep `send_to_channel()` — it's still used for instant mode and voice fallback

- [ ] **Step 4: Run check**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo check -p alephcore)`
Expected: Compiles with no errors

- [ ] **Step 5: Run tests**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib)`
Expected: All tests pass (reply_emitter has no unit tests for typewriter, so removal is safe)

- [ ] **Step 6: Commit**

```bash
(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && git add src/gateway/reply_emitter.rs && git commit -m "gateway: integrate real-time streaming into ReplyEmitter, remove typewriter mode")
```

---

## Task 10: Final cleanup + merge preparation

**Files:**
- All files in `src/gateway/interfaces/telegram/`
- `src/gateway/reply_emitter.rs`

- [ ] **Step 1: Run clippy**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo clippy -p alephcore -- -D warnings 2>&1 | head -50)`
Expected: No warnings in telegram/ or reply_emitter.rs files

- [ ] **Step 2: Run full test suite**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && cargo test -p alephcore --lib)`
Expected: All tests pass

- [ ] **Step 3: Verify mod.rs line count**

Run: `wc -l /Users/zouguojun/Workspace/Aleph-telegram-overhaul/src/gateway/interfaces/telegram/*.rs`
Expected: mod.rs < 200 lines, each submodule < 400 lines

- [ ] **Step 4: Verify no dead code**

Run: `(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && grep -rn "TYPEWRITER_CHARS_PER_STEP\|send_typewriter" src/)`
Expected: No matches (old typewriter code fully removed)

- [ ] **Step 5: Commit any cleanup**

```bash
(cd /Users/zouguojun/Workspace/Aleph-telegram-overhaul && git add -u src/ && git commit -m "telegram: final cleanup — remove dead code, fix clippy warnings")
```

- [ ] **Step 6: Merge to main**

```bash
# From main repo (not worktree!)
cd /Users/zouguojun/Workspace/Aleph
git merge feat/telegram-overhaul --no-edit
```
