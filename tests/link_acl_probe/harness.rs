//! Link ACL probe test harness.
//!
//! Wraps InboundMessageRouter with helpers for testing agent link access control.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tempfile::TempDir;
use tokio::sync::mpsc;

use alephcore::gateway::event_emitter::EventEmitter;
use alephcore::gateway::execution_engine::{ExecutionError, RunState};
use alephcore::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
use alephcore::gateway::session_store::SessionStore;
use alephcore::gateway::RunRequest;
use alephcore::gateway::{
    AgentEnvStore, AgentEnvStoreConfig, AgentInstance, AgentInstanceConfig, AgentRegistry, Channel,
    ChannelId, ChannelRegistry, ConversationId, DmPolicy, ExecutionAdapter, GroupPolicy,
    InboundMessage, InboundMessageRouter, MessageId, RouterChannelConfig, RoutingConfig, RunStatus,
    SqlitePairingStore, UserId,
};

use super::mock_channel::{CapturedReply, MockChannel};

// =============================================================================
// TrackingExecutionAdapter (duplicated from world/gateway_ctx.rs)
// =============================================================================

/// Tracking execution adapter that records whether execute() was called
pub struct TrackingExecutionAdapter {
    execute_called: AtomicBool,
    execute_count: AtomicUsize,
    should_fail: bool,
}

impl std::fmt::Debug for TrackingExecutionAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackingExecutionAdapter")
            .field(
                "execute_called",
                &self.execute_called.load(Ordering::SeqCst),
            )
            .field("execute_count", &self.execute_count.load(Ordering::SeqCst))
            .field("should_fail", &self.should_fail)
            .finish()
    }
}

impl TrackingExecutionAdapter {
    pub fn new() -> Self {
        Self {
            execute_called: AtomicBool::new(false),
            execute_count: AtomicUsize::new(0),
            should_fail: false,
        }
    }

    pub fn was_called(&self) -> bool {
        self.execute_called.load(Ordering::SeqCst)
    }

    pub fn call_count(&self) -> usize {
        self.execute_count.load(Ordering::SeqCst)
    }
}

impl Default for TrackingExecutionAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ExecutionAdapter for TrackingExecutionAdapter {
    async fn execute(
        &self,
        _request: RunRequest,
        _agent: Arc<AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        self.execute_called.store(true, Ordering::SeqCst);
        self.execute_count.fetch_add(1, Ordering::SeqCst);
        if self.should_fail {
            Err(ExecutionError::Failed("Mock failure".to_string()))
        } else {
            Ok(())
        }
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

    async fn active_run_count(&self) -> usize {
        0
    }
}

// =============================================================================
// LinkAclHarness
// =============================================================================

/// Test harness for link ACL probe tests.
pub struct LinkAclHarness {
    pub channel_registry: Arc<ChannelRegistry>,
    pub agent_registry: Arc<AgentRegistry>,
    pub workspace_manager: Arc<AgentEnvStore>,
    pub tracking_adapter: Arc<TrackingExecutionAdapter>,
    pub reply_rx: mpsc::UnboundedReceiver<CapturedReply>,
    reply_tx: mpsc::UnboundedSender<CapturedReply>,
    router: InboundMessageRouter,
    session_store: Arc<dyn SessionStore>,
    _temp_dir: TempDir,
    msg_counter: AtomicU64,
}

impl LinkAclHarness {
    /// Build a fully wired harness with InboundMessageRouter.
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        let channel_registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();
        let agent_registry = Arc::new(AgentRegistry::new());
        let tracking_adapter = Arc::new(TrackingExecutionAdapter::new());
        let adapter: Arc<dyn ExecutionAdapter> = tracking_adapter.clone();

        // Create agent env store with temp path
        let ws_config = AgentEnvStoreConfig {
            db_path: temp_dir.path().join("agent_envs.db"),
            default_profile: "default".to_string(),
        };
        let workspace_manager =
            Arc::new(AgentEnvStore::new(ws_config).expect("Failed to create AgentEnvStore"));

        let router = InboundMessageRouter::with_execution(
            channel_registry.clone(),
            store,
            config,
            agent_registry.clone(),
            adapter,
        )
        .with_workspace_manager(workspace_manager.clone());

        let (reply_tx, reply_rx) = mpsc::unbounded_channel();

