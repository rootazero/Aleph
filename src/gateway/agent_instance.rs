//! Agent Instance
//!
//! Provides isolated execution environments for agents. Each agent instance
//! has its own workspace directory, session store, and configuration.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::router::SessionKey;
use super::session_store::SessionStore;

#[cfg(test)]
use super::session_manager::SessionState;

/// Configuration for an agent instance
#[derive(Debug, Clone)]
pub struct AgentInstanceConfig {
    /// Unique agent identifier
    pub agent_id: String,
    /// Human-readable display name (e.g., "Trading Assistant", "Coding Agent")
    pub display_name: Option<String>,
    /// Workspace directory path
    pub workspace: PathBuf,
    /// Primary model to use
    pub model: String,
    /// Maximum agent loop iterations
    pub max_loops: u32,
    /// Maximum total token usage per request (loop guard, None = use default)
    pub max_tokens: Option<usize>,
    /// Tool whitelist (empty = all allowed)
    pub tool_whitelist: Vec<String>,
    /// Tool blacklist
    pub tool_blacklist: Vec<String>,
    /// Agent state directory (sessions, runtime state)
    pub agent_dir: PathBuf,
    /// Link access whitelist (None or empty = all links allowed)
    pub allowed_links: Option<Vec<String>>,
    /// Users who may start a run as this agent (None or empty = everyone).
    ///
    /// The registry is the authority for *which* agent actually runs
    /// (`AgentRegistry::get`), so the run-start gate has to read the list off
    /// the same object — reading it back out of `Config.agents.list` would
    /// leave "registered but not in that list" as a bypass.
    pub allowed_users: Option<Vec<String>>,
    /// Per-agent tool permission overrides
    pub tool_permissions: Option<crate::config::types::policies::ToolPermissionsConfig>,
    /// Optional per-agent timeout override (seconds). None = use global default.
    pub timeout_secs: Option<u64>,
}

impl Default for AgentInstanceConfig {
    fn default() -> Self {
        Self {
            agent_id: "main".to_string(),
            display_name: None,
            workspace: crate::config::agent_resolver::default_workspace_root().join("main"),
            model: "claude-sonnet-4-5".to_string(),
            max_loops: 100,
            max_tokens: None,
            tool_whitelist: vec![],
            tool_blacklist: vec![],
            // Both roots come from `agent_resolver` — the same functions the
            // production path (`from_resolved`) resolves through. Spelling
            // them by hand here made `Default` describe a layout no live agent
            // ever used once `ALEPH_HOME` was set.
            agent_dir: crate::config::agent_resolver::default_agents_root().join("main"),
            allowed_links: None,
            allowed_users: None,
            tool_permissions: None,
            timeout_secs: None,
        }
    }
}

impl AgentInstanceConfig {
    /// Return the agent's tool permissions config, or a default (all-Allow).
    #[must_use]
    pub fn tool_permissions(&self) -> crate::config::types::policies::ToolPermissionsConfig {
        self.tool_permissions.clone().unwrap_or_default()
    }

    /// Return the agent's timeout override, if set.
    #[must_use]
    pub const fn timeout_secs(&self) -> Option<u64> {
        self.timeout_secs
    }

    /// Create from a resolved agent definition.
    ///
    /// Maps `ResolvedAgent` fields to `AgentInstanceConfig`:
    /// - `tool_whitelist` <- skills
    /// - workspace <- `workspace_path`
    ///
    /// Note: a prior shape carried an eagerly-read `system_prompt` field
    /// from `ResolvedAgent.soul_md / agents_md`. That copy had zero
    /// production readers (the field was only consumed by tests); real
    /// system-prompt injection happens through `IdentityFiles::load` in
    /// `harness_bridge::prompt_build`, which reads the files fresh every
    /// turn and feeds them to `SoulLayer` / `ProfileLayer` /
    /// `IdentityFilesLayer`. The field is gone for good — anything that
    /// wants to add a system-prompt override should add a layer, not a
    /// boot-time string.
    #[must_use]
    pub fn from_resolved(agent: &crate::config::agent_resolver::ResolvedAgent) -> Self {
        Self {
            agent_id: agent.id.clone(),
            display_name: Some(agent.name.clone()),
            workspace: agent.workspace_path.clone(),
            model: agent.model.clone(),
            max_loops: 100,
            max_tokens: None,
            tool_whitelist: agent.skills.clone(),
            tool_blacklist: agent.skills_blacklist.clone(),
            agent_dir: agent.agent_dir.clone(),
            allowed_links: agent.allowed_links.clone(),
            allowed_users: agent.allowed_users.clone(),
            tool_permissions: agent.tool_permissions.clone(),
            timeout_secs: None,
        }
    }
}

/// Agent instance state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    /// Agent is idle, ready to accept requests
    Idle,
    /// Agent is processing a request
    Running { run_id: String },
    /// Agent is paused (waiting for user input)
    Paused { run_id: String, reason: String },
    /// Agent encountered an error
    Error { message: String },
    /// Agent is shutting down
    Stopping,
}

/// An isolated agent instance
///
/// Each instance has:
/// - Dedicated workspace directory
/// - Separate session store (`SQLite` via `SessionManager`)
/// - Independent configuration
/// - Isolated state
pub struct AgentInstance {
    /// Agent configuration
    config: AgentInstanceConfig,
    /// Current agent state
    state: Arc<RwLock<AgentState>>,
    /// Agent directory (contains workspace, config)
    agent_dir: PathBuf,
    /// Session store for persistence
    session_store: Arc<dyn SessionStore>,
    /// Optional L0 raw-memory writer. When set, every persisted user/assistant
    /// message is also captured into the `raw_memories` table as a `Transcript`
    /// entry so the compression pipeline (`CompressionService`) can later promote
    /// it to L1 notes. None falls back to compaction-only capture, which means
    /// short conversations never reach L0.
    raw_memory_writer: Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
}

