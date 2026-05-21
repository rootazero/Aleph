# Link ACL Probe Tests Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build 43 production-grade probe tests for agent link access control across 7 subsystems (P1-P7).

**Architecture:** Three-layer probe infrastructure (MockChannel → LinkAclHarness → Scenarios) in `tests/link_acl_probe/`, following the cron probe pattern. MockChannel captures outbound replies via mpsc; LinkAclHarness wraps InboundMessageRouter with all mocked dependencies; scenarios test all enforcement points.

**Tech Stack:** Rust, tokio, alephcore gateway internals, existing test utilities from `tests/world/gateway_ctx.rs`

---

### Task 1: Test infrastructure — MockChannel + entry point

**Files:**
- Create: `tests/link_acl_probe.rs`
- Create: `tests/link_acl_probe/mock_channel.rs`

**Step 1: Create entry point**

Create `tests/link_acl_probe.rs`:

```rust
mod link_acl_probe;
```

Create `tests/link_acl_probe/` directory.

**Step 2: Create MockChannel**

Create `tests/link_acl_probe/mock_channel.rs`. This implements the `Channel` trait and captures all outbound messages via an mpsc channel:

```rust
//! Mock channel that captures outbound messages for assertion.

use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use async_trait::async_trait;
use alephcore::gateway::channel::{
    Channel, ChannelCapabilities, ChannelInfo, ChannelResult, ChannelState,
    ChannelStatus, OutboundMessage, SendResult, ChannelId,
};

/// A captured outbound reply for test assertions.
#[derive(Debug, Clone)]
pub struct CapturedReply {
    pub conversation_id: String,
    pub text: String,
}

/// Mock channel that captures sent messages.
pub struct MockChannel {
    info: ChannelInfo,
    state: ChannelState,
    reply_tx: mpsc::UnboundedSender<CapturedReply>,
}

impl MockChannel {
    pub fn new(id: &str, reply_tx: mpsc::UnboundedSender<CapturedReply>) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            channel_type: "mock".to_string(),
            name: id.to_string(),
            capabilities: ChannelCapabilities::default(),
        };
        let (inbound_tx, _) = mpsc::channel(100);
        let state = ChannelState::new(inbound_tx);
        Self { info, state, reply_tx }
    }
}

#[async_trait]
impl Channel for MockChannel {
    fn info(&self) -> &ChannelInfo { &self.info }
    fn state(&self) -> &ChannelState { &self.state }

    async fn start(&mut self) -> ChannelResult<()> {
        self.state.set_status(ChannelStatus::Connected).await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        self.state.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let _ = self.reply_tx.send(CapturedReply {
            conversation_id: message.conversation_id.as_str().to_string(),
            text: message.text.clone(),
        });
        Ok(SendResult::Sent)
    }
}
```

**IMPORTANT:** Read the actual `Channel` trait, `ChannelInfo`, `ChannelState`, `ChannelCapabilities`, `SendResult` definitions to get the exact API right. The code above is a guide — adapt field names, method signatures, and imports to match the real trait. Key files to check:
- `src/gateway/channel.rs` — Channel trait, ChannelInfo, ChannelState, ChannelCapabilities, SendResult
- Check if ChannelInfo has `capabilities` as a field or if it's provided via the trait

**Step 3: Verify compilation**

Run: `cargo test -p alephcore --test link_acl_probe -- --list 2>&1 | head -5`
Expected: Compiles (no tests yet, but no errors).

**Step 4: Commit**

```bash
git add tests/link_acl_probe.rs tests/link_acl_probe/
git commit -m "link_acl probe: add test infrastructure — MockChannel + entry point

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 2: Test infrastructure — LinkAclHarness

**Files:**
- Create: `tests/link_acl_probe/harness.rs`
- Modify: `tests/link_acl_probe.rs` (add mod declaration)

**Step 1: Create the harness**

Create `tests/link_acl_probe/harness.rs`. This wraps InboundMessageRouter with all mocked dependencies:

```rust
//! LinkAclHarness — ergonomic test harness for link access control probes.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tempfile::TempDir;

