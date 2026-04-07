//! Agent Instance
//!
//! Provides isolated execution environments for agents. Each agent instance
//! has its own workspace directory, session store, and configuration.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::session_manager::StoredMessage;

use super::router::SessionKey;
use super::session_manager::SessionManager;

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
    /// - system_prompt <- agents_md (workspace AGENTS.md content)
    /// - tool_whitelist <- skills
    /// - workspace <- workspace_path
    pub fn from_resolved(agent: &crate::config::agent_resolver::ResolvedAgent) -> Self {
        Self {
            agent_id: agent.id.clone(),
            display_name: Some(agent.name.clone()),
            workspace: agent.workspace_path.clone(),
            model: agent.model.clone(),
            fallback_models: vec![],
            max_loops: 100,
            max_tokens: None,
            system_prompt: agent.agents_md.clone(),
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
    /// Session manager for SQLite persistence
    session_manager: Arc<SessionManager>,
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
    /// Create a new agent instance with SessionManager for SQLite persistence
    pub fn new(
        config: AgentInstanceConfig,
        session_manager: Arc<SessionManager>,
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
            "Created agent instance '{}' at {:?} (SQLite persistence)",
            config.agent_id, agent_dir
        );

        Ok(Self {
            config,
            state: Arc::new(RwLock::new(AgentState::Idle)),
            agent_dir,
            session_manager,
        })
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

    /// Get or create a session (delegated to SessionManager / SQLite)
    pub async fn get_or_create_session(&self, key: &SessionKey) -> SessionInfo {
        match self.session_manager.get_or_create(key).await {
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

    /// Ensure a session exists in SQLite.
    pub async fn ensure_session(&self, key: &SessionKey) {
        if let Err(e) = self.session_manager.get_or_create(key).await {
            warn!("Failed to ensure session in SessionManager: {}", e);
        }
    }

    /// Add a message to a session (delegated to SessionManager / SQLite)
    pub async fn add_message(&self, key: &SessionKey, role: MessageRole, content: &str) {
        let key_str = key.to_key_string();
        let role_str = match role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
            MessageRole::Tool => "tool",
        };

        // Ensure session exists, then add message
        if let Err(e) = self.session_manager.get_or_create(key).await {
            warn!("Failed to ensure session in SessionManager: {}", e);
        }
        if let Err(e) = self.session_manager.add_message(key, role_str, content).await {
            warn!("Failed to persist message to SQLite '{}': {}", key_str, e);
        }
    }

    /// Get session history (delegated to SessionManager / SQLite)
    pub async fn get_history(&self, key: &SessionKey, limit: Option<usize>) -> Vec<SessionMessage> {
        match self.session_manager.get_history(key, limit).await {
            Ok(stored) => stored.into_iter().map(SessionMessage::from_stored).collect(),
            Err(e) => {
                warn!("Failed to get history from SessionManager: {}", e);
                Vec::new()
            }
        }
    }

    /// Reset (clear) a session (delegated to SessionManager / SQLite)
    pub async fn reset_session(&self, key: &SessionKey) -> bool {
        match self.session_manager.reset_session(key).await {
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

    /// List all sessions for this agent (delegated to SessionManager / SQLite)
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        match self
            .session_manager
            .list_sessions(Some(&self.config.agent_id))
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
    /// Construct from SessionMetadata (SQLite)
    fn from_metadata(meta: &super::session_manager::SessionMetadata) -> Self {
        Self {
            key: meta.key.clone(),
            agent_id: meta.agent_id.clone(),
            message_count: meta.message_count as usize,
            created_at: chrono::DateTime::from_timestamp(meta.created_at, 0)
                .unwrap_or_else(|| chrono::Utc::now()),
            last_active_at: chrono::DateTime::from_timestamp(meta.last_active_at, 0)
                .unwrap_or_else(|| chrono::Utc::now()),
        }
    }
}

impl SessionMessage {
    /// Convert from StoredMessage (SQLite) to SessionMessage
    fn from_stored(stored: StoredMessage) -> Self {
        let role = match stored.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            "tool" => MessageRole::Tool,
            _ => MessageRole::User,
        };
        let timestamp = chrono::DateTime::from_timestamp(stored.timestamp, 0)
            .unwrap_or_else(|| chrono::Utc::now());
        let metadata = stored.metadata.and_then(|json_str| {
            serde_json::from_str::<HashMap<String, String>>(&json_str).ok()
        });
        Self {
            role,
            content: stored.content,
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

/// Registry of agent instances
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, Arc<AgentInstance>>>>,
    default_agent: String,
}

impl AgentRegistry {
    /// Create a new registry with default "main" agent
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            default_agent: "main".to_string(),
        }
    }

    /// Register an agent instance
    pub async fn register(&self, instance: AgentInstance) {
        let id = instance.id().to_string();
        let mut agents = self.agents.write().await;
        agents.insert(id.clone(), Arc::new(instance));
        info!("Registered agent: {}", id);
    }

    /// Get an agent by ID
    pub async fn get(&self, agent_id: &str) -> Option<Arc<AgentInstance>> {
        let agents = self.agents.read().await;
        agents.get(agent_id).cloned()
    }

    /// Get the default agent
    pub async fn get_default(&self) -> Option<Arc<AgentInstance>> {
        self.get(&self.default_agent).await
    }

    /// List all registered agents
    pub async fn list(&self) -> Vec<String> {
        let agents = self.agents.read().await;
        agents.keys().cloned().collect()
    }

    /// Find an agent by display name (case-insensitive substring match).
    ///
    /// Returns the agent ID if a unique match is found.
    pub async fn find_by_name(&self, name: &str) -> Option<String> {
        let agents = self.agents.read().await;
        let name_lower = name.to_lowercase();
        let mut matched_id: Option<String> = None;

        for (id, instance) in agents.iter() {
            let display = instance.display_name().to_lowercase();
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

    /// Get the allowed_links for an agent (None = all allowed)
    pub async fn get_allowed_links(&self, agent_id: &str) -> Option<Option<Vec<String>>> {
        let agents = self.agents.read().await;
        agents
            .get(agent_id)
            .map(|a| a.config().allowed_links.clone())
    }

    /// Remove an agent
    pub async fn remove(&self, agent_id: &str) -> Option<Arc<AgentInstance>> {
        let mut agents = self.agents.write().await;
        agents.remove(agent_id)
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
    /// Creates `~/.aleph/workspaces/{id}/SOUL.md` and registers an `AgentInstance`.
    pub async fn create_dynamic(
        &self,
        id: &str,
        soul_content: &str,
        session_manager: Arc<super::session_manager::SessionManager>,
    ) -> Result<Arc<AgentInstance>, AgentInstanceError> {
        if self.get(id).await.is_some() {
            return Err(AgentInstanceError::InitFailed(format!(
                "Agent '{}' already exists",
                id
            )));
        }

        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let workspace_path = home.join(".aleph/workspaces").join(id);
        let agent_dir = home.join(".aleph/agents").join(id);

        std::fs::create_dir_all(&workspace_path).map_err(|e| {
            AgentInstanceError::InitFailed(format!(
                "Failed to create workspace for '{}': {}",
                id, e
            ))
        })?;

        let soul_path = workspace_path.join("SOUL.md");
        if !soul_path.exists() {
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

        let instance = AgentInstance::new(config, session_manager)?;

        self.register(instance).await;
        let agent = self.get(id).await.unwrap();
        info!("Dynamically created agent: {}", id);
        Ok(agent)
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
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use tempfile::tempdir;

    /// Create a test SessionManager backed by a temporary SQLite database.
    fn test_session_manager(temp: &tempfile::TempDir) -> Arc<SessionManager> {
        let config = SessionManagerConfig {
            db_path: temp.path().join("test_sessions.db"),
            ..Default::default()
        };
        Arc::new(SessionManager::new(config).expect("test session manager"))
    }

    #[tokio::test]
    async fn test_agent_instance_creation() {
        let temp = tempdir().unwrap();
        let sm = test_session_manager(&temp);
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
        let sm = test_session_manager(&temp);
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
        let sm = test_session_manager(&temp);

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
        let sm = test_session_manager(&temp);

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
        let sm = test_session_manager(&temp);
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
        let sm = test_session_manager(&temp);
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
            agents_md: Some("Be a great coder.".to_string()),
            memory_md: None,
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
        assert_eq!(config.system_prompt.as_deref(), Some("Be a great coder."));
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
            agents_md: None,
            memory_md: None,
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