/// A message in a session
#[derive(Debug, Clone)]
pub struct SessionMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

/// Build the persisted-message `metadata` JSON from the optional `run_id` and
/// the optional last-turn context occupancy. Returns `None` when neither is
/// present (so legacy rows keep persisting NULL metadata). The occupancy keys
/// mirror the live `run_complete` summary fields so the Panel reads the gauge
/// identically on reload and on live completion.
#[must_use]
pub(crate) fn build_message_metadata(
    run_id: Option<&str>,
    occupancy: Option<crate::gateway::execution_engine::helpers::RunContextOccupancy>,
) -> Option<serde_json::Value> {
    if run_id.is_none() && occupancy.is_none() {
        return None;
    }
    let mut map = serde_json::Map::new();
    if let Some(r) = run_id {
        map.insert(
            "run_id".to_string(),
            serde_json::Value::String(r.to_string()),
        );
    }
    if let Some(o) = occupancy {
        // Stored as strings, not JSON numbers: `SessionMessage::from_record`
        // deserializes the whole metadata blob via `HashMap<String, String>`,
        // which would reject numeric values and silently drop run_id too.
        map.insert(
            "context_tokens".to_string(),
            serde_json::Value::String(o.context_tokens.to_string()),
        );
        map.insert(
            "context_window".to_string(),
            serde_json::Value::String(o.context_window.to_string()),
        );
        map.insert(
            "total_tokens".to_string(),
            serde_json::Value::String(o.total_tokens.to_string()),
        );
    }
    Some(serde_json::Value::Object(map))
}

impl AgentInstance {
    /// Create a new agent instance with a session store
    pub fn new(
        config: AgentInstanceConfig,
        session_store: Arc<dyn SessionStore>,
    ) -> Result<Self, AgentInstanceError> {
        let agent_dir = config.agent_dir.clone();

        // Create directories
        std::fs::create_dir_all(&agent_dir).map_err(|e| {
            AgentInstanceError::InitFailed(format!("Failed to create agent dir: {e}"))
        })?;

        std::fs::create_dir_all(&config.workspace).map_err(|e| {
            AgentInstanceError::InitFailed(format!("Failed to create workspace: {e}"))
        })?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            let _ = std::fs::set_permissions(&agent_dir, perms);
        }

        info!(
            "Created agent instance '{}' at {:?}",
            config.agent_id, agent_dir
        );

