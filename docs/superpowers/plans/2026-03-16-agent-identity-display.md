# Agent Identity Display Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Display agent identity in channel responses so users always know which agent they're talking to.

**Architecture:** Add `agent_display_name` field to `ReplyEmitter` and `OutboundMessage`. ReplyEmitter centralizes prefix logic (`[AgentName] message`) on first send, gated by `has_sent` and a `NativeIdentity` capability check. Webchat doesn't go through ReplyEmitter (uses WebSocket directly), so it naturally gets no prefix.

**Tech Stack:** Rust, async_trait, tokio

**Spec:** `docs/superpowers/specs/2026-03-16-agent-identity-display-design.md`

---

## Key Insight: Webchat Path

Webchat communicates via WebSocket through the HTTP server — it does NOT use the Channel trait or ReplyEmitter. Therefore:
- ReplyEmitter prefix logic only affects Channel-based interfaces (Telegram, Slack, Discord, CLI, etc.)
- Webchat naturally gets no prefix — no special handling needed
- We add `NativeIdentity` to the `Capability` enum for future extensibility
- ReplyEmitter checks `channel_has_native_identity()` via ChannelRegistry before applying prefix — if any future channel declares `NativeIdentity`, it will be respected

---

## Chunk 1: Core Data Model & Capability

### Task 1: Add NativeIdentity capability variant

**Files:**
- Modify: `src/thinker/interaction.rs:88-109` (Capability enum)

- [ ] **Step 1: Write the test**

Add to the existing test module in `src/thinker/interaction.rs`:

```rust
#[test]
fn test_native_identity_capability() {
    let (name, hint) = Capability::NativeIdentity.prompt_hint();
    assert_eq!(name, "native_identity");
    assert!(hint.contains("identity"));
}

#[test]
fn test_native_identity_serde_roundtrip() {
    let cap = Capability::NativeIdentity;
    let json = serde_json::to_string(&cap).expect("serialize");
    let deserialized: Capability = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, Capability::NativeIdentity);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib interaction::tests::test_native_identity`
Expected: FAIL — `NativeIdentity` variant does not exist

- [ ] **Step 3: Add the NativeIdentity variant**

In `src/thinker/interaction.rs`, add to the `Capability` enum (after `SilentReply`):

```rust
/// Channel natively displays agent identity (e.g. webchat sidebar)
NativeIdentity,
```

Add the `prompt_hint` match arm:

```rust
Self::NativeIdentity => (
    "native_identity",
    "Channel natively shows agent identity — no prefix needed",
),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib interaction::tests::test_native_identity`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/thinker/interaction.rs
git commit -m "interaction: add NativeIdentity capability variant"
```

---

### Task 2: Add agent_display_name to OutboundMessage

**Files:**
- Modify: `src/gateway/channel.rs:238-253` (OutboundMessage struct)

- [ ] **Step 1: Write the test**

Add to the existing test module in `src/gateway/channel.rs`:

```rust
#[test]
fn test_outbound_message_agent_display_name() {
    let msg = OutboundMessage::text("conv-1", "hello");
    // Default should be None
    assert!(msg.agent_display_name.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::channel::tests::test_outbound_message_agent`
Expected: FAIL — field does not exist

- [ ] **Step 3: Add agent_display_name field to OutboundMessage**

In `src/gateway/channel.rs`, add the field to `OutboundMessage`:

```rust
pub struct OutboundMessage {
    // ... existing fields ...
    /// Agent display name for identity indication
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_display_name: Option<String>,
}
```

Update `OutboundMessage::text()` to include the new field:

```rust
pub fn text(conversation_id: impl Into<String>, text: impl Into<String>) -> Self {
    Self {
        conversation_id: ConversationId::new(conversation_id),
        text: text.into(),
        attachments: Vec::new(),
        reply_to: None,
        inline_keyboard: None,
        metadata: HashMap::new(),
        agent_display_name: None,
    }
}
```

- [ ] **Step 4: Fix all compilation errors**

The `OutboundMessage` struct literal is used in `reply_emitter.rs:177`. Add `agent_display_name: None` to that construction site.

Run: `cargo check -p alephcore`
Expected: PASS (no compile errors)

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::channel::tests::test_outbound_message_agent`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/gateway/channel.rs src/gateway/reply_emitter.rs
git commit -m "channel: add agent_display_name field to OutboundMessage"
```

---

## Chunk 2: ReplyEmitter Prefix Logic

### Task 3: Add agent_display_name field and prefix logic to ReplyEmitter

**Files:**
- Modify: `src/gateway/reply_emitter.rs:64-121` (struct + constructors)
- Modify: `src/gateway/reply_emitter.rs:167-218` (send_to_channel)

- [ ] **Step 1: Write tests for apply_agent_prefix utility**

Add to the test module in `src/gateway/reply_emitter.rs`:

```rust
#[test]
fn test_apply_agent_prefix_with_name() {
    let result = apply_agent_prefix("Hello", &Some("Trading Bot".to_string()));
    assert_eq!(result, "[Trading Bot] Hello");
}

#[test]
fn test_apply_agent_prefix_none() {
    let result = apply_agent_prefix("Hello", &None);
    assert_eq!(result, "Hello");
}

#[test]
fn test_apply_agent_prefix_empty_name() {
    let result = apply_agent_prefix("Hello", &Some(String::new()));
    assert_eq!(result, "Hello");
}

#[test]
fn test_apply_agent_prefix_chinese_name() {
    let result = apply_agent_prefix("今天BTC价格是...", &Some("交易助手".to_string()));
    assert_eq!(result, "[交易助手] 今天BTC价格是...");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::reply_emitter::tests::test_apply_agent_prefix`
Expected: FAIL — function does not exist

- [ ] **Step 3: Implement apply_agent_prefix function**

Add before the `impl ReplyEmitter` block in `reply_emitter.rs`:

```rust
/// Prepend agent identity prefix if agent has a display name.
fn apply_agent_prefix(text: &str, agent_name: &Option<String>) -> String {
    match agent_name {
        Some(name) if !name.is_empty() => format!("[{}] {}", name, text),
        _ => text.to_string(),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::reply_emitter::tests::test_apply_agent_prefix`
Expected: PASS

- [ ] **Step 5: Add agent_display_name field to ReplyEmitter**

Add the field to `ReplyEmitter` struct:

```rust
pub struct ReplyEmitter {
    // ... existing fields ...
    /// Agent display name for identity prefix
    agent_display_name: Option<String>,
}
```

Update both constructors (`new` and `with_config`) to accept and store it:

```rust
pub fn new(
    channel_registry: Arc<ChannelRegistry>,
    route: ReplyRoute,
    run_id: String,
    agent_display_name: Option<String>,
) -> Self {
    Self {
        channel_registry,
        route,
        config: ReplyEmitterConfig::default(),
        buffer: Mutex::new(String::new()),
        seq_counter: AtomicU64::new(0),
        has_sent: AtomicBool::new(false),
        run_id,
        agent_display_name,
    }
}

pub fn with_config(
    channel_registry: Arc<ChannelRegistry>,
    route: ReplyRoute,
    run_id: String,
    config: ReplyEmitterConfig,
    agent_display_name: Option<String>,
) -> Self {
    Self {
        channel_registry,
        route,
        config,
        buffer: Mutex::new(String::new()),
        seq_counter: AtomicU64::new(0),
        has_sent: AtomicBool::new(false),
        run_id,
        agent_display_name,
    }
}
```

- [ ] **Step 6: Add channel_has_native_identity() method**

Add a helper method to `ReplyEmitter` that queries the channel's capabilities:

```rust
/// Check if the target channel natively displays agent identity.
async fn channel_has_native_identity(&self) -> bool {
    use crate::thinker::interaction::Capability;

    let Some(channel_handle) = self.channel_registry.get(&self.route.channel_id).await else {
        return false;
    };
    let channel = channel_handle.read().await;
    // Check ChannelProvider::detect_capabilities if the channel implements it
    // For now, use a trait-object downcast-free approach: check channel_type
    // against known native-identity channels. This will be replaced by
    // detect_capabilities() once more channels implement ChannelProvider.
    false
}
```

**Note:** Since `Channel` and `ChannelProvider` are separate traits and `Box<dyn Channel>` can't be downcast to `ChannelProvider`, we use a simpler approach: store `native_identity: bool` on `ReplyEmitter` itself, determined at construction time by the caller (who has access to the channel's capabilities). This avoids async capability checks in the hot path.

**Revised approach — add `native_identity` field to ReplyEmitter:**

```rust
pub struct ReplyEmitter {
    // ... existing fields ...
    agent_display_name: Option<String>,
    /// Whether the target channel natively shows agent identity
    native_identity: bool,
}
```

Update both constructors to accept `native_identity: bool` (default `false`):

```rust
pub fn new(
    channel_registry: Arc<ChannelRegistry>,
    route: ReplyRoute,
    run_id: String,
    agent_display_name: Option<String>,
    native_identity: bool,
) -> Self {
    Self {
        channel_registry,
        route,
        config: ReplyEmitterConfig::default(),
        buffer: Mutex::new(String::new()),
        seq_counter: AtomicU64::new(0),
        has_sent: AtomicBool::new(false),
        run_id,
        agent_display_name,
        native_identity,
    }
}
```

Same for `with_config` — add `native_identity: bool` parameter.

- [ ] **Step 7: Update send_to_channel to apply prefix on first send**

Replace `send_to_channel` entirely (lines 167-218). The key change is moving `has_sent` check before `store(true)` and adding the prefix:

```rust
async fn send_to_channel(&self, content: &str) {
    if content.is_empty() {
        return;
    }

    let is_first_send = !self.has_sent.load(Ordering::SeqCst);
    self.has_sent.store(true, Ordering::SeqCst);

    // Apply agent prefix only on first send, if channel lacks native identity
    let content = if is_first_send && !self.native_identity {
        apply_agent_prefix(content, &self.agent_display_name)
    } else {
        content.to_string()
    };

    let chunks = Self::split_message(&content, Self::MAX_MESSAGE_LENGTH);
    let total_chunks = chunks.len();

    for (i, chunk) in chunks.into_iter().enumerate() {
        let message = OutboundMessage {
            conversation_id: self.route.conversation_id.clone(),
            text: chunk,
            attachments: vec![],
            reply_to: if i == 0 {
                self.route.reply_to.clone()
            } else {
                None
            },
            inline_keyboard: None,
            metadata: Default::default(),
            agent_display_name: self.agent_display_name.clone(),
        };

        match self
            .channel_registry
            .send(&self.route.channel_id, message)
            .await
        {
            Ok(result) => {
                debug!(
                    "Sent reply to channel {} (message_id: {}, chunk {}/{})",
                    self.route.channel_id,
                    result.message_id.as_str(),
                    i + 1,
                    total_chunks
                );
            }
            Err(e) => {
                error!(
                    "Failed to send reply to channel {} (chunk {}/{}): {}",
                    self.route.channel_id,
                    i + 1,
                    total_chunks,
                    e
                );
                break;
            }
        }
    }
}
```

- [ ] **Step 8: Fix all existing tests in reply_emitter.rs**

Update all test `ReplyEmitter::new(...)` calls to include `None` and `false` as the 4th and 5th arguments:

```rust
// Example: change
ReplyEmitter::new(registry, route, "run-123".to_string())
// to
ReplyEmitter::new(registry, route, "run-123".to_string(), None, false)
```

Same for `ReplyEmitter::with_config(...)` — add `None` and `false` as 5th and 6th arguments.

- [ ] **Step 9: Run all reply_emitter tests**

Run: `cargo test -p alephcore --lib gateway::reply_emitter::tests`
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add src/gateway/reply_emitter.rs
git commit -m "reply_emitter: add agent identity prefix on first send"
```

---

## Chunk 3: Thread Agent Name to ReplyEmitter Construction Sites

### Task 4: Update executor.rs to pass agent display name

**Files:**
- Modify: `src/gateway/inbound_router/executor.rs:74-78`

- [ ] **Step 1: Pass agent display_name to ReplyEmitter**

In `executor.rs`, the `agent` variable is resolved at line 66-68. Update the ReplyEmitter construction (line 74-78):

```rust
let emitter = Arc::new(ReplyEmitter::new(
    self.channel_registry.clone(),
    ctx.reply_route.clone(),
    run_id.clone(),
    agent.config().display_name.clone(),
    false, // no channel currently declares NativeIdentity
));
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/gateway/inbound_router/executor.rs
git commit -m "executor: pass agent display name to ReplyEmitter"
```

---

### Task 5: Update session_scheduler.rs to pass agent display name

**Files:**
- Modify: `src/gateway/session_scheduler.rs:157-161` (execute_enriched)
- Modify: `src/gateway/session_scheduler.rs:357-361` (execute_next)

- [ ] **Step 1: Update execute_enriched method**

In `SessionScheduler::execute_enriched()`, the `agent` is resolved at line 143. Update ReplyEmitter construction (line 157-161):

```rust
let reply_emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(ReplyEmitter::new(
    self.channel_registry.clone(),
    enriched.merged.primary_context.reply_route.clone(),
    run_id.clone(),
    agent.config().display_name.clone(),
    false,
));
```

- [ ] **Step 2: Update execute_next function**

In `execute_next()`, the `agent` is resolved at line 344. Update ReplyEmitter construction (line 357-361):

```rust
let reply_emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(ReplyEmitter::new(
    channel_registry.clone(),
    enriched.merged.primary_context.reply_route.clone(),
    run_id.clone(),
    agent.config().display_name.clone(),
    false,
));
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS (pre-existing failures in `tools::markdown_skill::loader::tests` are acceptable)

- [ ] **Step 5: Commit**

```bash
git add src/gateway/session_scheduler.rs
git commit -m "session_scheduler: pass agent display name to ReplyEmitter"
```

---

## Chunk 4: Verification

### Task 6: Full compilation and test verification

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore`
Expected: PASS with no errors

- [ ] **Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS (ignoring pre-existing `markdown_skill::loader` failures)

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -50`
Expected: No new warnings

- [ ] **Step 4: Final commit (if any fixes needed)**

```bash
git add -A
git commit -m "agent-identity: fix clippy warnings"
```
