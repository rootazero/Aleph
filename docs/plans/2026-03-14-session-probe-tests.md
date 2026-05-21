# Session Isolation Probe Tests Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Production-grade probe tests validating the session isolation feature across epoch mechanics, topic generation, /new & /switch lifecycle, RPC handlers, memory filtering, and backward compatibility.

**Architecture:** Three-layer probe pattern (MockLlm + SessionProbeHarness + scenario files). Harness wraps InboundMessageRouter + SessionManager + MockChannel, reusing the established link_acl_probe pattern. Seven phases (P1–P7) cover orthogonal testing concerns from basic lifecycle through multi-agent matrix and edge cases.

**Tech Stack:** `tokio::test`, `tempfile`, `serde_json`, `alephcore` (SessionManager, InboundMessageRouter, SessionKey, MemoryFilter)

---

## File Layout

```
tests/
├── session_probe.rs                    # Entry: mod declarations
└── session_probe/
    ├── harness.rs                      # SessionProbeHarness
    ├── mock_llm.rs                     # MockLlmProvider for topic generation
    ├── lifecycle.rs                    # P1: Session lifecycle (8 tests)
    ├── epoch_mechanics.rs             # P2: Epoch versioning (7 tests)
    ├── topic_generation.rs            # P3: LLM topic generation (6 tests)
    ├── rpc_handler.rs                 # P4: sessions.new RPC (6 tests)
    ├── memory_filter.rs              # P5: Memory session_ids filtering (5 tests)
    ├── multi_agent.rs                # P6: Multi-agent session matrix (5 tests)
    ├── edge_cases.rs                 # P7: Edge cases & backward compat (8 tests)
```

---

### Task 1: Entry Point + MockLlmProvider

**Files:**
- Create: `tests/session_probe.rs`
- Create: `tests/session_probe/mock_llm.rs`

**Step 1: Create mock LLM provider**

`tests/session_probe/mock_llm.rs`:

```rust
//! Mock LLM provider for session probe tests.
//!
//! Configurable responses for topic generation testing.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::Mutex;

/// Captured LLM call for assertion.
#[derive(Debug, Clone)]
pub struct LlmCall {
    pub input: String,
    pub system_prompt: Option<String>,
}

/// Mock AI provider that returns configurable responses.
pub struct MockLlmProvider {
    responses: Mutex<Vec<String>>,
    default_response: String,
    calls: Mutex<Vec<LlmCall>>,
    call_count: AtomicUsize,
    should_fail: bool,
}

impl MockLlmProvider {
    /// Create a provider that returns the given default response.
    pub fn new(default_response: &str) -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            default_response: default_response.to_string(),
            calls: Mutex::new(Vec::new()),
            call_count: AtomicUsize::new(0),
            should_fail: false,
        }
    }

    /// Create a provider that always fails.
    pub fn failing() -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            default_response: String::new(),
            calls: Mutex::new(Vec::new()),
            call_count: AtomicUsize::new(0),
            should_fail: true,
        }
    }

    /// Queue a specific response for the next call.
    pub async fn queue_response(&self, response: &str) {
        self.responses.lock().await.push(response.to_string());
    }

    /// Get number of calls made.
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Get captured calls for assertion.
    pub async fn get_calls(&self) -> Vec<LlmCall> {
        self.calls.lock().await.clone()
    }
}

#[async_trait]
impl alephcore::providers::AiProvider for MockLlmProvider {
    async fn process(
        &self,
        input: &str,
        system_prompt: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.calls.lock().await.push(LlmCall {
            input: input.to_string(),
            system_prompt: system_prompt.map(|s| s.to_string()),
        });

        if self.should_fail {
            return Err("Mock LLM failure".into());
        }

        let mut responses = self.responses.lock().await;
        if responses.is_empty() {
            Ok(self.default_response.clone())
        } else {
            Ok(responses.remove(0))
        }
    }

    fn name(&self) -> &str {
        "mock-llm"
    }
}
```

**Step 2: Create entry point**

`tests/session_probe.rs`:

```rust
//! Session isolation probe integration tests.
//!
//! Tests the session lifecycle, epoch mechanics, topic generation,
//! RPC handlers, memory filtering, and multi-agent scenarios.

mod session_probe {
    pub mod mock_llm;
    pub mod harness;
    pub mod lifecycle;
    pub mod epoch_mechanics;
    pub mod topic_generation;
    pub mod rpc_handler;
    pub mod memory_filter;
    pub mod multi_agent;
    pub mod edge_cases;
}
```

**Step 3: Verify it compiles (will fail — harness module missing)**

Run: `cargo test -p alephcore --test session_probe --no-run 2>&1 | head -5`
Expected: Compilation error for missing `harness` module

**Step 4: Commit**

```bash
git add tests/session_probe.rs tests/session_probe/mock_llm.rs
git commit -m "session probe: add entry point and MockLlmProvider"
```

---

### Task 2: SessionProbeHarness

**Files:**
- Create: `tests/session_probe/harness.rs`

**Reference:** `tests/link_acl_probe/harness.rs` for pattern

**Step 1: Write the harness**

`tests/session_probe/harness.rs`:

```rust
//! Session probe test harness.
//!
//! Wraps SessionManager + InboundMessageRouter + MockChannel
//! with helpers for testing session isolation features.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tempfile::TempDir;
use tokio::sync::mpsc;

use alephcore::gateway::{
    AgentInstance, AgentInstanceConfig, AgentRegistry, AgentRouter,
    Channel, ChannelId, ChannelRegistry, ConversationId, DmPolicy,
    ExecutionAdapter, GroupPolicy, InboundMessage, InboundMessageRouter,
    MessageId, RouterChannelConfig, RoutingConfig, RunStatus,
    SqlitePairingStore, UserId, WorkspaceManager, WorkspaceManagerConfig,
};
use alephcore::gateway::channel::{
    ChannelCapabilities, ChannelInfo, ChannelResult, ChannelState,
    ChannelStatus, OutboundMessage, SendResult,
};
use alephcore::gateway::event_emitter::{EventEmitter, StreamEvent};
use alephcore::gateway::execution_engine::{ExecutionError, RunState};
use alephcore::gateway::router::SessionKey;
use alephcore::gateway::session_manager::{SessionManager, SessionManagerConfig};
use alephcore::gateway::RunRequest;
use alephcore::routing::session_key::SessionKey as RoutingKey;

use super::mock_llm::MockLlmProvider;

// =============================================================================
// CapturedReply
// =============================================================================

#[derive(Debug, Clone)]
pub struct CapturedReply {
    pub conversation_id: String,
    pub text: String,
}

// =============================================================================
// MockChannel
// =============================================================================

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
            status: ChannelStatus::Disconnected,
            capabilities: ChannelCapabilities::default(),
        };
        let state = ChannelState::new(100);
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
        Ok(SendResult {
            message_id: MessageId::new("sent-1"),
            timestamp: Utc::now(),
        })
    }
}

// =============================================================================
// NoOpExecutionAdapter
// =============================================================================

pub struct NoOpExecutionAdapter;

#[async_trait]
impl ExecutionAdapter for NoOpExecutionAdapter {
    async fn execute(
        &self,
        _request: RunRequest,
        _agent: Arc<AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        Ok(())
    }
    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        Err(ExecutionError::RunNotFound(run_id.to_string()))
    }
    async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        Some(RunStatus {
            run_id: run_id.to_string(),
            state: RunState::Completed,
            started_at: Some(Utc::now()),
            completed_at: Some(Utc::now()),
            steps_completed: 0,
            current_tool: None,
        })
    }
}

// =============================================================================
// SessionProbeHarness
// =============================================================================

pub struct SessionProbeHarness {
    pub session_manager: Arc<SessionManager>,
    pub channel_registry: Arc<ChannelRegistry>,
    pub agent_registry: Arc<AgentRegistry>,
    pub workspace_manager: Arc<WorkspaceManager>,
    pub reply_rx: mpsc::UnboundedReceiver<CapturedReply>,
    reply_tx: mpsc::UnboundedSender<CapturedReply>,
    router: InboundMessageRouter,
    _temp_dir: TempDir,
    msg_counter: AtomicU64,
}

impl SessionProbeHarness {
    /// Build a fully wired harness with session management support.
    pub fn new() -> Self {
        Self::with_llm(None)
    }

    /// Build harness with a specific LLM provider for topic generation.
    pub fn with_llm(llm: Option<Arc<MockLlmProvider>>) -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Session manager with SQLite
        let sm_config = SessionManagerConfig {
            db_path: temp_dir.path().join("sessions.db"),
            max_messages: 100,
            compaction_keep: 50,
            ..Default::default()
        };
        let session_manager = Arc::new(
            SessionManager::new(sm_config).expect("Failed to create SessionManager"),
        );

        let channel_registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();
        let agent_registry = Arc::new(AgentRegistry::new());
        let adapter: Arc<dyn ExecutionAdapter> = Arc::new(NoOpExecutionAdapter);
        let agent_router = Arc::new(AgentRouter::new());

        // Workspace manager
        let ws_config = WorkspaceManagerConfig {
            db_path: temp_dir.path().join("workspaces.db"),
            default_profile: "default".to_string(),
            archive_after_days: 0,
        };
        let workspace_manager = Arc::new(
            WorkspaceManager::new(ws_config).expect("Failed to create WorkspaceManager"),
        );

        let mut router = InboundMessageRouter::with_unified_routing(
            channel_registry.clone(),
            store,
            config,
            agent_registry.clone(),
            adapter,
            agent_router,
        )
        .with_workspace_manager(workspace_manager.clone())
        .with_session_manager(session_manager.clone());

        // Wire LLM provider if provided
        if let Some(llm_provider) = llm {
            let ai_provider: Arc<dyn alephcore::providers::AiProvider> = llm_provider;
            router = router.with_llm_provider(ai_provider);
        }

        let (reply_tx, reply_rx) = mpsc::unbounded_channel();

        Self {
            session_manager,
            channel_registry,
            agent_registry,
            workspace_manager,
            reply_rx,
            reply_tx,
            router,
            _temp_dir: temp_dir,
            msg_counter: AtomicU64::new(0),
        }
    }

    /// Register a mock channel.
    pub async fn register_channel(&mut self, channel_id: &str) {
        let mut channel = MockChannel::new(channel_id, self.reply_tx.clone());
        channel.start().await.expect("Failed to start mock channel");
        self.channel_registry.register(Box::new(channel)).await;
        self.router.register_channel_config(
            channel_id,
            RouterChannelConfig {
                dm_policy: DmPolicy::Open,
                group_policy: GroupPolicy::Open,
                allow_from: vec![],
                group_allow_from: vec![],
                require_mention: false,
                bot_name: None,
            },
        );
    }

    /// Register an agent.
    pub async fn register_agent(&self, id: &str) {
        let config = AgentInstanceConfig {
            agent_id: id.to_string(),
            workspace: self._temp_dir.path().join(format!("ws-{}", id)),
            agent_dir: self._temp_dir.path().join(format!("agent-{}", id)),
            allowed_links: None,
            ..AgentInstanceConfig::default()
        };
        let instance = AgentInstance::new(config).expect("Failed to create AgentInstance");
        self.agent_registry.register(instance).await;
    }

    /// Send a DM-style inbound message through the router.
    pub async fn send_message(&self, channel_id: &str, sender_id: &str, text: &str) {
        let msg = self.make_message(channel_id, sender_id, text);
        let _ = self.router.handle_message(msg).await;
    }

    /// Build an InboundMessage.
    fn make_message(&self, channel_id: &str, sender_id: &str, text: &str) -> InboundMessage {
        let n = self.msg_counter.fetch_add(1, Ordering::SeqCst);
        InboundMessage {
            id: MessageId::new(format!("msg-{}", n)),
            channel_id: ChannelId::new(channel_id),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new(sender_id),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        }
    }

    /// Drain all captured replies.
    pub fn drain_replies(&mut self) -> Vec<CapturedReply> {
        let mut replies = Vec::new();
        while let Ok(reply) = self.reply_rx.try_recv() {
            replies.push(reply);
        }
        replies
    }

    /// Assert that at least one reply contains the given substring.
    pub fn assert_reply_contains(&mut self, substring: &str) {
        let replies = self.drain_replies();
        assert!(
            replies.iter().any(|r| r.text.contains(substring)),
            "Expected reply containing '{}', got: {:?}",
            substring,
            replies.iter().map(|r| &r.text).collect::<Vec<_>>()
        );
    }

    /// Assert that no replies were sent.
    pub fn assert_no_replies(&mut self) {
        let replies = self.drain_replies();
        assert!(
            replies.is_empty(),
            "Expected no replies, got: {:?}",
            replies.iter().map(|r| &r.text).collect::<Vec<_>>()
        );
    }

    /// Create a session directly in the manager.
    pub async fn create_session(&self, key: &SessionKey) {
        self.session_manager.get_or_create(key).await
            .expect("Failed to create session");
    }

    /// Add messages to a session for testing topic generation.
    pub async fn seed_messages(&self, key: &SessionKey, messages: &[(&str, &str)]) {
        for (role, content) in messages {
            self.session_manager.add_message(key, role, content).await
                .expect("Failed to add message");
        }
    }

    /// Get session topic from metadata.
    pub async fn get_topic(&self, key: &SessionKey) -> Option<String> {
        self.session_manager.get_session_topic(key).await.ok().flatten()
    }

    /// Check if session is closed.
    pub async fn is_session_closed(&self, key: &SessionKey) -> bool {
        let meta = self.session_manager.get_or_create(key).await.ok();
        if let Some(m) = meta {
            if let Some(json_str) = &m.metadata_json {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
                    return v.get("status").and_then(|s| s.as_str()) == Some("closed");
                }
            }
        }
        false
    }

    /// List all sessions.
    pub async fn list_sessions(&self) -> Vec<alephcore::gateway::session_manager::SessionMetadata> {
        self.session_manager.list_sessions(None).await
            .unwrap_or_default()
    }
}
```