        Ok(Self {
            config,
            state: Arc::new(RwLock::new(AgentState::Idle)),
            agent_dir,
            session_store,
            raw_memory_writer: None,
        })
    }

    /// Attach an L0 raw-memory writer so every persisted message is also
    /// captured to `raw_memories`. Wired at gateway startup.
    pub fn with_raw_memory_writer(
        mut self,
        writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    ) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }

    /// A copy of this instance whose admission list is `allowed_users`,
    /// sharing every runtime handle with the original.
    ///
    /// `state`, `session_store` and `raw_memory_writer` are cloned handles, so
    /// a run already in flight against the old `Arc` keeps observing — and
    /// updating — the same live state. That is the intended semantics: this
    /// swaps *who may start a run*, not what is already running.
    ///
    /// Deliberately not routed through [`Self::new`], which `mkdir`s the agent
    /// and workspace directories and logs an instance creation. An
    /// admission-list edit is neither.
    #[must_use]
    pub fn with_allowed_users(&self, allowed_users: Option<Vec<String>>) -> Self {
        let mut config = self.config.clone();
        config.allowed_users = allowed_users;
        Self {
            config,
            state: Arc::clone(&self.state),
            agent_dir: self.agent_dir.clone(),
            session_store: Arc::clone(&self.session_store),
            raw_memory_writer: self.raw_memory_writer.clone(),
        }
    }

    /// Get the agent ID
    #[must_use]
    pub fn id(&self) -> &str {
        &self.config.agent_id
    }

    /// Get the human-readable display name (falls back to `agent_id`)
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.config
            .display_name
            .as_deref()
            .unwrap_or(&self.config.agent_id)
    }

    /// Get the agent configuration
    #[must_use]
    pub const fn config(&self) -> &AgentInstanceConfig {
        &self.config
    }

    /// Get the workspace directory
    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.config.workspace
    }

    /// Get the agent directory
    #[must_use]
    pub fn agent_dir(&self) -> &Path {
        &self.agent_dir
    }

    /// Shared handle to this instance's session store.
    ///
    /// Exposes the raw `MessageRecord` store so the gateway run loop can
    /// backfill `session_events` from legacy `messages` rows before dispatch.
    #[must_use]
    pub(crate) fn session_store(&self) -> Arc<dyn SessionStore> {
        self.session_store.clone()
    }

    /// Get the current agent state
    pub async fn state(&self) -> AgentState {
        self.state.read().await.clone()
    }

    /// Check if the agent is idle
    pub async fn is_idle(&self) -> bool {
        matches!(*self.state.read().await, AgentState::Idle)
    }

    /// Set the agent state
    pub async fn set_state(&self, new_state: AgentState) {
        let mut state = self.state.write().await;
        debug!(
            "Agent '{}' state change: {:?} -> {:?}",
            self.config.agent_id, *state, new_state
        );
        *state = new_state;
    }

    /// Transition the session's persisted lifecycle state to Idle.
    ///
    /// The execution engine calls this when a run finishes (success, error, or
    /// cancellation) so the session metadata does not stay stuck in `Running`.
    pub async fn set_session_idle(&self, key: &SessionKey) {
        if let Err(e) = self.session_store.set_idle(key).await {
            warn!(
                "Failed to set session idle '{}': {}",
                key.to_key_string(),
                e
            );
        }
    }

    /// Read the session's persisted lifecycle state, if available.
    #[cfg(test)]
    pub(crate) async fn session_state(&self, key: &SessionKey) -> Option<SessionState> {
        match self.session_store.get_state(key).await {
            Ok(state) => Some(state),
            Err(e) => {
                warn!(
                    "Failed to get session state '{}': {}",
                    key.to_key_string(),
                    e
                );
                None
            }
        }
    }

    /// Atomically transition from Idle to Running.
    ///
    /// Closes the TOCTOU window between `is_idle()` and `set_state(Running)`:
    /// two concurrent executions can no longer both observe Idle before either
    /// flips the state. Returns true on success, false if the agent is not idle.
    ///
    /// Retired as the production `ExecutionEngine`'s admission gate (Task 6):
    /// that path now claims per-*session* via
    /// `execution_engine::session_run_registry::SessionRunRegistry`, so two
    /// sessions of the same agent can run in parallel — a guarantee this
    /// per-*agent* flag cannot express. Kept only for
    /// `SimpleExecutionEngine`'s own (unchanged) fallback gate.
    pub async fn try_start_run(&self, run_id: &str) -> bool {
        let mut state = self.state.write().await;
        if matches!(*state, AgentState::Idle) {
            let new_state = AgentState::Running {
                run_id: run_id.to_string(),
            };
            debug!(
                "Agent '{}' state change: {:?} -> {:?}",
                self.config.agent_id, *state, new_state
            );
            *state = new_state;
            true
        } else {
            false
        }
    }

    /// Get or create a session (delegated to session store)
    pub async fn get_or_create_session(&self, key: &SessionKey) -> SessionInfo {
        match self.session_store.get_or_create(key).await {
            Ok(meta) => SessionInfo::from_metadata(&meta),
            Err(e) => {
                warn!("Failed to get_or_create session: {}", e);
                // Return a minimal fallback so callers don't break
                let now = chrono::Utc::now();
                SessionInfo {
                    key: key.to_key_string(),
                    agent_id: self.config.agent_id.clone(),
                    message_count: 0,
                    created_at: now,
                    last_active_at: now,
                }
            }
        }
    }

    /// Ensure a session exists.
    pub async fn ensure_session(&self, key: &SessionKey) {
        if let Err(e) = self.session_store.get_or_create(key).await {
            warn!("Failed to ensure session in store: {}", e);
        }
    }

    /// Record the session's originating channel (and, for inbound channels, its
    /// conversation id) onto identity metadata so `sessions.list` /
    /// `sessions.changed` can surface conversation origin and a later
    /// cross-surface continuation can fan the reply back (sub-gap (b)).
    /// Idempotent in the store (never clobbers a real origin); best-effort (a
    /// stamp failure must not abort the run).
    pub async fn set_session_source_channel(
        &self,
        key: &SessionKey,
        channel: &str,
        conversation: Option<&str>,
    ) {
        if let Err(e) = self
            .session_store
            .set_source_channel(key, channel, conversation)
            .await
        {
            warn!("Failed to stamp session source_channel in store: {}", e);
        }
    }

    /// Resolve a session's bound origin `(channel, conversation)` for
    /// cross-surface reply fan-out. `Some` only when the session was first
    /// created by an *external* channel (not the Panel's own `"gui:chat"` nor
    /// the unknown sentinel) and an origin conversation id was captured.
    pub async fn origin_route(&self, key: &SessionKey) -> Option<(String, String)> {
        origin_route_from_store(&self.session_store, key).await
    }

    /// Add a message to a session (delegated to session store) and capture
    /// it into the L0 `raw_memories` buffer when a writer is wired.
    pub async fn add_message(&self, key: &SessionKey, role: MessageRole, content: &str) {
        self.add_message_with_run_id(key, role, content, None, None)
            .await;
    }

    /// Like [`add_message`], but stamps `metadata.run_id` on the persisted
    /// row. This is the link that lets `chat.history` surface a `run_id` so the
    /// Panel can fetch the run's observability trace (`task_traces`) and
    /// rehydrate the workspace step view on session reload/switch. Without it,
    /// assistant rows persist with NULL metadata and the workspace pane goes
    /// blank whenever the live event stream is gone.
    pub async fn add_message_with_run_id(
        &self,
        key: &SessionKey,
        role: MessageRole,
        content: &str,
        run_id: Option<&str>,
        occupancy: Option<crate::gateway::execution_engine::helpers::RunContextOccupancy>,
    ) {
        let key_str = key.to_key_string();
        let role_str = match role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
            MessageRole::Tool => "tool",
        };

        let metadata = build_message_metadata(run_id, occupancy);

        // Ensure session exists, then add message
        if let Err(e) = self.session_store.get_or_create(key).await {
            warn!("Failed to ensure session in store: {}", e);
        }
        if let Err(e) = self
            .session_store
            .append_message(
                key,
                crate::gateway::session_store::types::MessageRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    role: role_str.to_string(),
                    content: content.to_string(),
                    timestamp: chrono::Utc::now().timestamp(),
                    metadata,
                    input_tokens: 0,
                    output_tokens: 0,
                    tool_call_id: None,
                    tool_name: None,
                },
            )
            .await
        {
            warn!("Failed to persist message to store '{}': {}", key_str, e);
        }

        // L0 capture: only persist user and assistant turns. System messages
        // are prompt scaffolding (re-built each turn) and tool messages are
        // captured separately via RawMemorySource::ToolOutput in the agent
        // loop. Skipping them keeps L0 focused on conversational signal.
        if matches!(role, MessageRole::User | MessageRole::Assistant) {
            if let Some(writer) = self.raw_memory_writer.as_ref() {
                let body = format!("[{role_str}] {content}");
                let raw = crate::memory::store::raw_memory::RawMemory::new(
                    body,
                    crate::memory::store::raw_memory::RawMemorySource::Transcript,
                )
                .with_agent(self.config.agent_id.clone())
                .with_session(key_str.clone());
                if let Err(e) = writer.insert_raw_memory(&raw).await {
                    warn!("L0 raw_memory write failed for {}: {}", key_str, e);
                }
            }
        }
    }

    /// Get session history (delegated to session store)
    pub async fn get_history(&self, key: &SessionKey, limit: Option<usize>) -> Vec<SessionMessage> {
        match self.session_store.get_history(key, limit).await {
            Ok(stored) => stored
                .into_iter()
                .map(SessionMessage::from_record)
                .collect(),
            Err(e) => {
                warn!("Failed to get history from store: {}", e);
                Vec::new()
            }
        }
    }

    /// Reset (clear) a session (delegated to session store).
    ///
    /// Retires the live event log first, then the `/btw` side session, then the
    /// projection — the same order, for the same two reasons, as `chat.clear`
    /// and the `sessions.reset` RPC. The store's `reset_session` only empties
    /// the `messages` table the Panel reads; the model replays `session_events`,
    /// so clearing the projection alone blanks the screen while the model still
    /// remembers every word. And the side session holds a copied prefix of this
    /// transcript in its own event log, so a reset that spares it leaves the
    /// cleared content readable through the next `/btw`. Side session only —
    /// the key is unchanged, so any loop/goal keyed to it is still reachable
    /// and must survive a content wipe.
    ///
    /// No production caller today; the wire is here so the first one inherits
    /// the parity rather than the defect the other two surfaces document.
    pub async fn reset_session(&self, key: &SessionKey) -> bool {
        // Parity requirement with both live clear surfaces: SSOT before
        // projection, so a failure here leaves recoverable ghost rows on screen
        // rather than a conversation the model secretly still holds.
        if let Err(e) = crate::session::store::retire_live_events(key, 1).await {
            warn!("Failed to retire session event log on reset: {}", e);
            return false;
        }
        crate::gateway::continuation_lifecycle::retire_side_session(
            key,
            "agent_instance.reset",
            Some(self.session_store.clone()),
        );
        match self.session_store.reset_session(key).await {
            Ok(deleted) => {
                debug!("Reset session: {}", key.to_key_string());
                deleted
            }
            Err(e) => {
                warn!("Failed to reset session: {}", e);
                false
            }
        }
    }

    /// List all sessions for this agent (delegated to session store)
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        match self
            .session_store
            .list_sessions(crate::gateway::session_store::types::SessionFilter {
                agent_id: Some(self.config.agent_id.clone()),
                ..Default::default()
            })
            .await
        {
            Ok(metas) => metas.iter().map(SessionInfo::from_metadata).collect(),
            Err(e) => {
                warn!("Failed to list sessions: {}", e);
                Vec::new()
            }
        }
    }

    /// Check if a tool is allowed for this agent
    #[must_use]
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        tool_allowed_by(
            tool_name,
            &self.config.tool_whitelist,
            &self.config.tool_blacklist,
        )
    }
}

