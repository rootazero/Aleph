//! Session probe test harness.
//!
//! Wraps SessionManager, InboundMessageRouter, MockChannel, and registries
//! with helpers for session lifecycle, message sending, and assertions.

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
    GroupPolicy, InboundMessage, InboundMessageRouter, MessageId,
    RouterChannelConfig, RoutingConfig, RunStatus, SqlitePairingStore, UserId,
    WorkspaceManager, WorkspaceManagerConfig,
    SessionManager, SessionManagerConfig,
};
use alephcore::gateway::channel::{
    ChannelCapabilities, ChannelInfo, ChannelResult, ChannelState,
    ChannelStatus, OutboundMessage, SendResult,
};
use alephcore::gateway::execution_engine::{ExecutionError, RunState};
use alephcore::gateway::event_emitter::EventEmitter;
use alephcore::gateway::RunRequest;
use alephcore::gateway::router::SessionKey;

use super::mock_llm::MockLlmProvider;

// =============================================================================
// CapturedReply
// =============================================================================

/// A reply captured from a MockChannel for assertion.
#[derive(Debug, Clone)]
pub struct CapturedReply {
    pub conversation_id: String,
    pub text: String,
}

// =============================================================================
// MockChannel
// =============================================================================

/// Mock channel that captures outbound messages.
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
        Self {
            info,
            state,
            reply_tx,
        }
    }
}

#[async_trait]
impl Channel for MockChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.state
    }

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
// TrackingExecutionAdapter
// =============================================================================

/// Execution adapter that tracks calls without executing real logic.
pub struct TrackingExecutionAdapter {
    execute_count: AtomicU64,
}

impl std::fmt::Debug for TrackingExecutionAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackingExecutionAdapter")
            .field("execute_count", &self.execute_count.load(Ordering::SeqCst))
            .finish()
    }
}

impl TrackingExecutionAdapter {
    pub fn new() -> Self {
        Self {
            execute_count: AtomicU64::new(0),
        }
    }

    pub fn call_count(&self) -> u64 {
        self.execute_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl alephcore::gateway::ExecutionAdapter for TrackingExecutionAdapter {
    async fn execute(
        &self,
        _request: RunRequest,
        _agent: Arc<AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        self.execute_count.fetch_add(1, Ordering::SeqCst);
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
            steps_completed: 1,
            current_tool: None,
        })
    }
}

// =============================================================================
// SessionProbeHarness
// =============================================================================

/// Test harness for session isolation probe tests.
pub struct SessionProbeHarness {
    pub session_manager: Arc<SessionManager>,
    pub channel_registry: Arc<ChannelRegistry>,
    pub agent_registry: Arc<AgentRegistry>,
    pub workspace_manager: Arc<WorkspaceManager>,
    pub tracking_adapter: Arc<TrackingExecutionAdapter>,
    pub mock_llm: Option<Arc<MockLlmProvider>>,

    reply_rx: mpsc::UnboundedReceiver<CapturedReply>,
    reply_tx: mpsc::UnboundedSender<CapturedReply>,
    router: InboundMessageRouter,
    _temp_dir: TempDir,
    msg_counter: AtomicU64,
}

impl SessionProbeHarness {
    /// Build a fully wired harness with default (no) LLM.
    pub fn new() -> Self {
        Self::with_llm(None)
    }

    /// Build a harness with an optional MockLlmProvider.
    pub fn with_llm(mock_llm: Option<Arc<MockLlmProvider>>) -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Session manager (SQLite in temp dir)
        let sm_config = SessionManagerConfig {
            db_path: temp_dir.path().join("sessions.db"),
            max_messages: 100,
            compaction_keep: 50,
            auto_reset_hour: None,
            session_expiry_secs: 0,
        };
        let session_manager = Arc::new(
            SessionManager::new(sm_config).expect("Failed to create SessionManager"),
        );

        let channel_registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();
        let agent_registry = Arc::new(AgentRegistry::new());
        let tracking_adapter = Arc::new(TrackingExecutionAdapter::new());
        let adapter: Arc<dyn alephcore::gateway::ExecutionAdapter> = tracking_adapter.clone();
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

        let router = InboundMessageRouter::with_unified_routing(
            channel_registry.clone(),
            store,
            config,
            agent_registry.clone(),
            adapter,
            agent_router,
        )
        .with_workspace_manager(workspace_manager.clone())
        .with_session_manager(session_manager.clone());

        let (reply_tx, reply_rx) = mpsc::unbounded_channel();