use alephcore::gateway::channel::{InboundMessage, ChannelId, ConversationId, MessageId, UserId};
use alephcore::gateway::channel_registry::ChannelRegistry;
use alephcore::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
use alephcore::gateway::inbound_router::InboundMessageRouter;
use alephcore::gateway::pairing_store::SqlitePairingStore;
use alephcore::gateway::routing_config::RoutingConfig;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::workspace::WorkspaceManager;

// Re-use TrackingExecutionAdapter from existing test world
// (may need to duplicate or import depending on visibility)

use super::mock_channel::{MockChannel, CapturedReply};

pub struct LinkAclHarness {
    pub channel_registry: Arc<ChannelRegistry>,
    pub agent_registry: Arc<AgentRegistry>,
    pub workspace_manager: Arc<WorkspaceManager>,
    pub reply_rx: mpsc::UnboundedReceiver<CapturedReply>,
    reply_tx: mpsc::UnboundedSender<CapturedReply>,
    router: Option<Arc<InboundMessageRouter>>,
    _temp_dir: TempDir,
    msg_counter: std::sync::atomic::AtomicU64,
}

impl LinkAclHarness {
    /// Create a new harness with all mocked dependencies.
    pub async fn new() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let (reply_tx, reply_rx) = mpsc::unbounded_channel();

        let channel_registry = Arc::new(ChannelRegistry::new());
        let agent_registry = Arc::new(AgentRegistry::new());
        let pairing_store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();

        // Create execution adapter (tracking)
        // Need to implement or duplicate TrackingExecutionAdapter here
        let tracking = Arc::new(TrackingExecutionAdapter::new());
        let adapter: Arc<dyn ExecutionAdapter> = tracking.clone();
        let agent_router = Arc::new(AgentRouter::new());

        // Build workspace manager with temp path
        let ws_config = WorkspaceManagerConfig {
            db_path: temp_dir.path().join("workspaces.db"),
            ..Default::default()
        };
        let workspace_manager = Arc::new(WorkspaceManager::new(ws_config).unwrap());

        let router = InboundMessageRouter::with_unified_routing(
            channel_registry.clone(),
            pairing_store,
            config,
            agent_registry.clone(),
            adapter,
            agent_router,
        ).with_workspace_manager(workspace_manager.clone());