/// Resolve a session's bound origin `(channel, conversation)` directly from a
/// session-store handle, with no live [`AgentInstance`] required.
///
/// Extracted from [`AgentInstance::origin_route`] (which just delegates here
/// with `&self.session_store`) so a caller holding only a store handle and a
/// [`SessionKey`] — e.g. the continuation dispatcher's agent-miss race, where
/// the agent that would normally answer this has already been deleted — can
/// resolve the same origin instead of duplicating (and inevitably drifting
/// from) this lookup.
pub(crate) async fn origin_route_from_store(
    session_store: &Arc<dyn SessionStore>,
    key: &SessionKey,
) -> Option<(String, String)> {
    let meta = session_store.get_metadata(key).await.ok().flatten()?;
    let channel = meta.origin_channel()?;
    if channel == "gui:chat" {
        return None;
    }
    let conversation = meta.origin_conversation()?;
    Some((channel, conversation))
}

/// Glob-aware allow/deny match over a `(whitelist, blacklist)` pair.
///
/// Extracted from [`AgentInstance::is_tool_allowed`] so the same semantics can
/// be applied *before* an instance exists — team member provisioning validates
/// a declared toolset against the tools its launch prompt contracts it to call
/// (`teams::member_provision`). A second copy of this matcher there would drift
/// from the one that actually gates the run.
///
/// Rules: the blacklist wins; an empty whitelist (or one containing `"*"`)
/// allows everything else; both lists support a trailing-`*` prefix glob.
#[must_use]
pub fn tool_allowed_by(tool_name: &str, whitelist: &[String], blacklist: &[String]) -> bool {
    let matches = |pattern: &String| {
        pattern
            .strip_suffix('*')
            .map_or(pattern == tool_name, |prefix| tool_name.starts_with(prefix))
    };

    // Check blacklist first (supports glob prefix like "bash_*", same as the
    // whitelist below — a bare Vec::contains would never match a glob entry).
    if blacklist.iter().any(&matches) {
        return false;
    }

    // If whitelist is empty or contains "*", allow all (except blacklisted)
    if whitelist.is_empty() || whitelist.iter().any(|p| p == "*") {
        return true;
    }

    // Check whitelist (supports glob prefix like "fs_*")
    whitelist.iter().any(&matches)
}

/// Session information (public view)
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub key: String,
    pub agent_id: String,
    pub message_count: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_active_at: chrono::DateTime<chrono::Utc>,
}

