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

/// Configuration for an agent instance
#[derive(Debug, Clone)]
pub struct AgentInstanceConfig {
    /// Unique agent identifier
    pub agent_id: String,
    /// Human-readable display name (e.g., "交易助手", "Coding Agent")
    pub display_name: Option<String>,
    /// Workspace directory path
    pub workspace: PathBuf,
    /// Primary model to use
    pub model: String,
    /// Fallback models if primary fails
    pub fallback_models: Vec<String>,
    /// Maximum agent loop iterations
    pub max_loops: u32,
    /// Maximum total token usage per request (loop guard, None = use default)
    pub max_tokens: Option<usize>,
    /// Custom system prompt (optional)
    pub system_prompt: Option<String>,
    /// Tool whitelist (empty = all allowed)
    pub tool_whitelist: Vec<String>,
    /// Tool blacklist
    pub tool_blacklist: Vec<String>,
    /// Agent state directory (sessions, runtime state)
    pub agent_dir: PathBuf,
    /// Link access whitelist (None or empty = all links allowed)
    pub allowed_links: Option<Vec<String>>,
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
            workspace: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".aleph/workspaces/main"),
            model: "claude-sonnet-4-5".to_string(),
            fallback_models: vec![],
            max_loops: 100,
            max_tokens: None,
            system_prompt: None,
            tool_whitelist: vec![],
            tool_blacklist: vec![],
            agent_dir: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".aleph/agents/main"),
            allowed_links: None,
            tool_permissions: None,
            timeout_secs: None,
        }
    }
}

impl AgentInstanceConfig {
    /// Return the agent's tool permissions config, or a default (all-Allow).
    pub fn tool_permissions(&self) -> crate::config::types::policies::ToolPermissionsConfig {
        self.tool_permissions.clone().unwrap_or_default()
    }

    /// Return the agent's timeout override, if set.
    pub fn timeout_secs(&self) -> Option<u64> {
        self.timeout_secs
    }

    /// Create from a resolved agent definition.
    ///
    /// Maps ResolvedAgent fields to AgentInstanceConfig:
    /// - system_prompt <- soul_md (workspace SOUL.md content), falls back to agents_md
    /// - tool_whitelist <- skills
    /// - workspace <- workspace_path
    pub fn from_resolved(agent: &crate::config::agent_resolver::ResolvedAgent) -> Self {
        // Prioritize SOUL.md over AGENTS.md for system prompt
        let system_prompt = agent.soul_md.clone().or_else(|| agent.agents_md.clone());
        Self {
            agent_id: agent.id.clone(),
            display_name: Some(agent.name.clone()),
            workspace: agent.workspace_path.clone(),
            model: agent.model.clone(),
            fallback_models: vec![],
            max_loops: 100,
            max_tokens: None,
            system_prompt,
            tool_whitelist: agent.skills.clone(),
            tool_blacklist: agent.skills_blacklist.clone(),
            agent_dir: agent.agent_dir.clone(),
            allowed_links: agent.allowed_links.clone(),
            tool_permissions: agent.tool_permissions.clone(),
            timeout_secs: None,
        }
    }
}