        Self {
            session_manager,
            channel_registry,
            agent_registry,
            workspace_manager,
            tracking_adapter,
            mock_llm,
            reply_rx,
            reply_tx,
            router,
            _temp_dir: temp_dir,
            msg_counter: AtomicU64::new(0),
        }
    }

    // -------------------------------------------------------------------------
    // Channel helpers
    // -------------------------------------------------------------------------

    /// Register a mock channel with open DM/group policies.
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

    // -------------------------------------------------------------------------
    // Agent helpers
    // -------------------------------------------------------------------------

    /// Register an agent with optional link access whitelist.
    pub async fn register_agent(&self, id: &str, allowed_links: Option<Vec<String>>) {
        let config = AgentInstanceConfig {
            agent_id: id.to_string(),
            workspace: self._temp_dir.path().join(format!("ws-{}", id)),
            agent_dir: self._temp_dir.path().join(format!("agent-{}", id)),
            allowed_links,
            ..AgentInstanceConfig::default()
        };
        let instance = AgentInstance::new(config).expect("Failed to create AgentInstance");
        self.agent_registry.register(instance).await;
    }

    // -------------------------------------------------------------------------
    // Message helpers
    // -------------------------------------------------------------------------

    /// Send a DM-style message through the router.
    pub async fn send_message(&self, channel_id: &str, text: &str) {
        let msg = self.make_message(channel_id, text, false);
        let _ = self.router.handle_message(msg).await;
    }

    /// Send a group-style message through the router.
    pub async fn send_group_message(&self, channel_id: &str, text: &str) {
        let msg = self.make_message(channel_id, text, true);
        let _ = self.router.handle_message(msg).await;
    }

    fn make_message(&self, channel_id: &str, text: &str, is_group: bool) -> InboundMessage {
        let n = self.msg_counter.fetch_add(1, Ordering::SeqCst);
        InboundMessage {
            id: MessageId::new(format!("msg-{}", n)),
            channel_id: ChannelId::new(channel_id),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group,
            raw: None,
        }
    }

    // -------------------------------------------------------------------------
    // Session helpers
    // -------------------------------------------------------------------------

    /// Create a session via SessionManager and return its metadata.
    pub async fn create_session(
        &self,
        agent_id: &str,
    ) -> alephcore::gateway::session_manager::SessionMetadata {
        let key = SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: "main".to_string(),
        };
        self.session_manager
            .get_or_create(&key)
            .await
            .expect("Failed to create session")
    }

    /// Seed messages into a session.
    pub async fn seed_messages(&self, agent_id: &str, messages: &[(&str, &str)]) {
        let key = SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: "main".to_string(),
        };
        for (role, content) in messages {
            self.session_manager
                .add_message(&key, role, content)
                .await
                .expect("Failed to seed message");
        }
    }

    /// Get the topic for a session (from metadata JSON).
    pub async fn get_topic(&self, agent_id: &str) -> Option<String> {
        let key = SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: "main".to_string(),
        };
        self.session_manager
            .get_session_topic(&key)
            .await
            .unwrap_or(None)
    }

    /// Check whether a session is marked closed.
    pub async fn is_session_closed(&self, agent_id: &str) -> bool {
        let sessions = self
            .session_manager
            .list_sessions(Some(agent_id))
            .await
            .unwrap_or_default();
        sessions.iter().any(|s| {
            s.metadata_json
                .as_deref()
                .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
                .and_then(|v| v.get("status")?.as_str().map(|s| s == "closed"))
                .unwrap_or(false)
        })
    }

    /// List all sessions, optionally filtered by agent.
    pub async fn list_sessions(
        &self,
        agent_id: Option<&str>,
    ) -> Vec<alephcore::gateway::session_manager::SessionMetadata> {
        self.session_manager
            .list_sessions(agent_id)
            .await
            .unwrap_or_default()
    }

    // -------------------------------------------------------------------------
    // Reply assertion helpers
    // -------------------------------------------------------------------------

    /// Drain all captured replies from registered channels.
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
            "Expected a reply containing '{}', got: {:?}",
            substring,
            replies.iter().map(|r| &r.text).collect::<Vec<_>>()
        );
    }

    /// Assert that no replies were captured.
    pub fn assert_no_replies(&mut self) {
        let replies = self.drain_replies();
        assert!(
            replies.is_empty(),
            "Expected no replies, got {} replies: {:?}",
            replies.len(),
            replies.iter().map(|r| &r.text).collect::<Vec<_>>()
        );
    }
}