        Self {
            channel_registry,
            agent_registry,
            workspace_manager,
            reply_rx,
            reply_tx,
            router: Some(Arc::new(router)),
            _temp_dir: temp_dir,
            msg_counter: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Register a mock channel (link/bot) that captures outbound replies.
    pub async fn register_link(&self, link_id: &str) {
        let mut channel = MockChannel::new(link_id, self.reply_tx.clone());
        channel.start().await.unwrap();
        self.channel_registry.register(Box::new(channel)).await;
    }

    /// Register an agent with optional allowed_links.
    pub async fn register_agent(&self, id: &str, allowed_links: Option<Vec<String>>) {
        let config = AgentInstanceConfig {
            agent_id: id.to_string(),
            display_name: Some(id.to_string()),
            workspace: self._temp_dir.path().join(format!("ws-{}", id)),
            agent_dir: self._temp_dir.path().join(format!("agent-{}", id)),
            allowed_links,
            ..Default::default()
        };
        let instance = AgentInstance::new(config).unwrap();
        self.agent_registry.register(instance).await;
    }

    /// Update an agent's allowed_links at runtime.
    pub async fn update_allowed_links(&self, agent_id: &str, links: Option<Vec<String>>) {
        // Remove + re-register with new config
        // (or add a method to AgentRegistry if needed)
        self.agent_registry.remove(agent_id).await;
        self.register_agent(agent_id, links).await;
    }

    /// Send a message from a link to the router.
    pub async fn send_message(&self, link_id: &str, text: &str) {
        let msg = self.make_message(link_id, text);
        self.router.as_ref().unwrap().handle_message(msg).await.unwrap();
    }

    /// Build an InboundMessage from a link.
    fn make_message(&self, link_id: &str, text: &str) -> InboundMessage {
        let n = self.msg_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        InboundMessage {
            id: MessageId::new(format!("msg-{}", n)),
            channel_id: ChannelId::new(link_id),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        }
    }

    /// Drain all captured replies and return them.
    pub fn drain_replies(&mut self) -> Vec<CapturedReply> {
        let mut replies = vec![];
        while let Ok(reply) = self.reply_rx.try_recv() {
            replies.push(reply);
        }
        replies
    }

    /// Assert that the last reply contains the given substring.
    pub fn assert_reply_contains(&mut self, substring: &str) {
        let replies = self.drain_replies();
        assert!(
            replies.iter().any(|r| r.text.contains(substring)),
            "Expected reply containing '{}', got: {:?}",
            substring,
            replies.iter().map(|r| &r.text).collect::<Vec<_>>()
        );
    }

    /// Assert no replies were sent.
    pub fn assert_no_denial(&mut self) {
        let replies = self.drain_replies();
        let denials: Vec<_> = replies.iter().filter(|r| r.text.contains("⛔") || r.text.contains("denied")).collect();
        assert!(
            denials.is_empty(),
            "Expected no denial replies, got: {:?}",
            denials.iter().map(|r| &r.text).collect::<Vec<_>>()
        );
    }

    /// Assert a denial reply was sent.
    pub fn assert_denied(&mut self) {
        let replies = self.drain_replies();
        assert!(
            replies.iter().any(|r| r.text.contains("⛔") || r.text.contains("denied")),
            "Expected denial reply, got: {:?}",
            replies.iter().map(|r| &r.text).collect::<Vec<_>>()
        );
    }
}
```

**IMPORTANT:** This is a guide. The implementer must:
1. Check exact imports — `InboundMessage`, `ChannelId`, etc. may be in different submodules
2. Check `AgentInstance::new()` — it creates directories, so use temp paths
3. Either duplicate `TrackingExecutionAdapter` or find a way to import it from `tests/world/`
4. Check if `WorkspaceManagerConfig` fields match — read `src/gateway/workspace.rs`
5. The pairing store needs `DmPolicyConfig::Open` in the channel config to skip pairing checks, OR register the sender as approved

**Step 2: Add mod declaration**

In `tests/link_acl_probe.rs`, add:
```rust
mod link_acl_probe;
// becomes:
mod link_acl_probe {
    mod mock_channel;
    mod harness;
}
```

Actually, check how cron_probe.rs does it — it uses a flat mod structure:
```rust
// tests/cron_probe.rs
mod cron_probe;
```
Then `tests/cron_probe/mod.rs` has the sub-mod declarations. Follow the same pattern.

**Step 3: Verify compilation**

Run: `cargo test -p alephcore --test link_acl_probe -- --list 2>&1 | head -5`

**Step 4: Commit**

```bash
git add tests/link_acl_probe/
git commit -m "link_acl probe: add LinkAclHarness test harness

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 3: P1 — Access Control pure logic probes (6 scenarios)

**Files:**
- Create: `tests/link_acl_probe/access_control.rs`

**Step 1: Write 6 probe scenarios**

These test `check_link_access` directly. Since it's a private function in `inbound_router.rs`, test it indirectly through the harness by sending messages and checking results.

```rust
//! P1 Access Control — 6 end-to-end scenarios covering check_link_access pure logic.

use super::harness::LinkAclHarness;

// ── 1. none_allows_all ──────────────────────────────────────

#[tokio::test]
async fn none_allows_all() {
    let mut h = LinkAclHarness::new().await;
    h.register_link("telegram-bot").await;
    h.register_agent("main", None).await; // None = all allowed
    h.send_message("telegram-bot", "hello").await;
    h.assert_no_denial();
}

// ── 2. empty_list_allows_all ────────────────────────────────

#[tokio::test]
async fn empty_list_allows_all() {
    let mut h = LinkAclHarness::new().await;
    h.register_link("telegram-bot").await;
    h.register_agent("main", Some(vec![])).await; // empty = all allowed
    h.send_message("telegram-bot", "hello").await;
    h.assert_no_denial();
}

// ── 3. whitelist_hit ────────────────────────────────────────

#[tokio::test]
async fn whitelist_hit() {
    let mut h = LinkAclHarness::new().await;
    h.register_link("telegram-bot").await;
    h.register_agent("main", Some(vec!["telegram-bot".to_string()])).await;
    h.send_message("telegram-bot", "hello").await;
    h.assert_no_denial();
}

// ── 4. whitelist_miss ───────────────────────────────────────

#[tokio::test]
async fn whitelist_miss() {
    let mut h = LinkAclHarness::new().await;
    h.register_link("discord-bot").await;
    h.register_agent("main", Some(vec!["telegram-bot".to_string()])).await;
    h.send_message("discord-bot", "hello").await;
    h.assert_denied();
}

// ── 5. multi_link_whitelist ─────────────────────────────────

#[tokio::test]
async fn multi_link_whitelist() {
    let mut h = LinkAclHarness::new().await;
    h.register_link("tg-2").await;
    h.register_agent("main", Some(vec!["tg-1".to_string(), "tg-2".to_string()])).await;
    h.send_message("tg-2", "hello").await;
    h.assert_no_denial();
}

// ── 6. single_link_rejects_others ───────────────────────────

#[tokio::test]
async fn single_link_rejects_others() {
    let mut h = LinkAclHarness::new().await;
    h.register_link("dc-1").await;
    h.register_agent("main", Some(vec!["tg-1".to_string()])).await;
    h.send_message("dc-1", "hello").await;
    h.assert_denied();
    h.assert_reply_contains("dc-1");
    h.assert_reply_contains("main");
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test link_acl_probe`
Expected: 6 passed.

**Step 3: Commit**

```bash
git add tests/link_acl_probe/access_control.rs
git commit -m "link_acl probe: P1 access control pure logic (6 scenarios)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 4: P2 — Message Routing enforcement probes (8 scenarios)

**Files:**
- Create: `tests/link_acl_probe/message_routing.rs`

**Step 1: Write 8 probe scenarios**

```rust
//! P2 Message Routing — 8 end-to-end scenarios covering handle_message enforcement.

use super::harness::LinkAclHarness;

// ── 1. unrestricted_agent_any_link ──────────────────────────
// Agent with no restrictions accepts messages from any link

// ── 2. restricted_agent_allowed_link ────────────────────────
// Agent with whitelist accepts messages from listed link

// ── 3. restricted_agent_denied_link ─────────────────────────
// Agent with whitelist rejects messages from unlisted link + sends ⛔ reply

// ── 4. agent_not_in_registry ────────────────────────────────
// When agent_registry has no entry for resolved agent_id, skip ACL check (graceful)

// ── 5. denied_message_not_executed ──────────────────────────
// Verify that after denial, the execution adapter was NOT called
// (need tracking adapter reference)

// ── 6. denial_reply_format ──────────────────────────────────
// Verify error message contains both link_id and agent_id

// ── 7. consecutive_denials ──────────────────────────────────
// Send two messages from denied link, both get denied (no cache/bypass)

// ── 8. group_message_acl ────────────────────────────────────
// is_group=true message also subject to ACL (groups not exempt)
// (need harness helper to send group messages)
```

Each scenario follows the pattern from P1 — arrange with harness, send message, assert.

For scenario 5, the harness needs to expose the `TrackingExecutionAdapter` so tests can check `was_called()` / `call_count()`.

For scenario 8, add a `send_group_message` helper to harness that sets `is_group: true`.

**Step 2: Run tests**

Run: `cargo test -p alephcore --test link_acl_probe`
Expected: 14 passed (6 + 8).

**Step 3: Commit**

```bash
git add tests/link_acl_probe/message_routing.rs
git commit -m "link_acl probe: P2 message routing enforcement (8 scenarios)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 5: P3 — /switch command enforcement probes (6 scenarios)

**Files:**
- Create: `tests/link_acl_probe/switch_command.rs`

**Step 1: Write 6 probe scenarios**

Test `/switch` command enforcement. Send messages starting with `/switch <agent>` through the router.

```rust
//! P3 Switch Command — 6 end-to-end scenarios covering /switch enforcement.

// ── 1. switch_to_allowed_agent ──────────────────────────────
// /switch to agent that allows this link → success ✅

// ── 2. switch_to_denied_agent ───────────────────────────────
// /switch to agent that denies this link → ⛔ denial

// ── 3. switch_to_nonexistent_agent ──────────────────────────
// /switch to agent not in registry → "not found" (existing logic, not ACL)

// ── 4. switch_back_to_allowed ───────────────────────────────
// Link denied by agent-B, /switch back to agent-A (allowed) → success

// ── 5. different_links_different_permissions ─────────────────
// agent allows link-A, denies link-B. Both try /switch → A succeeds, B denied

// ── 6. denied_switch_preserves_current ──────────────────────
// After denied /switch, verify current active agent unchanged
```

**IMPORTANT:** The `/switch` command needs the `command_parser` to be set, OR use the fallback path that checks for `/switch ` prefix directly. Check the router code — there's a fallback at line 594 that handles `/switch <name>` even without command_parser. Tests should send `/switch <agent_id>` as message text.

Also: the router needs `workspace_manager` set to handle agent switching. The harness already sets it.

**Step 2: Run tests**

Run: `cargo test -p alephcore --test link_acl_probe`
Expected: 20 passed.

**Step 3: Commit**

```bash
git add tests/link_acl_probe/switch_command.rs
git commit -m "link_acl probe: P3 /switch command enforcement (6 scenarios)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 6: P4 — Intent switch enforcement probes (5 scenarios)

**Files:**
- Create: `tests/link_acl_probe/intent_switch.rs`

**Step 1: Write 5 probe scenarios**

Intent-based switching requires the `IntentDetector` with an LLM provider. For testing without real LLM, there are two approaches:

**Option A (recommended):** Skip intent detection tests if they require LLM. The intent detector without LLM returns `DetectedIntent::Normal` for everything. Instead, test the code path by directly calling `handle_message` with a message that would trigger the fallback `/switch` path.

**Option B:** Create a mock `AiProvider` that returns canned JSON responses for intent detection.

If the implementer finds that intent-based switching cannot be tested without an LLM mock, these 5 scenarios can be adapted to test additional `/switch` edge cases instead, OR implement a minimal mock AiProvider.

```rust
//! P4 Intent Switch — 5 scenarios covering intent-based agent switch enforcement.
//!
//! NOTE: If IntentDetector requires real LLM, adapt to test via /switch fallback
//! or implement MockAiProvider.

// ── 1. intent_switch_allowed ────────────────────────────────
// ── 2. intent_switch_denied ─────────────────────────────────
// ── 3. dynamic_create_then_denied ───────────────────────────
// ── 4. intent_with_task_denied ──────────────────────────────
// ── 5. non_switch_intent_no_acl ─────────────────────────────
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test link_acl_probe`
Expected: 25 passed.

**Step 3: Commit**

```bash
git add tests/link_acl_probe/intent_switch.rs
git commit -m "link_acl probe: P4 intent switch enforcement (5 scenarios)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 7: P5 — Config lifecycle probes (5 scenarios)

**Files:**
- Create: `tests/link_acl_probe/config_lifecycle.rs`

**Step 1: Write 5 probe scenarios**

```rust
//! P5 Config Lifecycle — 5 scenarios covering runtime config changes.

// ── 1. runtime_restrict ─────────────────────────────────────
// Agent starts with None (all allowed), update to restricted, next message denied

// ── 2. runtime_unrestrict ───────────────────────────────────
// Agent starts restricted, update to None, next message allowed

// ── 3. runtime_change_list ──────────────────────────────────
// Agent allows link-A, update to allow only link-B → link-A now denied

// ── 4. new_agent_default_none ───────────────────────────────
// Newly created agent has allowed_links=None → all links access

// ── 5. delete_recreate_agent ────────────────────────────────
// Delete agent, recreate with different allowed_links → new config effective
```

Uses `h.update_allowed_links()` between sends.

**Step 2: Run tests**

Run: `cargo test -p alephcore --test link_acl_probe`
Expected: 30 passed.

**Step 3: Commit**

```bash
git add tests/link_acl_probe/config_lifecycle.rs
git commit -m "link_acl probe: P5 config lifecycle (5 scenarios)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 8: P6 — Multi-agent matrix probes (6 scenarios)

**Files:**
- Create: `tests/link_acl_probe/multi_agent_matrix.rs`

**Step 1: Write 6 probe scenarios**

```rust
//! P6 Multi-Agent Matrix — 6 scenarios covering multi-link × multi-agent combinations.

// ── 1. three_by_three_matrix ────────────────────────────────
// 3 agents × 3 links with different permissions → verify all 9 combos

// ── 2. default_agent_restricted ─────────────────────────────
// Default agent has restricted list → unrouted message from denied link gets denied

// ── 3. independent_agents ───────────────────────────────────
// Agent-A allows link-1, Agent-B allows link-2 → no cross-contamination

// ── 4. same_link_different_agents ───────────────────────────
// One link has different permissions on different agents → each checked independently

// ── 5. all_agents_deny_link ─────────────────────────────────
// Every agent denies a specific link → that link cannot access anything

// ── 6. mixed_policies ───────────────────────────────────────
// One agent allows all (None), another restricts → verify both work simultaneously
```

For testing routing to specific agents (not just default), the harness needs to either:
- Set up `AgentRouter` bindings, or
- Use `workspace_manager.set_active_agent()` to pre-set the active agent for a channel

**Step 2: Run tests**

Run: `cargo test -p alephcore --test link_acl_probe`
Expected: 36 passed.

**Step 3: Commit**

```bash
git add tests/link_acl_probe/multi_agent_matrix.rs
git commit -m "link_acl probe: P6 multi-agent matrix (6 scenarios)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 9: P7 — Edge cases + system interaction probes (7 scenarios)

**Files:**
- Create: `tests/link_acl_probe/edge_cases.rs`

**Step 1: Write 7 probe scenarios**

```rust
//! P7 Edge Cases — 7 scenarios covering boundaries and system interaction.

// ── 1. stale_link_in_whitelist ───────────────────────────────
// allowed_links contains "deleted-bot" (not registered) → other links still checked correctly

// ── 2. empty_link_id ────────────────────────────────────────
// Message from link with empty string id → safe deny or handle

// ── 3. duplicate_entries ────────────────────────────────────
// allowed_links = ["tg-1", "tg-1"] → works correctly, no panic

// ── 4. empty_agent_id ───────────────────────────────────────
// Agent with empty string id → safe handling

// ── 5. pairing_approved_but_acl_denied ──────────────────────
// Sender is paired/approved, but ACL denies the link → ACL wins
// (register sender as approved in pairing store, but set allowed_links to exclude)

// ── 6. routing_rule_but_acl_denied ──────────────────────────
// AgentRouter binding routes to agent-X, but agent-X denies this link → denied

// ── 7. concurrent_denials ───────────────────────────────────
// Spawn multiple tokio tasks sending from denied link simultaneously → all denied, no race
```

For scenario 5: use `SqlitePairingStore.approve()` to mark sender as approved, then verify ACL still blocks.

For scenario 7: use `tokio::join!` or `tokio::spawn` to send multiple messages concurrently.

**Step 2: Run tests**

Run: `cargo test -p alephcore --test link_acl_probe`
Expected: 43 passed.

**Step 3: Commit**

```bash
git add tests/link_acl_probe/edge_cases.rs
git commit -m "link_acl probe: P7 edge cases + system interaction (7 scenarios)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 10: Final verification + cleanup

**Step 1: Run full test suite**

Run: `cargo test -p alephcore --test link_acl_probe -v`
Expected: 43 tests, all passing.

**Step 2: Run existing tests to ensure no regressions**

Run: `cargo test -p alephcore --lib`
Expected: ~8067 tests pass.

**Step 3: Final commit**

```bash
git add -A
git commit -m "link_acl probe: complete — 43 scenarios across 7 subsystems (P1-P7)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```