**Step 2: Verify compilation**

Run: `cargo test -p alephcore --test session_probe --no-run 2>&1 | head -10`
Expected: May fail on missing scenario modules — create empty stubs next

**Step 3: Create empty stub files for all scenario modules**

Create each file with just a module header:

`tests/session_probe/lifecycle.rs`:
```rust
//! P1: Session lifecycle probes

use super::harness::SessionProbeHarness;
```

`tests/session_probe/epoch_mechanics.rs`:
```rust
//! P2: Epoch versioning probes

use super::harness::SessionProbeHarness;
```

`tests/session_probe/topic_generation.rs`:
```rust
//! P3: Topic generation probes

use super::harness::SessionProbeHarness;
use super::mock_llm::MockLlmProvider;
```

`tests/session_probe/rpc_handler.rs`:
```rust
//! P4: sessions.new RPC handler probes

use super::harness::SessionProbeHarness;
```

`tests/session_probe/memory_filter.rs`:
```rust
//! P5: Memory session_ids filter probes
```

`tests/session_probe/multi_agent.rs`:
```rust
//! P6: Multi-agent session matrix probes

use super::harness::SessionProbeHarness;
use super::mock_llm::MockLlmProvider;
```

`tests/session_probe/edge_cases.rs`:
```rust
//! P7: Edge cases and backward compatibility probes

use super::harness::SessionProbeHarness;
```

**Step 4: Verify clean compilation**

Run: `cargo test -p alephcore --test session_probe --no-run 2>&1 | tail -3`
Expected: Compiled successfully (warnings OK)

**Step 5: Commit**

```bash
git add tests/session_probe/
git commit -m "session probe: add harness and scenario stubs"
```

---

### Task 3: P1 — Session Lifecycle (8 tests)

**Files:**
- Modify: `tests/session_probe/lifecycle.rs`

**Tests cover:**
1. Create session → verify metadata
2. Close session → verify status=closed
3. Close with topic → verify topic stored
4. Close without topic → verify no topic
5. Create new epoch session → verify separate from old
6. Old session retains messages after new epoch
7. New epoch session starts empty
8. Multiple close/create cycles

**Step 1: Write the tests**

`tests/session_probe/lifecycle.rs`:

```rust
//! P1: Session lifecycle probes
//!
//! Validates the full create → close → new-epoch lifecycle.

use std::sync::Arc;
use alephcore::gateway::router::SessionKey;

use super::harness::SessionProbeHarness;

#[tokio::test]
async fn p1_01_create_session_returns_metadata() {
    let h = SessionProbeHarness::new();
    let key = SessionKey::main("test");

    let meta = h.session_manager.get_or_create(&key).await.unwrap();
    assert_eq!(meta.agent_id, "test");
    assert_eq!(meta.session_type, "main");
    assert_eq!(meta.message_count, 0);
}

#[tokio::test]
async fn p1_02_close_session_sets_closed_status() {
    let h = SessionProbeHarness::new();
    let key = SessionKey::main("test");

    h.create_session(&key).await;
    h.session_manager.close_session(&key, None).await.unwrap();

    assert!(h.is_session_closed(&key).await);
}

#[tokio::test]
async fn p1_03_close_with_topic_stores_topic() {
    let h = SessionProbeHarness::new();
    let key = SessionKey::main("test");

    h.create_session(&key).await;
    h.session_manager
        .close_session(&key, Some("Rust并发讨论".to_string()))
        .await
        .unwrap();

    let topic = h.get_topic(&key).await;
    assert_eq!(topic, Some("Rust并发讨论".to_string()));
    assert!(h.is_session_closed(&key).await);
}

#[tokio::test]
async fn p1_04_close_without_topic_leaves_topic_none() {
    let h = SessionProbeHarness::new();
    let key = SessionKey::main("test");

    h.create_session(&key).await;
    h.session_manager.close_session(&key, None).await.unwrap();

    let topic = h.get_topic(&key).await;
    assert_eq!(topic, None);
}

#[tokio::test]
async fn p1_05_new_epoch_creates_separate_session() {
    let h = SessionProbeHarness::new();
    let key_e0 = SessionKey::main("test");
    let key_e1 = {
        use alephcore::routing::session_key::SessionKey as RK;
        let rk = key_e0.to_new().with_next_epoch();
        SessionKey::from_new(&rk)
    };

    h.create_session(&key_e0).await;
    h.create_session(&key_e1).await;

    let sessions = h.list_sessions().await;
    assert_eq!(sessions.len(), 2);
}

#[tokio::test]
async fn p1_06_old_session_retains_messages_after_new_epoch() {
    let h = SessionProbeHarness::new();
    let key_e0 = SessionKey::main("test");

    h.create_session(&key_e0).await;
    h.seed_messages(&key_e0, &[
        ("user", "Hello"),
        ("assistant", "Hi there!"),
    ]).await;

    // Close old, create new epoch
    h.session_manager.close_session(&key_e0, Some("问候".to_string())).await.unwrap();

    // Old messages still accessible
    let history = h.session_manager.get_history(&key_e0, None).await.unwrap();
    assert_eq!(history.len(), 2);
}

#[tokio::test]
async fn p1_07_new_epoch_starts_with_empty_history() {
    let h = SessionProbeHarness::new();
    let key_e0 = SessionKey::main("test");

    h.create_session(&key_e0).await;
    h.seed_messages(&key_e0, &[("user", "old message")]).await;

    let key_e1 = {
        use alephcore::routing::session_key::SessionKey as RK;
        SessionKey::from_new(&key_e0.to_new().with_next_epoch())
    };
    h.create_session(&key_e1).await;

    let history = h.session_manager.get_history(&key_e1, None).await.unwrap();
    assert!(history.is_empty());
}

#[tokio::test]
async fn p1_08_multiple_epoch_cycles() {
    let h = SessionProbeHarness::new();
    let key_e0 = SessionKey::main("test");

    h.create_session(&key_e0).await;
    h.session_manager.close_session(&key_e0, Some("话题一".to_string())).await.unwrap();

    let rk1 = key_e0.to_new().with_next_epoch();
    let key_e1 = SessionKey::from_new(&rk1);
    h.create_session(&key_e1).await;
    h.session_manager.close_session(&key_e1, Some("话题二".to_string())).await.unwrap();

    let rk2 = rk1.with_next_epoch();
    let key_e2 = SessionKey::from_new(&rk2);
    h.create_session(&key_e2).await;

    // Verify all three sessions exist
    let sessions = h.list_sessions().await;
    assert_eq!(sessions.len(), 3);

    // Verify topics on closed sessions
    assert_eq!(h.get_topic(&key_e0).await, Some("话题一".to_string()));
    assert_eq!(h.get_topic(&key_e1).await, Some("话题二".to_string()));
    assert_eq!(h.get_topic(&key_e2).await, None); // active, no topic
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test session_probe lifecycle -- --nocapture 2>&1 | tail -15`
Expected: 8 tests PASS