impl SessionInfo {
    /// Construct from `SessionMetadata`
    fn from_metadata(meta: &super::session_store::types::SessionMetadata) -> Self {
        Self {
            key: meta.key.clone(),
            agent_id: meta.agent_id.clone(),
            message_count: meta.message_count as usize,
            created_at: chrono::DateTime::from_timestamp(meta.created_at, 0)
                .unwrap_or_else(chrono::Utc::now),
            last_active_at: chrono::DateTime::from_timestamp(meta.last_active_at, 0)
                .unwrap_or_else(chrono::Utc::now),
        }
    }
}

impl SessionMessage {
    /// Convert from `MessageRecord` to `SessionMessage`
    fn from_record(record: super::session_store::types::MessageRecord) -> Self {
        let role = match record.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            "tool" => MessageRole::Tool,
            _ => MessageRole::User,
        };
        let timestamp = record.instant().unwrap_or_else(chrono::Utc::now);
        let metadata = record
            .metadata
            .and_then(|v| serde_json::from_value::<HashMap<String, String>>(v).ok());
        Self {
            role,
            content: record.content,
            timestamp,
            metadata,
        }
    }
}

/// Agent instance errors
#[derive(Debug, thiserror::Error)]
pub enum AgentInstanceError {
    #[error("Initialization failed: {0}")]
    InitFailed(String),

    #[error("Agent not found: {0}")]
    NotFound(String),

    #[error("Agent busy: {0}")]
    Busy(String),

    #[error("Session error: {0}")]
    SessionError(String),
}

/// Lazy-loaded agent entry: config-only until first access.
#[allow(clippy::large_enum_variant)]
enum AgentEntry {
    /// Registered but not yet instantiated
    Config {
        config: AgentInstanceConfig,
        session_store: Arc<dyn SessionStore>,
    },
    /// Fully instantiated
    Instance(Arc<AgentInstance>),
}

/// What [`AgentRegistry::remove`] actually evicted.
///
/// Both variants mean "the agent is gone from the registry"; they differ in
/// what teardown material the caller gets back.
pub enum RemovedAgent {
    /// The agent was instantiated; the live instance is returned for teardown.
    Instance(Arc<AgentInstance>),
    /// The agent was still a lazy config entry; its config is returned so the
    /// caller can archive the workspace it would have used.
    ///
    /// Boxed: `AgentInstanceConfig` is ~352 bytes vs the `Arc` in `Instance`,
    /// so an unboxed payload would bloat every `RemovedAgent` (clippy
    /// `large_enum_variant`).
    Lazy(Box<AgentInstanceConfig>),
}

impl RemovedAgent {
    /// Workspace path of the removed agent, regardless of instantiation state.
    #[must_use]
    pub fn workspace(&self) -> &std::path::Path {
        match self {
            Self::Instance(inst) => inst.workspace(),
            Self::Lazy(config) => &config.workspace,
        }
    }
}

/// Registry of agent instances
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, AgentEntry>>>,
    default_agent: String,
    /// Optional L0 writer applied to every lazily-instantiated agent so
    /// gateway-mediated turns reach `raw_memories`. Set once at startup.
    raw_memory_writer:
        Arc<RwLock<Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>>>,
}