/// Agent instance state
#[derive(Debug, Clone, PartialEq)]
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
/// - Separate session store (SQLite via SessionManager)
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
    /// message is also captured into the raw_memories table as a `Transcript`
    /// entry so the compression pipeline (CompressionService) can later promote
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

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
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
            AgentInstanceError::InitFailed(format!("Failed to create agent dir: {}", e))
        })?;

        std::fs::create_dir_all(&config.workspace).map_err(|e| {
            AgentInstanceError::InitFailed(format!("Failed to create workspace: {}", e))
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

    /// Get the agent ID
    pub fn id(&self) -> &str {
        &self.config.agent_id
    }

    /// Get the human-readable display name (falls back to agent_id)
    pub fn display_name(&self) -> &str {
        self.config
            .display_name
            .as_deref()
            .unwrap_or(&self.config.agent_id)
    }

    /// Get the agent configuration
    pub fn config(&self) -> &AgentInstanceConfig {
        &self.config
    }

    /// Get the workspace directory
    pub fn workspace(&self) -> &Path {
        &self.config.workspace
    }

    /// Get the agent directory
    pub fn agent_dir(&self) -> &Path {
        &self.agent_dir
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

    /// Add a message to a session (delegated to session store) and capture
    /// it into the L0 raw_memories buffer when a writer is wired.
    pub async fn add_message(&self, key: &SessionKey, role: MessageRole, content: &str) {
        let key_str = key.to_key_string();
        let role_str = match role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
            MessageRole::Tool => "tool",
        };

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
                    metadata: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    model: None,
                    model_provider: None,
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
                let body = format!("[{}] {}", role_str, content);
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

    /// Reset (clear) a session (delegated to session store)
    pub async fn reset_session(&self, key: &SessionKey) -> bool {
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
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        // Check blacklist first
        if self.config.tool_blacklist.contains(&tool_name.to_string()) {
            return false;
        }

        // If whitelist is empty or contains "*", allow all (except blacklisted)
        if self.config.tool_whitelist.is_empty()
            || self.config.tool_whitelist.contains(&"*".to_string())
        {
            return true;
        }

        // Check whitelist (supports glob prefix like "fs_*")
        self.config.tool_whitelist.iter().any(|pattern| {
            if let Some(prefix) = pattern.strip_suffix('*') {
                tool_name.starts_with(prefix)
            } else {
                pattern == tool_name
            }
        })
    }
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
    /// Construct from SessionMetadata
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
    /// Convert from MessageRecord to SessionMessage
    fn from_record(record: super::session_store::types::MessageRecord) -> Self {
        let role = match record.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            "tool" => MessageRole::Tool,
            _ => MessageRole::User,
        };
        let timestamp =
            chrono::DateTime::from_timestamp(record.timestamp, 0).unwrap_or_else(chrono::Utc::now);
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

    /// Find an agent by display name (case-insensitive substring match).
    ///
    /// Returns the agent ID if a unique match is found.
    /// Extracts display_name from either variant without instantiating.
    pub async fn find_by_name(&self, name: &str) -> Option<String> {
        let agents = self.agents.read().await;
        let name_lower = name.to_lowercase();
        let mut matched_id: Option<String> = None;

        for (id, entry) in agents.iter() {
            let display = match entry {
                AgentEntry::Instance(inst) => inst.display_name().to_lowercase(),
                AgentEntry::Config { config, .. } => config
                    .display_name
                    .as_deref()
                    .unwrap_or(&config.agent_id)
                    .to_lowercase(),
            };
            if display == name_lower
                || display.contains(&name_lower)
                || name_lower.contains(&display)
            {
                if matched_id.is_some() {
                    // Ambiguous: multiple agents match — prefer exact match
                    if display == name_lower {
                        matched_id = Some(id.clone());
                    }
                    // Otherwise keep first match
                } else {
                    matched_id = Some(id.clone());
                }
            }
        }

        matched_id
    }

    /// Get the allowed_links for an agent (None = all allowed).
    /// Extracts from either variant without instantiating.
    pub async fn get_allowed_links(&self, agent_id: &str) -> Option<Option<Vec<String>>> {
        let agents = self.agents.read().await;
        agents.get(agent_id).map(|entry| match entry {
            AgentEntry::Instance(inst) => inst.config().allowed_links.clone(),
            AgentEntry::Config { config, .. } => config.allowed_links.clone(),
        })
    }

    /// Remove an agent (works for both lazy and instantiated entries)
    pub async fn remove(&self, agent_id: &str) -> Option<Arc<AgentInstance>> {
        let mut agents = self.agents.write().await;
        match agents.remove(agent_id) {
            Some(AgentEntry::Instance(inst)) => Some(inst),
            Some(AgentEntry::Config { .. }) => None,
            None => None,
        }
    }

    /// Set the default agent
    pub fn set_default(&mut self, agent_id: impl Into<String>) {
        self.default_agent = agent_id.into();
    }

    /// Get default agent ID
    pub fn default_agent_id(&self) -> &str {
        &self.default_agent
    }

    /// Dynamically create and register a new agent at runtime.
    ///
    /// Creates `~/.aleph/agents/{id}/SOUL.md` and registers an `AgentInstance`.
    pub async fn create_dynamic(
        &self,
        id: &str,
        soul_content: &str,
        session_store: Arc<dyn SessionStore>,
    ) -> Result<Arc<AgentInstance>, AgentInstanceError> {
        // Check existence (works for both Config and Instance entries)
        {
            let agents = self.agents.read().await;
            if agents.contains_key(id) {
                return Err(AgentInstanceError::InitFailed(format!(
                    "Agent '{}' already exists",
                    id
                )));
            }
        }

        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let workspace_path = home.join(".aleph/workspaces").join(id);
        let agent_dir = home.join(".aleph/agents").join(id);

        // Create workspace directory (runtime output, project files)
        std::fs::create_dir_all(&workspace_path).map_err(|e| {
            AgentInstanceError::InitFailed(format!(
                "Failed to create workspace for '{}': {}",
                id, e
            ))
        })?;

        // Initialize all identity files (SOUL.md, AGENTS.md, IDENTITY.md, etc.)
        crate::config::agent_resolver::initialize_agent_identity(&agent_dir, id).map_err(|e| {
            AgentInstanceError::InitFailed(format!(
                "Failed to initialize identity files for '{}': {}",
                id, e
            ))
        })?;

        // Overwrite SOUL.md with custom content if provided
        if !soul_content.is_empty() {
            let soul_path = agent_dir.join("SOUL.md");
            std::fs::write(&soul_path, soul_content).map_err(|e| {
                AgentInstanceError::InitFailed(format!(
                    "Failed to write SOUL.md for '{}': {}",
                    id, e
                ))
            })?;
        }

        let config = AgentInstanceConfig {
            agent_id: id.to_string(),
            workspace: workspace_path,
            agent_dir,
            ..Default::default()
        };

        let instance = AgentInstance::new(config, session_store)?;
        let arc = Arc::new(instance);
        {
            let mut agents = self.agents.write().await;
            agents.insert(id.to_string(), AgentEntry::Instance(Arc::clone(&arc)));
        }
        info!("Dynamically created agent: {}", id);
        Ok(arc)
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
    async fn test_create_dynamic_agent() {
        let temp = tempdir().unwrap();
        let sm = test_session_store(&temp);
        let registry = AgentRegistry::new();

        // Manually create an agent to simulate create_dynamic without polluting ~/.aleph
        let config = AgentInstanceConfig {
            agent_id: "trading".to_string(),
            workspace: temp.path().join("workspaces/trading"),
            agent_dir: temp.path().join("agents/trading"),
            ..Default::default()
        };
        let instance = AgentInstance::new(config, sm).unwrap();
        registry.register(instance).await;

        let agent = registry.get("trading").await;
        assert!(agent.is_some());
        assert_eq!(agent.unwrap().id(), "trading");
    }

    #[tokio::test]
    async fn test_create_dynamic_already_exists() {
        let temp = tempdir().unwrap();
        let sm = test_session_store(&temp);
        let registry = AgentRegistry::new();
        let config = AgentInstanceConfig {
            agent_id: "main".to_string(),
            workspace: temp.path().join("main"),
            agent_dir: temp.path().join("agents/main"),
            ..Default::default()
        };
        registry
            .register(AgentInstance::new(config, Arc::clone(&sm)).unwrap())
            .await;
        let result = registry.create_dynamic("main", "soul", sm).await;
        assert!(result.is_err());
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
            soul: None,
            soul_md: Some("You are a coding expert.".to_string()),
            agents_md: Some("Be a great coder.".to_string()),
            model: "claude-opus-4-6".to_string(),
            skills: vec!["git_*".to_string(), "fs_*".to_string()],
            skills_blacklist: vec![],
            subagent_policy: None,
            allowed_links: None,
            tool_permissions: None,
        };

        let config = AgentInstanceConfig::from_resolved(&resolved);
        assert_eq!(config.agent_id, "coding");
        assert_eq!(config.workspace, PathBuf::from("/tmp/test-workspace"));
        assert_eq!(config.model, "claude-opus-4-6");
        // soul_md takes priority over agents_md
        assert_eq!(
            config.system_prompt.as_deref(),
            Some("You are a coding expert.")
        );
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
            soul: None,
            soul_md: None,
            agents_md: None,
            model: "claude-sonnet-4-5".to_string(),
            skills: vec!["*".to_string()],
            skills_blacklist: vec!["bash".to_string(), "code_exec".to_string()],
            subagent_policy: None,
            allowed_links: None,
            tool_permissions: None,
        };

        let config = AgentInstanceConfig::from_resolved(&resolved);
        assert_eq!(config.tool_whitelist, vec!["*"]);
        assert_eq!(config.tool_blacklist, vec!["bash", "code_exec"]);
    }
}