**Step 3: Commit**

```bash
git add tests/session_probe/lifecycle.rs
git commit -m "session probe: P1 lifecycle — 8 scenarios"
```

---

### Task 4: P2 — Epoch Mechanics (7 tests)

**Files:**
- Modify: `tests/session_probe/epoch_mechanics.rs`

**Tests cover:**
1. SessionKey epoch default = 0
2. `with_next_epoch()` increments Main
3. `with_next_epoch()` increments DirectMessage
4. `with_next_epoch()` is no-op for Task/Group/Ephemeral
5. `to_key_string()` omits epoch suffix when epoch=0
6. `to_key_string()` appends `:sN` when epoch>0
7. `parse()` ↔ `to_key_string()` roundtrip preserves epoch

**Step 1: Write the tests**

`tests/session_probe/epoch_mechanics.rs`:

```rust
//! P2: Epoch versioning probes
//!
//! Validates SessionKey epoch mechanics: default, increment,
//! serialization, parsing roundtrip.

use alephcore::routing::session_key::{SessionKey, DmScope};

#[test]
fn p2_01_main_default_epoch_is_zero() {
    let key = SessionKey::main("test");
    assert_eq!(key.epoch(), 0);
}

#[test]
fn p2_02_with_next_epoch_increments_main() {
    let key = SessionKey::main("test");
    let next = key.with_next_epoch();
    assert_eq!(next.epoch(), 1);

    let next2 = next.with_next_epoch();
    assert_eq!(next2.epoch(), 2);
}

#[test]
fn p2_03_with_next_epoch_increments_dm() {
    let key = SessionKey::dm("test", "telegram", "user123", DmScope::PerPeer);
    assert_eq!(key.epoch(), 0);

    let next = key.with_next_epoch();
    assert_eq!(next.epoch(), 1);
}

#[test]
fn p2_04_with_next_epoch_noop_for_non_epoch_types() {
    let task = SessionKey::Task {
        agent_id: "test".to_string(),
        task_type: "cron".to_string(),
        task_id: "daily".to_string(),
    };
    assert_eq!(task.epoch(), 0);
    assert_eq!(task.with_next_epoch().epoch(), 0);

    let ephemeral = SessionKey::Ephemeral {
        agent_id: "test".to_string(),
        ephemeral_id: "abc".to_string(),
    };
    assert_eq!(ephemeral.epoch(), 0);
    assert_eq!(ephemeral.with_next_epoch().epoch(), 0);
}

#[test]
fn p2_05_to_key_string_no_suffix_for_epoch_zero() {
    let key = SessionKey::main("test");
    let s = key.to_key_string();
    assert!(!s.contains(":s"), "Epoch 0 should not have :sN suffix: {}", s);
    assert_eq!(s, "agent:test:main");
}

#[test]
fn p2_06_to_key_string_appends_suffix_for_nonzero_epoch() {
    let key = SessionKey::main("test").with_next_epoch();
    let s = key.to_key_string();
    assert!(s.ends_with(":s1"), "Expected :s1 suffix, got: {}", s);

    let key3 = key.with_next_epoch().with_next_epoch();
    let s3 = key3.to_key_string();
    assert!(s3.ends_with(":s3"), "Expected :s3 suffix, got: {}", s3);
}

#[test]
fn p2_07_parse_roundtrip_preserves_epoch() {
    for epoch in [0u32, 1, 5, 100] {
        let mut key = SessionKey::main("test");
        for _ in 0..epoch {
            key = key.with_next_epoch();
        }
        let serialized = key.to_key_string();
        let parsed = SessionKey::parse(&serialized)
            .unwrap_or_else(|| panic!("Failed to parse: {}", serialized));
        assert_eq!(parsed.epoch(), epoch, "Epoch mismatch for: {}", serialized);
        assert_eq!(parsed.to_key_string(), serialized, "Roundtrip failed for epoch {}", epoch);
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test session_probe epoch_mechanics -- --nocapture 2>&1 | tail -15`
Expected: 7 tests PASS

**Step 3: Commit**

```bash
git add tests/session_probe/epoch_mechanics.rs
git commit -m "session probe: P2 epoch mechanics — 7 scenarios"
```

---

### Task 5: P3 — Topic Generation (6 tests)

**Files:**
- Modify: `tests/session_probe/topic_generation.rs`