impl AgentRegistry {
    /// Create a new registry with default "main" agent
    #[must_use]
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            default_agent: "main".to_string(),
            raw_memory_writer: Arc::new(RwLock::new(None)),
        }
    }

    /// Wire an L0 raw-memory writer that will be applied to every agent
    /// instantiated through this registry. Idempotent; the latest writer wins.
    pub async fn set_raw_memory_writer(
        &self,
        writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    ) {
        let mut slot = self.raw_memory_writer.write().await;
        *slot = Some(writer);
    }

    /// Register an already-instantiated agent (for tests and dynamic creation).
    /// If a raw-memory writer has been set on the registry and the instance
    /// does not already carry one, attach it here so dynamically-created agents
    /// also fill `raw_memories`.
    pub async fn register(&self, instance: AgentInstance) {
        let id = instance.id().to_string();
        let instance = if instance.raw_memory_writer.is_none() {
            if let Some(writer) = self.raw_memory_writer.read().await.clone() {
                instance.with_raw_memory_writer(writer)
            } else {
                instance
            }
        } else {
            instance
        };
        let mut agents = self.agents.write().await;
        agents.insert(id.clone(), AgentEntry::Instance(Arc::new(instance)));
        info!("Registered agent: {}", id);
    }

    /// Register an agent config for lazy instantiation on first access.
    pub async fn register_config(
        &self,
        config: AgentInstanceConfig,
        session_store: Arc<dyn SessionStore>,
    ) {
        let id = config.agent_id.clone();
        let mut agents = self.agents.write().await;
        agents.insert(
            id.clone(),
            AgentEntry::Config {
                config,
                session_store,
            },
        );
        info!("Registered agent config (lazy): {}", id);
    }

    /// Get an agent by ID, instantiating lazily if needed.
    pub async fn get(&self, agent_id: &str) -> Option<Arc<AgentInstance>> {
        // Fast path: read lock, return if already instantiated
        {
            let agents = self.agents.read().await;
            match agents.get(agent_id) {
                Some(AgentEntry::Instance(inst)) => return Some(Arc::clone(inst)),
                Some(AgentEntry::Config { .. }) => { /* need to instantiate */ }
                None => return None,
            }
        }

        // Slow path: write lock, instantiate from config
        let mut agents = self.agents.write().await;

        // Re-check after acquiring write lock (TOCTOU race: another task may
        // have instantiated between dropping read lock and acquiring write lock)
        match agents.get(agent_id) {
            Some(AgentEntry::Instance(inst)) => return Some(Arc::clone(inst)),
            None => return None,
            Some(AgentEntry::Config { .. }) => {}
        }

        // Take the Config entry out to consume it
        let entry = agents.remove(agent_id)?;
        let (config, session_store) = match entry {
            AgentEntry::Config {
                config,
                session_store,
            } => (config, session_store),
            AgentEntry::Instance(_) => unreachable!("checked above"),
        };

        let id = config.agent_id.clone();
        match AgentInstance::new(config, session_store) {
            Ok(mut instance) => {
                if let Some(writer) = self.raw_memory_writer.read().await.clone() {
                    instance = instance.with_raw_memory_writer(writer);
                }
                let arc = Arc::new(instance);
                agents.insert(id, AgentEntry::Instance(Arc::clone(&arc)));
                Some(arc)
            }
            Err(e) => {
                warn!("Failed to lazily instantiate agent '{}': {}", id, e);
                None
            }
        }
    }

    /// Get the default agent
    pub async fn get_default(&self) -> Option<Arc<AgentInstance>> {
        self.get(&self.default_agent).await
    }

    /// List all registered agent IDs (both lazy and instantiated)
    pub async fn list(&self) -> Vec<String> {
        let agents = self.agents.read().await;
        agents.keys().cloned().collect()
    }

    /// Whether an agent ID is registered (lazy or instantiated).
    ///
    /// Unlike [`Self::get`], this never instantiates a lazy entry — use it for
    /// existence validation (e.g. binding a channel to an agent) where forcing
    /// a full `AgentInstance` build would be wasted work.
    pub async fn contains(&self, agent_id: &str) -> bool {
        self.agents.read().await.contains_key(agent_id)
    }

    /// Get the `allowed_links` for an agent (None = all allowed).
    /// Extracts from either variant without instantiating.
    pub async fn get_allowed_links(&self, agent_id: &str) -> Option<Option<Vec<String>>> {
        let agents = self.agents.read().await;
        agents.get(agent_id).map(|entry| match entry {
            AgentEntry::Instance(inst) => inst.config().allowed_links.clone(),
            AgentEntry::Config { config, .. } => config.allowed_links.clone(),
        })
    }

    /// Get the `allowed_users` for an agent (None = everyone). Outer `None`
    /// means the agent is not registered at all.
    ///
    /// Twin of [`Self::get_allowed_links`] and non-instantiating for the same
    /// reason: the delegation face asks this question before it decides to
    /// build the target agent, and building one just to refuse it is wasted
    /// work.
    pub async fn get_allowed_users(&self, agent_id: &str) -> Option<Option<Vec<String>>> {
        let agents = self.agents.read().await;
        agents.get(agent_id).map(|entry| match entry {
            AgentEntry::Instance(inst) => inst.config().allowed_users.clone(),
            AgentEntry::Config { config, .. } => config.allowed_users.clone(),
        })
    }

    /// Install a new `allowed_users` admission list on a registered agent
    /// without a restart. Returns `false` when `agent_id` is not registered —
    /// nothing was applied and no caller may report that it was.
    ///
    /// # Why this lives on the registry, and why it is the only one
    ///
    /// The run-start gate reads the list off whatever
    /// [`AgentRegistry::get`] hands back (`build_run_request` →
    /// [`caller_may_act_as_agent`](crate::gateway::caller_identity::caller_may_act_as_agent)),
    /// so the registry is where a revocation has to land to bite. `agent_update`
    /// (the tool) and `agents.update` (the RPC the Panel calls) are two faces of
    /// one verb; both call this. A face that wrote only TOML would report a
    /// REVOCATION as done while the refused user kept running until the next
    /// boot — the single failure the whole admission axis exists to prevent, and
    /// the one that is invisible to every test of the gate itself, because the
    /// gate would still be enforced, faithfully, against a stale list.
    ///
    /// # Staleness boundary
    ///
    /// An `Arc<AgentInstance>` handed out **before** this call keeps the old
    /// list. That is bounded and intended: those handles belong to runs whose
    /// gate has already been passed, and every run-start path re-reads the
    /// registry (`registry.get(..)` per turn) rather than caching an instance.
    pub async fn set_allowed_users(
        &self,
        agent_id: &str,
        allowed_users: Option<Vec<String>>,
    ) -> bool {
        let mut agents = self.agents.write().await;
        match agents.get_mut(agent_id) {
            // Lazy entry: the config in the map IS the one `get` will
            // instantiate from, so writing it here is the whole job.
            Some(AgentEntry::Config { config, .. }) => {
                config.allowed_users = allowed_users;
                true
            }
            // Instantiated: `AgentInstanceConfig` is not mutable behind the
            // `Arc`, so swap in a sibling instance that shares the live state.
            Some(AgentEntry::Instance(inst)) => {
                *inst = Arc::new(inst.with_allowed_users(allowed_users));
                true
            }
            None => false,
        }
    }

    /// Remove an agent (works for both lazy and instantiated entries).
    ///
    /// Returns what was actually evicted. The previous signature
    /// (`Option<Arc<AgentInstance>>`) collapsed the lazy case into `None`,
    /// which callers read as "nothing was removed" — deleting a
    /// never-instantiated agent then misreported failure and skipped both
    /// workspace archival and the `Deleted` lifecycle event even though the
    /// registry entry was gone.
    pub async fn remove(&self, agent_id: &str) -> Option<RemovedAgent> {
        let mut agents = self.agents.write().await;
        match agents.remove(agent_id) {
            Some(AgentEntry::Instance(inst)) => Some(RemovedAgent::Instance(inst)),
            Some(AgentEntry::Config { config, .. }) => Some(RemovedAgent::Lazy(Box::new(config))),
            None => None,
        }
    }

    /// Set the default agent
    pub fn set_default(&mut self, agent_id: impl Into<String>) {
        self.default_agent = agent_id.into();
    }

    /// Get default agent ID
    #[must_use]
    pub fn default_agent_id(&self) -> &str {
        &self.default_agent
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_store::sqlite_backend::{
        SqliteSessionStore, SqliteSessionStoreConfig,
    };
    use tempfile::tempdir;

    /// Create a test session store backed by a temporary SQLite database.
    fn test_session_store(temp: &tempfile::TempDir) -> Arc<dyn SessionStore> {
        let config = SqliteSessionStoreConfig {
            db_path: temp.path().join("test_sessions.db"),
            ..Default::default()
        };
        Arc::new(SqliteSessionStore::new(config).expect("test session store"))
    }

    #[tokio::test]
    async fn test_agent_instance_creation() {
        let temp = tempdir().unwrap();
        let sm = test_session_store(&temp);
        let config = AgentInstanceConfig {
            agent_id: "test-agent".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/test-agent"),
            ..Default::default()
        };

        let instance = AgentInstance::new(config, sm).unwrap();
        assert_eq!(instance.id(), "test-agent");
        assert!(instance.is_idle().await);
    }

    #[tokio::test]
    async fn test_session_management() {
        let temp = tempdir().unwrap();
        let sm = test_session_store(&temp);
        let config = AgentInstanceConfig {
            agent_id: "test".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/test"),
            ..Default::default()
        };

        let instance = AgentInstance::new(config, sm).unwrap();
        let key = SessionKey::main("test");

        // Create session
        let info = instance.get_or_create_session(&key).await;
        assert_eq!(info.message_count, 0);

        // Add messages
        instance.add_message(&key, MessageRole::User, "Hello").await;
        instance
            .add_message(&key, MessageRole::Assistant, "Hi!")
            .await;

        let history = instance.get_history(&key, None).await;
        assert_eq!(history.len(), 2);

        // Reset
        assert!(instance.reset_session(&key).await);
        let history = instance.get_history(&key, None).await;
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn assistant_message_persists_run_id_in_metadata() {
        let temp = tempdir().unwrap();
        let sm = test_session_store(&temp);
        let config = AgentInstanceConfig {
            agent_id: "test".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/test"),
            ..Default::default()
        };
        let instance = AgentInstance::new(config, sm).unwrap();
        let key = SessionKey::main("test");

        // The agent loop stamps the run_id so chat.history can later map this
        // assistant turn back to its persisted observability trace, letting
        // the workspace panel rehydrate on session reload/switch.
        instance
            .add_message_with_run_id(
                &key,
                MessageRole::Assistant,
                "Hi!",
                Some("run-xyz"),
                Some(
                    crate::gateway::execution_engine::helpers::RunContextOccupancy {
                        context_tokens: 42_000,
                        context_window: 200_000,
                        total_tokens: 55_000,
                        input_tokens: 40_000,
                        output_tokens: 15_000,
                        cost_usd: Some(0.42),
                        model: None,
                        model_provider: None,
                    },
                ),
            )
            .await;

        let history = instance.get_history(&key, None).await;
        let last = history.last().expect("one persisted message");
        assert_eq!(last.role, MessageRole::Assistant);
        let meta = last
            .metadata
            .as_ref()
            .expect("assistant turn must carry metadata");
        assert_eq!(
            meta.get("run_id").map(String::as_str),
            Some("run-xyz"),
            "assistant turn must carry run_id in metadata for trace replay"
        );
        // Occupancy persisted as strings so the HashMap<String,String> decode in
        // `from_record` keeps the whole blob (run_id included) intact.
        assert_eq!(
            meta.get("context_tokens").map(String::as_str),
            Some("42000")
        );
        assert_eq!(
            meta.get("context_window").map(String::as_str),
            Some("200000")
        );
        assert_eq!(meta.get("total_tokens").map(String::as_str), Some("55000"));
    }

    #[test]
    fn build_message_metadata_combines_run_id_and_occupancy() {
        use crate::gateway::execution_engine::helpers::RunContextOccupancy;
        // None + None → no metadata row at all (legacy NULL behavior).
        assert!(build_message_metadata(None, None).is_none());

        // run_id only → just the run_id key (no occupancy noise).
        let only_run = build_message_metadata(Some("r1"), None).expect("some");
        assert_eq!(only_run.get("run_id").and_then(|v| v.as_str()), Some("r1"));
        assert!(only_run.get("context_tokens").is_none());

        // run_id + occupancy → numbers serialized as strings (HashMap-safe).
        let full = build_message_metadata(
            Some("r2"),
            Some(RunContextOccupancy {
                context_tokens: 10,
                context_window: 20,
                total_tokens: 30,
                input_tokens: 8,
                output_tokens: 22,
                cost_usd: None,
                model: None,
                model_provider: None,
            }),
        )
        .expect("some");
        assert_eq!(full.get("run_id").and_then(|v| v.as_str()), Some("r2"));
        assert_eq!(
            full.get("context_tokens").and_then(|v| v.as_str()),
            Some("10")
        );
        assert_eq!(
            full.get("context_window").and_then(|v| v.as_str()),
            Some("20")
        );
        assert_eq!(
            full.get("total_tokens").and_then(|v| v.as_str()),
            Some("30")
        );
    }

    #[tokio::test]
    async fn test_tool_filtering() {
        let temp = tempdir().unwrap();
        let sm = test_session_store(&temp);

        // Test with whitelist
        let config = AgentInstanceConfig {
            agent_id: "test".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/test"),
            tool_whitelist: vec!["read_file".to_string(), "write_file".to_string()],
            ..Default::default()
        };

        let instance = AgentInstance::new(config, Arc::clone(&sm)).unwrap();
        assert!(instance.is_tool_allowed("read_file"));
        assert!(!instance.is_tool_allowed("execute_command"));

        // Test with blacklist
        let config2 = AgentInstanceConfig {
            agent_id: "test2".to_string(),
            workspace: temp.path().join("workspace2"),
            agent_dir: temp.path().join("agents/test2"),
            tool_blacklist: vec!["execute_command".to_string()],
            ..Default::default()
        };

        let instance2 = AgentInstance::new(config2, sm).unwrap();
        assert!(instance2.is_tool_allowed("read_file"));
        assert!(!instance2.is_tool_allowed("execute_command"));
    }

    #[tokio::test]
    async fn test_agent_registry() {
        let temp = tempdir().unwrap();
        let sm = test_session_store(&temp);

        let registry = AgentRegistry::new();

        let config = AgentInstanceConfig {
            agent_id: "main".to_string(),
            workspace: temp.path().join("main"),
            agent_dir: temp.path().join("agents/main"),
            ..Default::default()
        };

        let instance = AgentInstance::new(config, sm).unwrap();
        registry.register(instance).await;

        assert!(registry.get("main").await.is_some());
        assert!(registry.get("nonexistent").await.is_none());

        let agents = registry.list().await;
        assert_eq!(agents.len(), 1);
        assert!(agents.contains(&"main".to_string()));
    }

    #[tokio::test]
    async fn contains_sees_lazy_entry_without_instantiating() {
        let temp = tempdir().unwrap();
        let sm = test_session_store(&temp);
        let registry = AgentRegistry::new();
        let config = AgentInstanceConfig {
            agent_id: "lazy".to_string(),
            workspace: temp.path().join("workspaces/lazy"),
            agent_dir: temp.path().join("agents/lazy"),
            ..Default::default()
        };
        registry.register_config(config, sm).await;

        assert!(registry.contains("lazy").await);
        assert!(!registry.contains("ghost").await);
        // contains() must not have forced instantiation: removing afterwards
        // still yields the Lazy variant.
        assert!(matches!(
            registry.remove("lazy").await,
            Some(RemovedAgent::Lazy(_))
        ));
    }

    #[tokio::test]
    async fn remove_lazy_entry_reports_removed_with_workspace() {
        let temp = tempdir().unwrap();
        let sm = test_session_store(&temp);
        let registry = AgentRegistry::new();
        let ws = temp.path().join("workspaces/trading");
        let config = AgentInstanceConfig {
            agent_id: "trading".to_string(),
            workspace: ws.clone(),
            agent_dir: temp.path().join("agents/trading"),
            ..Default::default()
        };
        registry.register_config(config, sm).await;

        let removed = registry
            .remove("trading")
            .await
            .expect("lazy entry removal must report Some, not None");
        assert_eq!(removed.workspace(), ws.as_path());
        assert!(!registry.contains("trading").await);
    }

    #[tokio::test]
    async fn remove_instantiated_entry_returns_instance() {
        let temp = tempdir().unwrap();
        let sm = test_session_store(&temp);
        let registry = AgentRegistry::new();
        let config = AgentInstanceConfig {
            agent_id: "trading".to_string(),
            workspace: temp.path().join("workspaces/trading"),
            agent_dir: temp.path().join("agents/trading"),
            ..Default::default()
        };
        registry
            .register(AgentInstance::new(config, sm).unwrap())
            .await;

        match registry.remove("trading").await {
            Some(RemovedAgent::Instance(inst)) => assert_eq!(inst.id(), "trading"),
            other => panic!("expected Instance variant, got {:?}", other.is_some()),
        }
        assert!(registry.remove("trading").await.is_none());
    }

    #[test]
    fn test_agent_instance_config_from_resolved() {
        use crate::config::agent_resolver::ResolvedAgent;
        use crate::config::types::profile::ProfileConfig;

        let resolved = ResolvedAgent {
            id: "coding".to_string(),
            name: "Code Expert".to_string(),
            is_default: false,
            workspace_path: PathBuf::from("/tmp/test-workspace"),
            agent_dir: PathBuf::from("/tmp/test-agents/coding"),
            profile: ProfileConfig::default(),
            model: "claude-opus-4-6".to_string(),
            skills: vec!["git_*".to_string(), "fs_*".to_string()],
            skills_blacklist: vec![],
            subagent_policy: None,
            allowed_links: None,
            allowed_users: None,
            tool_permissions: None,
        };

        let config = AgentInstanceConfig::from_resolved(&resolved);
        assert_eq!(config.agent_id, "coding");
        assert_eq!(config.workspace, PathBuf::from("/tmp/test-workspace"));
        assert_eq!(config.model, "claude-opus-4-6");
        // The `soul_md` / `agents_md` -> `system_prompt` copy is gone; that
        // boot-time read chain had zero production readers. Real persona
        // injection happens at runtime through `IdentityFiles::load` in
        // `harness_bridge::prompt_build`, not through `AgentInstanceConfig`.
        assert_eq!(config.tool_whitelist, vec!["git_*", "fs_*"]);
        assert!(config.tool_blacklist.is_empty());
        assert_eq!(config.max_loops, 100);
    }

    #[test]
    fn test_agent_instance_config_blacklist_from_resolved() {
        use crate::config::agent_resolver::ResolvedAgent;
        use crate::config::types::profile::ProfileConfig;

        let resolved = ResolvedAgent {
            id: "restricted".to_string(),
            name: "Restricted Agent".to_string(),
            is_default: false,
            workspace_path: PathBuf::from("/tmp/test-workspace"),
            agent_dir: PathBuf::from("/tmp/test-agents/restricted"),
            profile: ProfileConfig::default(),
            model: "claude-sonnet-4-5".to_string(),
            skills: vec!["*".to_string()],
            skills_blacklist: vec!["bash".to_string(), "code_exec".to_string()],
            subagent_policy: None,
            allowed_links: None,
            allowed_users: None,
            tool_permissions: None,
        };

        let config = AgentInstanceConfig::from_resolved(&resolved);
        assert_eq!(config.tool_whitelist, vec!["*"]);
        assert_eq!(config.tool_blacklist, vec!["bash", "code_exec"]);
    }
}