        let session_store: Arc<dyn SessionStore> = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: temp_dir.path().join("sessions"),
                ..FileSessionStoreConfig::default()
            })
            .expect("Failed to create FileSessionStore"),
        );

        Self {
            channel_registry,
            agent_registry,
            workspace_manager,
            tracking_adapter,
            reply_rx,
            reply_tx,
            router,
            session_store,
            _temp_dir: temp_dir,
            msg_counter: AtomicU64::new(0),
        }
    }

    /// Register a mock channel (link) and set its DM/group policy to Open.
    pub async fn register_link(&mut self, link_id: &str) {
        let mut channel = MockChannel::new(link_id, self.reply_tx.clone());
        // Start the channel so it's connected
        channel.start().await.expect("Failed to start mock channel");
        // Register in channel registry
        self.channel_registry.register(Box::new(channel)).await;
        // Register channel config with open policies so messages are not blocked
        self.router.register_channel_config(
            link_id,
            RouterChannelConfig {
                dm_policy: DmPolicy::Open,
                group_policy: GroupPolicy::Open,
                allow_from: vec![],
                group_allow_from: vec![],
                require_mention: false,
                bot_name: None,
                // All-empty slash-access → gating OFF (equivalent to the prior
                // four empty ACL vecs now folded into SlashAccessConfig).
                slash_access: Default::default(),
                permission_level: Default::default(),
                busy_input_mode: Default::default(),
                default_workspace: None,
                tool_permissions: None,
            },
        );
    }

    /// Bind an agent to a channel (link).
    pub fn bind(&self, link_id: &str, agent_id: &str) {
        self.workspace_manager
            .set_active_agent(link_id, agent_id)
            .expect("Failed to bind agent to link");
    }

    /// Register an agent with optional link access whitelist.
    pub async fn register_agent(&self, id: &str, allowed_links: Option<Vec<String>>) {
        let config = AgentInstanceConfig {
            agent_id: id.to_string(),
            workspace: self._temp_dir.path().join(format!("ws-{}", id)),
            agent_dir: self._temp_dir.path().join(format!("agent-{}", id)),
            allowed_links,
            ..AgentInstanceConfig::default()
        };
        let instance = AgentInstance::new(config, Arc::clone(&self.session_store))
            .expect("Failed to create AgentInstance");
        self.agent_registry.register(instance).await;
    }

    /// Update an agent's allowed_links by removing and re-registering.
    pub async fn update_allowed_links(&self, agent_id: &str, links: Option<Vec<String>>) {
        // Remove existing
        self.agent_registry.remove(agent_id).await;
        // Re-register with new allowed_links
        self.register_agent(agent_id, links).await;
    }

    /// Send a DM-style inbound message through the router.
    pub async fn send_message(&self, link_id: &str, text: &str) {
        let msg = self.make_message(link_id, text, false);
        let _ = self.router.handle_message(msg).await;
    }

    /// Send a group-style inbound message through the router.
    pub async fn send_group_message(&self, link_id: &str, text: &str) {
        let msg = self.make_message(link_id, text, true);
        let _ = self.router.handle_message(msg).await;
    }

    /// Build an InboundMessage.
    fn make_message(&self, link_id: &str, text: &str, is_group: bool) -> InboundMessage {
        let n = self.msg_counter.fetch_add(1, Ordering::SeqCst);
        InboundMessage {
            id: MessageId::new(format!("msg-{}", n)),
            channel_id: ChannelId::new(link_id),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group,
            raw: None,
            metadata: vec![],
        }
    }

    /// Drain all captured replies from the channel.
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

    /// Assert that no reply contains denial indicators.
    pub fn assert_no_denial(&mut self) {
        let replies = self.drain_replies();
        for r in &replies {
            assert!(
                !r.text.contains('\u{26D4}') && !r.text.to_lowercase().contains("denied"),
                "Unexpected denial reply: {}",
                r.text
            );
        }
    }

    /// Assert that at least one reply is a denial.
    pub fn assert_denied(&mut self) {
        let replies = self.drain_replies();
        assert!(
            replies.iter().any(|r| {
                r.text.contains('\u{26D4}')
                    || r.text.to_lowercase().contains("denied")
                    || r.text.to_lowercase().contains("not allowed")
            }),
            "Expected a denial reply, got: {:?}",
            replies.iter().map(|r| &r.text).collect::<Vec<_>>()
        );
    }
}