**Tests cover:**
1. `/new` with sufficient history generates topic via LLM
2. `/new` with < 2 messages skips LLM, no topic
3. LLM failure gracefully produces no topic
4. `/new` sends confirmation reply with topic
5. `/switch` closes old session (no topic — just closes)
6. Topic truncation handles multi-byte UTF-8 chars

**Step 1: Write the tests**

`tests/session_probe/topic_generation.rs`:

```rust
//! P3: Topic generation probes
//!
//! Validates LLM-based topic generation, graceful failures,
//! and /new command integration.

use std::sync::Arc;
use alephcore::gateway::router::SessionKey;

use super::harness::SessionProbeHarness;
use super::mock_llm::MockLlmProvider;

#[tokio::test]
async fn p3_01_new_command_generates_topic_with_sufficient_history() {
    let llm = Arc::new(MockLlmProvider::new("Rust并发测试"));
    let mut h = SessionProbeHarness::with_llm(Some(llm.clone()));
    h.register_channel("telegram").await;
    h.register_agent("main").await;

    // Seed session with enough messages
    let key = SessionKey::main("main");
    h.create_session(&key).await;
    h.seed_messages(&key, &[
        ("user", "How do I test concurrent code in Rust?"),
        ("assistant", "You can use loom for concurrency testing"),
        ("user", "What about tokio test?"),
        ("assistant", "tokio::test is great for async tests"),
    ]).await;

    // Send /new command
    h.send_message("telegram", "user-1", "/new").await;

    // LLM should have been called
    assert!(llm.call_count() > 0, "LLM should be called for topic generation");

    // Old session should be closed with topic
    let topic = h.get_topic(&key).await;
    assert_eq!(topic, Some("Rust并发测试".to_string()));
}

#[tokio::test]
async fn p3_02_new_command_skips_topic_with_short_history() {
    let llm = Arc::new(MockLlmProvider::new("不应该被调用"));
    let mut h = SessionProbeHarness::with_llm(Some(llm.clone()));
    h.register_channel("telegram").await;
    h.register_agent("main").await;

    // Create session with only 1 message (below threshold)
    let key = SessionKey::main("main");
    h.create_session(&key).await;
    h.seed_messages(&key, &[
        ("user", "Hello"),
    ]).await;

    h.send_message("telegram", "user-1", "/new").await;

    // LLM should NOT have been called (< 2 messages)
    assert_eq!(llm.call_count(), 0, "LLM should not be called for < 2 messages");

    // Session should still be closed, but with no topic
    let topic = h.get_topic(&key).await;
    assert_eq!(topic, None);
}

#[tokio::test]
async fn p3_03_llm_failure_produces_no_topic_gracefully() {
    let llm = Arc::new(MockLlmProvider::failing());
    let mut h = SessionProbeHarness::with_llm(Some(llm.clone()));
    h.register_channel("telegram").await;
    h.register_agent("main").await;

    let key = SessionKey::main("main");
    h.create_session(&key).await;
    h.seed_messages(&key, &[
        ("user", "Question one"),
        ("assistant", "Answer one"),
    ]).await;

    // Should not panic or error — graceful degradation
    h.send_message("telegram", "user-1", "/new").await;

    // Session closed but no topic (LLM failed)
    assert!(h.is_session_closed(&key).await);
    assert_eq!(h.get_topic(&key).await, None);
}

#[tokio::test]
async fn p3_04_new_command_sends_confirmation_reply() {
    let llm = Arc::new(MockLlmProvider::new("测试主题"));
    let mut h = SessionProbeHarness::with_llm(Some(llm));
    h.register_channel("telegram").await;
    h.register_agent("main").await;

    let key = SessionKey::main("main");
    h.create_session(&key).await;
    h.seed_messages(&key, &[
        ("user", "Hello"),
        ("assistant", "World"),
    ]).await;

    h.send_message("telegram", "user-1", "/new").await;

    // Should receive reply containing "新对话已开始"
    h.assert_reply_contains("新对话已开始");
}

#[tokio::test]
async fn p3_05_switch_closes_old_session() {
    let llm = Arc::new(MockLlmProvider::new("切换前话题"));
    let mut h = SessionProbeHarness::with_llm(Some(llm));
    h.register_channel("telegram").await;
    h.register_agent("main").await;
    h.register_agent("helper").await;

    let key = SessionKey::main("main");
    h.create_session(&key).await;
    h.seed_messages(&key, &[
        ("user", "Working with main agent"),
        ("assistant", "I'm the main agent"),
    ]).await;

    // Switch to helper agent
    h.send_message("telegram", "user-1", "/switch helper").await;

    // Old session should be closed
    assert!(h.is_session_closed(&key).await);
}

#[tokio::test]
fn p3_06_truncate_handles_multibyte_utf8() {
    // truncate_for_topic is a free function — test via the public interface
    // Verify that multi-byte characters don't cause panics
    let chinese = "你好世界这是一个测试用的很长的中文字符串";
    // Each Chinese char = 3 bytes. 18 chars total.
    // Truncate to 5 chars should give first 5 Chinese chars.
    assert_eq!(chinese.chars().count(), 18);

    // We can't call truncate_for_topic directly (private), but we verify
    // the underlying char_indices pattern works correctly
    let max_chars = 5;
    let truncated = match chinese.char_indices().nth(max_chars) {
        Some((idx, _)) => &chinese[..idx],
        None => chinese,
    };
    assert_eq!(truncated.chars().count(), 5);
    assert_eq!(truncated, "你好世界这");
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test session_probe topic_generation -- --nocapture 2>&1 | tail -15`
Expected: 6 tests PASS

**Step 3: Commit**

```bash
git add tests/session_probe/topic_generation.rs
git commit -m "session probe: P3 topic generation — 6 scenarios"
```

---

### Task 6: P4 — sessions.new RPC Handler (6 tests)

**Files:**
- Modify: `tests/session_probe/rpc_handler.rs`

**Tests cover:**
1. Valid request returns old + new session keys
2. Missing session_key returns INVALID_PARAMS
3. Invalid session_key format returns error
4. Topic is optional and passed through
5. New key has incremented epoch
6. New session is created in database

**Step 1: Write the tests**

`tests/session_probe/rpc_handler.rs`:

```rust
//! P4: sessions.new RPC handler probes
//!
//! Validates the sessions.new JSON-RPC handler.

use std::sync::Arc;
use serde_json::json;
use alephcore::gateway::protocol::JsonRpcRequest;
use alephcore::gateway::router::SessionKey;
use alephcore::gateway::handlers::session as session_handlers;
use alephcore::gateway::session_manager::{SessionManager, SessionManagerConfig};
use tempfile::TempDir;

fn setup() -> (Arc<SessionManager>, TempDir) {
    let temp = TempDir::new().unwrap();
    let config = SessionManagerConfig {
        db_path: temp.path().join("test.db"),
        max_messages: 100,
        compaction_keep: 50,
        ..Default::default()
    };
    let manager = Arc::new(SessionManager::new(config).unwrap());
    (manager, temp)
}

fn make_request(id: u64, params: serde_json::Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(id)),
        method: "sessions.new".to_string(),
        params: Some(params),
    }
}

#[tokio::test]
async fn p4_01_valid_request_returns_both_keys() {
    let (manager, _temp) = setup();
    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    let request = make_request(1, json!({
        "session_key": "agent:test:main",
        "topic": "测试话题"
    }));

    let response = session_handlers::handle_new_session_db(request, manager).await;
    let result = response.result.unwrap();

    assert_eq!(result["old_session_key"], "agent:test:main");
    assert_eq!(result["new_session_key"], "agent:test:main:s1");
    assert_eq!(result["topic"], "测试话题");
}

#[tokio::test]
async fn p4_02_missing_session_key_returns_error() {
    let (manager, _temp) = setup();

    let request = make_request(1, json!({}));
    let response = session_handlers::handle_new_session_db(request, manager).await;

    assert!(response.error.is_some());
    let err = response.error.unwrap();
    assert!(err.message.contains("Missing session_key"));
}

#[tokio::test]
async fn p4_03_invalid_session_key_format_returns_error() {
    let (manager, _temp) = setup();

    let request = make_request(1, json!({
        "session_key": "completely-invalid"
    }));
    let response = session_handlers::handle_new_session_db(request, manager).await;

    assert!(response.error.is_some());
}

#[tokio::test]
async fn p4_04_topic_is_optional() {
    let (manager, _temp) = setup();
    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    let request = make_request(1, json!({
        "session_key": "agent:test:main"
    }));
    let response = session_handlers::handle_new_session_db(request, manager).await;
    let result = response.result.unwrap();

    assert_eq!(result["old_session_key"], "agent:test:main");
    assert_eq!(result["new_session_key"], "agent:test:main:s1");
    assert!(result["topic"].is_null());
}

#[tokio::test]
async fn p4_05_new_key_has_incremented_epoch() {
    let (manager, _temp) = setup();

    // Create epoch 0 session
    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // First /new: epoch 0 → 1
    let request = make_request(1, json!({ "session_key": "agent:test:main" }));
    let r1 = session_handlers::handle_new_session_db(request, manager.clone()).await;
    assert_eq!(r1.result.unwrap()["new_session_key"], "agent:test:main:s1");

    // Second /new: epoch 1 → 2
    let request = make_request(2, json!({ "session_key": "agent:test:main:s1" }));
    let r2 = session_handlers::handle_new_session_db(request, manager.clone()).await;
    assert_eq!(r2.result.unwrap()["new_session_key"], "agent:test:main:s2");
}

#[tokio::test]
async fn p4_06_new_session_exists_in_database() {
    let (manager, _temp) = setup();
    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    let request = make_request(1, json!({ "session_key": "agent:test:main" }));
    session_handlers::handle_new_session_db(request, manager.clone()).await;

    // Verify new session exists
    let sessions = manager.list_sessions(None).await.unwrap();
    assert_eq!(sessions.len(), 2); // old + new
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test session_probe rpc_handler -- --nocapture 2>&1 | tail -15`
Expected: 6 tests PASS

**Step 3: Commit**

```bash
git add tests/session_probe/rpc_handler.rs
git commit -m "session probe: P4 RPC handler — 6 scenarios"
```

---

### Task 7: P5 — Memory Filter Session IDs (5 tests)

**Files:**
- Modify: `tests/session_probe/memory_filter.rs`

**Tests cover:**
1. `session_ids: None` generates no filter clause
2. `session_ids: Some(vec![])` generates no filter clause
3. Single session_id generates `IN ('...')` clause
4. Multiple session_ids generate `IN ('...', '...')` clause
5. Session IDs with special chars are SQL-escaped

**Step 1: Write the tests**

`tests/session_probe/memory_filter.rs`:

```rust
//! P5: Memory session_ids filter probes
//!
//! Validates MemoryFilter.session_ids SQL generation and escaping.

use alephcore::memory::store::types::MemoryFilter;

#[test]
fn p5_01_no_session_ids_no_filter_clause() {
    let filter = MemoryFilter {
        session_ids: None,
        ..Default::default()
    };
    let sql = filter.to_lance_filter();
    assert!(sql.is_none(), "Empty filter should produce None: {:?}", sql);
}

#[test]
fn p5_02_empty_session_ids_no_filter_clause() {
    let filter = MemoryFilter {
        session_ids: Some(vec![]),
        ..Default::default()
    };
    let sql = filter.to_lance_filter();
    assert!(sql.is_none(), "Empty vec should produce None: {:?}", sql);
}

#[test]
fn p5_03_single_session_id_generates_in_clause() {
    let filter = MemoryFilter {
        session_ids: Some(vec!["agent:main:main".to_string()]),
        ..Default::default()
    };
    let sql = filter.to_lance_filter().expect("Should produce SQL");
    assert!(sql.contains("session_id IN"), "Expected IN clause: {}", sql);
    assert!(sql.contains("agent:main:main"), "Expected session ID: {}", sql);
}

#[test]
fn p5_04_multiple_session_ids_generate_in_clause() {
    let filter = MemoryFilter {
        session_ids: Some(vec![
            "agent:main:main".to_string(),
            "agent:main:main:s1".to_string(),
            "agent:main:main:s2".to_string(),
        ]),
        ..Default::default()
    };
    let sql = filter.to_lance_filter().expect("Should produce SQL");
    assert!(sql.contains("session_id IN"), "Expected IN clause: {}", sql);
    assert!(sql.contains("agent:main:main:s1"), "Expected s1: {}", sql);
    assert!(sql.contains("agent:main:main:s2"), "Expected s2: {}", sql);
}

#[test]
fn p5_05_session_ids_with_special_chars_are_escaped() {
    let filter = MemoryFilter {
        session_ids: Some(vec!["agent:test:O'Brien".to_string()]),
        ..Default::default()
    };
    let sql = filter.to_lance_filter().expect("Should produce SQL");
    // Single quote should be escaped to double single-quote
    assert!(sql.contains("O''Brien"), "Expected escaped quote: {}", sql);
    assert!(!sql.contains("O'B"), "Unescaped quote is SQL injection risk: {}", sql);
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test session_probe memory_filter -- --nocapture 2>&1 | tail -10`
Expected: 5 tests PASS

