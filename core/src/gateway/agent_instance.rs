//! Agent Instance
//!
//! Provides isolated execution environments for agents. Each agent instance
//! has its own workspace directory, session store, and configuration.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

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
            max_loops: 50,
            max_tokens: None,
            system_prompt: None,
            tool_whitelist: vec![],
            tool_blacklist: vec![],
            agent_dir: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".aleph/agents/main"),
            allowed_links: None,
        }
    }
}

impl AgentInstanceConfig {
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
            max_loops: 50,
            max_tokens: None,
            system_prompt: agent.agents_md.clone(),
            tool_whitelist: agent.skills.clone(),
            tool_blacklist: agent.skills_blacklist.clone(),
            agent_dir: agent.agent_dir.clone(),
            allowed_links: agent.allowed_links.clone(),
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
    /// Active sessions for this agent (in-memory cache)
    sessions: Arc<RwLock<HashMap<String, SessionData>>>,
    /// Optional session manager for SQLite persistence
    session_manager: Option<Arc<SessionManager>>,
}

/// Session data stored in memory
#[derive(Debug, Clone)]
struct SessionData {
    messages: Vec<SessionMessage>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_active_at: chrono::DateTime<chrono::Utc>,
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
    /// Create a new agent instance
    pub fn new(config: AgentInstanceConfig) -> Result<Self, AgentInstanceError> {
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
            sessions: Arc::new(RwLock::new(HashMap::new())),
            session_manager: None,
        })
    }

    /// Create a new agent instance with SessionManager for SQLite persistence
    pub fn with_session_manager(
        config: AgentInstanceConfig,
        session_manager: Arc<SessionManager>,
    ) -> Result<Self, AgentInstanceError> {
        let mut instance = Self::new(config)?;
        instance.session_manager = Some(session_manager);
        info!(
            "Agent '{}' connected to SessionManager for SQLite persistence",
            instance.config.agent_id
        );
        Ok(instance)
    }

    /// Get the agent ID
    pub fn id(&self) -> &str {
        &self.config.agent_id
    }

    /// Get the human-readable display name (falls back to agent_id)
    pub fn display_name(&self) -> &str {
        self.config.display_name.as_deref().unwrap_or(&self.config.agent_id)
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

    /// Get or create a session
    ///
    /// If the session exists in memory, returns it directly.
    /// Otherwise creates a new in-memory session and syncs with SessionManager (SQLite).
    pub async fn get_or_create_session(&self, key: &SessionKey) -> SessionInfo {
        let key_str = key.to_key_string();
        let mut sessions = self.sessions.write().await;

        // Check if already in memory
        if let Some(data) = sessions.get(&key_str) {
            return SessionInfo {
                key: key_str,
                agent_id: self.config.agent_id.clone(),
                message_count: data.messages.len(),
                created_at: data.created_at,
                last_active_at: data.last_active_at,
            };
        }

        // Create new session in memory
        let now = chrono::Utc::now();
        sessions.insert(key_str.clone(), SessionData {
            messages: Vec::new(),
            created_at: now,
            last_active_at: now,
        });

        // Ensure session exists in SessionManager (SQLite)
        if let Some(ref sm) = self.session_manager {
            if let Err(e) = sm.get_or_create(key).await {
                warn!("Failed to sync session to SessionManager: {}", e);
            }
        }

        let data = sessions.get(&key_str).unwrap();
        SessionInfo {
            key: key_str,
            agent_id: self.config.agent_id.clone(),
            message_count: data.messages.len(),
            created_at: data.created_at,
            last_active_at: data.last_active_at,
        }
    }

    /// Ensure a session exists in both the in-memory cache and SQLite.
    ///
    /// Must be called before `add_message()` for any new session key
    /// to guarantee the in-memory HashMap has an entry.
    pub async fn ensure_session(&self, key: &SessionKey) {
        let key_str = key.to_key_string();

        // Ensure in-memory entry exists
        {
            let mut sessions = self.sessions.write().await;
            sessions.entry(key_str.clone()).or_insert_with(|| {
                let now = chrono::Utc::now();
                SessionData {
                    messages: Vec::new(),
                    created_at: now,
                    last_active_at: now,
                }
            });
        }

        // Ensure SQLite row exists
        if let Some(ref sm) = self.session_manager {
            if let Err(e) = sm.get_or_create(key).await {
                warn!("Failed to ensure session in SessionManager: {}", e);
            }
        }
    }

    /// Add a message to a session
    ///
    /// The message is added to the in-memory session and persisted to SQLite via SessionManager.
    pub async fn add_message(&self, key: &SessionKey, role: MessageRole, content: &str) {
        let key_str = key.to_key_string();
        let role_str = match role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
            MessageRole::Tool => "tool",
        };

        // Update in-memory cache
        {
            let mut sessions = self.sessions.write().await;

            if let Some(data) = sessions.get_mut(&key_str) {
                let timestamp = chrono::Utc::now();
                data.messages.push(SessionMessage {
                    role: role.clone(),
                    content: content.to_string(),
                    timestamp,
                    metadata: None,
                });
                data.last_active_at = timestamp;
            }
        }

        // Persist to SQLite via SessionManager (if connected)
        if let Some(ref sm) = self.session_manager {
            // Ensure session exists in SessionManager
            if let Err(e) = sm.get_or_create(key).await {
                warn!("Failed to ensure session in SessionManager: {}", e);
            }
            // Add message
            if let Err(e) = sm.add_message(key, role_str, content).await {
                warn!("Failed to persist message to SQLite '{}': {}", key_str, e);
            }
        }
    }

    /// Get session history
    pub async fn get_history(&self, key: &SessionKey, limit: Option<usize>) -> Vec<SessionMessage> {
        let key_str = key.to_key_string();
        let sessions = self.sessions.read().await;

        sessions
            .get(&key_str)
            .map(|data| {
                let messages = &data.messages;
                match limit {
                    Some(n) => messages.iter().rev().take(n).rev().cloned().collect(),
                    None => messages.clone(),
                }
            })
            .unwrap_or_default()
    }

    /// Reset (clear) a session
    ///
    /// Clears in-memory messages.
    pub async fn reset_session(&self, key: &SessionKey) -> bool {
        let key_str = key.to_key_string();
        let mut sessions = self.sessions.write().await;

        if let Some(data) = sessions.get_mut(&key_str) {
            data.messages.clear();
            data.last_active_at = chrono::Utc::now();

            debug!("Reset session: {}", key_str);
            true
        } else {
            false
        }
    }

    /// List all sessions for this agent
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.read().await;

        sessions
            .iter()
            .map(|(key, data)| SessionInfo {
                key: key.clone(),
                agent_id: self.config.agent_id.clone(),
                message_count: data.messages.len(),
                created_at: data.created_at,
                last_active_at: data.last_active_at,
            })
            .collect()
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
            if display == name_lower || display.contains(&name_lower) || name_lower.contains(&display) {
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
        agents.get(agent_id).map(|a| a.config().allowed_links.clone())
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
        session_manager: Option<Arc<super::session_manager::SessionManager>>,
    ) -> Result<Arc<AgentInstance>, AgentInstanceError> {
        if self.get(id).await.is_some() {
            return Err(AgentInstanceError::InitFailed(
                format!("Agent '{}' already exists", id),
            ));
        }

        let home = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let workspace_path = home.join(".aleph/workspaces").join(id);
        let agent_dir = home.join(".aleph/agents").join(id);

        std::fs::create_dir_all(&workspace_path).map_err(|e| {
            AgentInstanceError::InitFailed(format!(
                "Failed to create workspace for '{}': {}", id, e
            ))
        })?;

        let soul_path = workspace_path.join("SOUL.md");
        if !soul_path.exists() {
            std::fs::write(&soul_path, soul_content).map_err(|e| {
                AgentInstanceError::InitFailed(format!(
                    "Failed to write SOUL.md for '{}': {}", id, e
                ))
            })?;
        }

        let config = AgentInstanceConfig {
            agent_id: id.to_string(),
            workspace: workspace_path,
            agent_dir,
            ..Default::default()
        };

        let instance = if let Some(sm) = session_manager {
            AgentInstance::with_session_manager(config, sm)?
        } else {
            AgentInstance::new(config)?
        };

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
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_agent_instance_creation() {
        let temp = tempdir().unwrap();
        let config = AgentInstanceConfig {
            agent_id: "test-agent".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/test-agent"),
            ..Default::default()
        };

        let instance = AgentInstance::new(config).unwrap();
        assert_eq!(instance.id(), "test-agent");
        assert!(instance.is_idle().await);
    }

    #[tokio::test]
    async fn test_session_management() {
        let temp = tempdir().unwrap();
        let config = AgentInstanceConfig {
            agent_id: "test".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/test"),
            ..Default::default()
        };

        let instance = AgentInstance::new(config).unwrap();
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

        // Test with whitelist
        let config = AgentInstanceConfig {
            agent_id: "test".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/test"),
            tool_whitelist: vec!["read_file".to_string(), "write_file".to_string()],
            ..Default::default()
        };

        let instance = AgentInstance::new(config).unwrap();
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

        let instance2 = AgentInstance::new(config2).unwrap();
        assert!(instance2.is_tool_allowed("read_file"));
        assert!(!instance2.is_tool_allowed("execute_command"));
    }

    #[tokio::test]
    async fn test_agent_registry() {
        let temp = tempdir().unwrap();

        let registry = AgentRegistry::new();

        let config = AgentInstanceConfig {
            agent_id: "main".to_string(),
            workspace: temp.path().join("main"),
            agent_dir: temp.path().join("agents/main"),
            ..Default::default()
        };

        let instance = AgentInstance::new(config).unwrap();
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
        let registry = AgentRegistry::new();

        // Manually create an agent to simulate create_dynamic without polluting ~/.aleph
        let config = AgentInstanceConfig {
            agent_id: "trading".to_string(),
            workspace: temp.path().join("workspaces/trading"),
            agent_dir: temp.path().join("agents/trading"),
            ..Default::default()
        };
        let instance = AgentInstance::new(config).unwrap();
        registry.register(instance).await;

        let agent = registry.get("trading").await;
        assert!(agent.is_some());
        assert_eq!(agent.unwrap().id(), "trading");
    }

    #[tokio::test]
    async fn test_create_dynamic_already_exists() {
        let temp = tempdir().unwrap();
        let registry = AgentRegistry::new();
        let config = AgentInstanceConfig {
            agent_id: "main".to_string(),
            workspace: temp.path().join("main"),
            agent_dir: temp.path().join("agents/main"),
            ..Default::default()
        };
        registry
            .register(AgentInstance::new(config).unwrap())
            .await;
        let result = registry.create_dynamic("main", "soul", None).await;
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
        };

        let config = AgentInstanceConfig::from_resolved(&resolved);
        assert_eq!(config.agent_id, "coding");
        assert_eq!(config.workspace, PathBuf::from("/tmp/test-workspace"));
        assert_eq!(config.model, "claude-opus-4-6");
        assert_eq!(config.system_prompt.as_deref(), Some("Be a great coder."));
        assert_eq!(config.tool_whitelist, vec!["git_*", "fs_*"]);
        assert!(config.tool_blacklist.is_empty());
        assert_eq!(config.max_loops, 50);
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
        };

        let config = AgentInstanceConfig::from_resolved(&resolved);
        assert_eq!(config.tool_whitelist, vec!["*"]);
        assert_eq!(config.tool_blacklist, vec!["bash", "code_exec"]);
    }
}