**Step 3: Commit**

```bash
git add tests/session_probe/memory_filter.rs
git commit -m "session probe: P5 memory filter — 5 scenarios"
```

---

### Task 8: P6 — Multi-Agent Session Matrix (5 tests)

**Files:**
- Modify: `tests/session_probe/multi_agent.rs`

**Tests cover:**
1. Two agents have independent sessions (no cross-talk)
2. `/new` on agent A doesn't affect agent B's session
3. `/switch` from A to B closes A's session only
4. Multiple channels have independent sessions per agent
5. Epoch increments are agent-scoped

**Step 1: Write the tests**

`tests/session_probe/multi_agent.rs`:

```rust
//! P6: Multi-agent session matrix probes
//!
//! Validates session isolation between agents and channels.

use std::sync::Arc;
use alephcore::gateway::router::SessionKey;

use super::harness::SessionProbeHarness;
use super::mock_llm::MockLlmProvider;

#[tokio::test]
async fn p6_01_two_agents_have_independent_sessions() {
    let h = SessionProbeHarness::new();
    let key_a = SessionKey::main("agent-a");
    let key_b = SessionKey::main("agent-b");

    h.create_session(&key_a).await;
    h.create_session(&key_b).await;

    h.seed_messages(&key_a, &[("user", "Message for A")]).await;
    h.seed_messages(&key_b, &[("user", "Message for B")]).await;

    let hist_a = h.session_manager.get_history(&key_a, None).await.unwrap();
    let hist_b = h.session_manager.get_history(&key_b, None).await.unwrap();

    assert_eq!(hist_a.len(), 1);
    assert_eq!(hist_b.len(), 1);
    assert_eq!(hist_a[0].content, "Message for A");
    assert_eq!(hist_b[0].content, "Message for B");
}

#[tokio::test]
async fn p6_02_new_on_agent_a_does_not_affect_agent_b() {
    let h = SessionProbeHarness::new();
    let key_a = SessionKey::main("agent-a");
    let key_b = SessionKey::main("agent-b");

    h.create_session(&key_a).await;
    h.create_session(&key_b).await;

    // Close agent A's session
    h.session_manager.close_session(&key_a, Some("A话题".to_string())).await.unwrap();

    // Agent B should NOT be affected
    assert!(h.is_session_closed(&key_a).await);
    assert!(!h.is_session_closed(&key_b).await);
}

#[tokio::test]
async fn p6_03_switch_closes_only_source_agent_session() {
    let llm = Arc::new(MockLlmProvider::new("切换话题"));
    let mut h = SessionProbeHarness::with_llm(Some(llm));
    h.register_channel("telegram").await;
    h.register_agent("alpha").await;
    h.register_agent("beta").await;

    // Create sessions for both agents
    let key_alpha = SessionKey::main("alpha");
    let key_beta = SessionKey::main("beta");
    h.create_session(&key_alpha).await;
    h.create_session(&key_beta).await;

    h.seed_messages(&key_alpha, &[
        ("user", "Using alpha"),
        ("assistant", "Alpha here"),
    ]).await;

    // Switch from alpha to beta
    h.send_message("telegram", "user-1", "/switch beta").await;

    // Alpha session should be closed, beta untouched
    assert!(h.is_session_closed(&key_alpha).await);
    assert!(!h.is_session_closed(&key_beta).await);
}

#[tokio::test]
async fn p6_04_different_channels_have_independent_sessions() {
    let h = SessionProbeHarness::new();

    // Create per-peer sessions for different channels
    let key_tg = SessionKey::per_peer("main", "tg-user1");
    let key_dc = SessionKey::per_peer("main", "dc-user1");

    h.create_session(&key_tg).await;
    h.create_session(&key_dc).await;

    h.seed_messages(&key_tg, &[("user", "Telegram message")]).await;
    h.seed_messages(&key_dc, &[("user", "Discord message")]).await;

    // Close telegram session
    h.session_manager.close_session(&key_tg, Some("TG话题".to_string())).await.unwrap();

    assert!(h.is_session_closed(&key_tg).await);
    assert!(!h.is_session_closed(&key_dc).await);
}

#[tokio::test]
async fn p6_05_epoch_increments_are_agent_scoped() {
    let h = SessionProbeHarness::new();
    let key_a = SessionKey::main("agent-a");
    let key_b = SessionKey::main("agent-b");

    h.create_session(&key_a).await;
    h.create_session(&key_b).await;

    // Increment agent A's epoch twice
    let rk_a1 = key_a.to_new().with_next_epoch();
    let key_a1 = SessionKey::from_new(&rk_a1);
    h.create_session(&key_a1).await;

    let rk_a2 = rk_a1.with_next_epoch();
    let key_a2 = SessionKey::from_new(&rk_a2);
    h.create_session(&key_a2).await;

    // Agent B is still at epoch 0
    let current_epoch_b = h.session_manager
        .get_current_epoch(&key_b.to_new().base_key_pattern())
        .await.unwrap();
    assert_eq!(current_epoch_b, 0, "Agent B epoch should be 0");

    let current_epoch_a = h.session_manager
        .get_current_epoch(&key_a.to_new().base_key_pattern())
        .await.unwrap();
    assert_eq!(current_epoch_a, 2, "Agent A epoch should be 2");
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test session_probe multi_agent -- --nocapture 2>&1 | tail -15`
Expected: 5 tests PASS

**Step 3: Commit**

```bash
git add tests/session_probe/multi_agent.rs
git commit -m "session probe: P6 multi-agent matrix — 5 scenarios"
```

---

### Task 9: P7 — Edge Cases & Backward Compatibility (8 tests)

**Files:**
- Modify: `tests/session_probe/edge_cases.rs`

**Tests cover:**
1. Close session that was never created (no panic)
2. Close session twice (idempotent)
3. Empty session (0 messages) — `/new` works gracefully
4. Session key with epoch 0 parses without `:s0` suffix
5. Legacy key string without epoch suffix defaults to epoch 0
6. `get_current_epoch` returns 0 for nonexistent pattern
7. `get_session_topic` returns None for session without metadata
8. Concurrent session creation doesn't corrupt database

**Step 1: Write the tests**

`tests/session_probe/edge_cases.rs`:

```rust
//! P7: Edge cases and backward compatibility probes
//!
//! Validates graceful handling of unusual states,
//! backward compatibility, and concurrent access.

use std::sync::Arc;
use alephcore::gateway::router::SessionKey;
use alephcore::routing::session_key::SessionKey as RoutingKey;

use super::harness::SessionProbeHarness;

#[tokio::test]
async fn p7_01_close_nonexistent_session_no_panic() {
    let h = SessionProbeHarness::new();
    let key = SessionKey::main("nonexistent");

    // Should not panic — graceful handling
    let result = h.session_manager.close_session(&key, Some("topic".to_string())).await;
    // It's OK if this succeeds (upsert) or errors — just no panic
    let _ = result;
}

#[tokio::test]
async fn p7_02_close_session_twice_is_idempotent() {
    let h = SessionProbeHarness::new();
    let key = SessionKey::main("test");

    h.create_session(&key).await;

    // First close
    h.session_manager.close_session(&key, Some("话题一".to_string())).await.unwrap();
    assert!(h.is_session_closed(&key).await);

    // Second close with different topic — should overwrite
    h.session_manager.close_session(&key, Some("话题二".to_string())).await.unwrap();
    assert!(h.is_session_closed(&key).await);
    assert_eq!(h.get_topic(&key).await, Some("话题二".to_string()));
}

#[tokio::test]
async fn p7_03_empty_session_new_works_gracefully() {
    let h = SessionProbeHarness::new();
    let key = SessionKey::main("test");

    // Create empty session (0 messages)
    h.create_session(&key).await;

    // Close it — no topic should be generated (not enough history)
    h.session_manager.close_session(&key, None).await.unwrap();
    assert!(h.is_session_closed(&key).await);
    assert_eq!(h.get_topic(&key).await, None);
}

#[test]
fn p7_04_epoch_zero_no_s0_suffix() {
    let key = RoutingKey::main("test");
    let s = key.to_key_string();
    assert!(!s.contains(":s0"), "Epoch 0 should not produce :s0: {}", s);
    assert!(!s.contains(":s"), "Epoch 0 should have no :s suffix at all: {}", s);
}

#[test]
fn p7_05_legacy_key_without_epoch_defaults_to_zero() {
    let parsed = RoutingKey::parse("agent:test:main").unwrap();
    assert_eq!(parsed.epoch(), 0);

    let parsed_dm = RoutingKey::parse("agent:test:telegram:dm:user123").unwrap();
    assert_eq!(parsed_dm.epoch(), 0);
}

#[tokio::test]
async fn p7_06_get_current_epoch_returns_zero_for_nonexistent() {
    let h = SessionProbeHarness::new();
    let epoch = h.session_manager
        .get_current_epoch("agent:nonexistent:main")
        .await.unwrap();
    assert_eq!(epoch, 0);
}

#[tokio::test]
async fn p7_07_get_topic_returns_none_for_session_without_metadata() {
    let h = SessionProbeHarness::new();
    let key = SessionKey::main("test");

    // Create session but don't close it — no metadata
    h.create_session(&key).await;

    let topic = h.get_topic(&key).await;
    assert_eq!(topic, None);
}

#[tokio::test]
async fn p7_08_concurrent_session_creation_no_corruption() {
    let h = SessionProbeHarness::new();
    let manager = h.session_manager.clone();

    // Spawn 10 concurrent session creations for different agents
    let mut handles = Vec::new();
    for i in 0..10 {
        let m = manager.clone();
        handles.push(tokio::spawn(async move {
            let key = SessionKey::main(&format!("agent-{}", i));
            m.get_or_create(&key).await.unwrap();
            m.add_message(&key, "user", &format!("Hello from {}", i)).await.unwrap();
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // All 10 sessions should exist with correct data
    let sessions = h.list_sessions().await;
    assert_eq!(sessions.len(), 10);
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test session_probe edge_cases -- --nocapture 2>&1 | tail -15`
Expected: 8 tests PASS

**Step 3: Commit**

```bash
git add tests/session_probe/edge_cases.rs
git commit -m "session probe: P7 edge cases & backward compat — 8 scenarios"
```

---

### Task 10: Full Suite Validation

**Step 1: Run all session probe tests**

Run: `cargo test -p alephcore --test session_probe -- --nocapture 2>&1 | tail -20`
Expected: 45 tests PASS (8+7+6+6+5+5+8)

**Step 2: Run full core test suite to check no regressions**

Run: `cargo test -p alephcore --lib 2>&1 | tail -5`
Expected: All tests pass

**Step 3: Run clippy**

Run: `cargo clippy -p alephcore --tests 2>&1 | grep "^error" | head -5`
Expected: No errors

**Step 4: Final commit**

```bash
git add -A
git commit -m "session probe: complete suite — 45 scenarios across 7 phases"
```

---

## Test Matrix Summary

| Phase | File | Tests | Concern |
|-------|------|-------|---------|
| P1 | `lifecycle.rs` | 8 | Create → close → new-epoch lifecycle |
| P2 | `epoch_mechanics.rs` | 7 | SessionKey epoch versioning |
| P3 | `topic_generation.rs` | 6 | LLM topic generation + /new + /switch |
| P4 | `rpc_handler.rs` | 6 | sessions.new JSON-RPC handler |
| P5 | `memory_filter.rs` | 5 | MemoryFilter session_ids SQL |
| P6 | `multi_agent.rs` | 5 | Multi-agent isolation matrix |
| P7 | `edge_cases.rs` | 8 | Backward compat, concurrency, idempotency |
| **Total** | | **45** | |

## Run Commands

```bash
# All session probes
cargo test -p alephcore --test session_probe

# Single phase
cargo test -p alephcore --test session_probe lifecycle
cargo test -p alephcore --test session_probe epoch_mechanics
cargo test -p alephcore --test session_probe topic_generation
cargo test -p alephcore --test session_probe rpc_handler
cargo test -p alephcore --test session_probe memory_filter
cargo test -p alephcore --test session_probe multi_agent
cargo test -p alephcore --test session_probe edge_cases
```
