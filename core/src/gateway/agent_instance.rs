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
use super::session_storage::SessionStorage;

/// Configuration for an agent instance
#[derive(Debug, Clone)]
pub struct AgentInstanceConfig {
    /// Unique agent identifier
    pub agent_id: String,
    /// Workspace directory path
    pub workspace: PathBuf,
    /// Primary model to use
    pub model: String,
    /// Fallback models if primary fails
    pub fallback_models: Vec<String>,
    /// Maximum agent loop iterations
    pub max_loops: u32,
    /// Custom system prompt (optional)
    pub system_prompt: Option<String>,
    /// Tool whitelist (empty = all allowed)
    pub tool_whitelist: Vec<String>,
    /// Tool blacklist
    pub tool_blacklist: Vec<String>,
}

impl Default for AgentInstanceConfig {
    fn default() -> Self {
        Self {
            agent_id: "main".to_string(),
            workspace: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".aleph/agents/main/workspace"),
            model: "claude-sonnet-4-5".to_string(),
            fallback_models: vec![],
            max_loops: 20,
            system_prompt: None,
            tool_whitelist: vec![],
            tool_blacklist: vec![],
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
/// - Separate session store (JSONL files + optional SQLite via SessionManager)
/// - Independent configuration
/// - Isolated state
pub struct AgentInstance {
    /// Agent configuration
    config: AgentInstanceConfig,
    /// Current agent state
    state: Arc<RwLock<AgentState>>,
    /// Agent directory (contains workspace, sessions/, config)
    agent_dir: PathBuf,
    /// Active sessions for this agent (in-memory cache)
    sessions: Arc<RwLock<HashMap<String, SessionData>>>,
    /// Persistent session storage (JSONL files)
    storage: Option<SessionStorage>,
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
        let agents_dir = dirs::home_dir()
            .ok_or_else(|| AgentInstanceError::InitFailed("No home directory".to_string()))?
            .join(".aleph/agents");

        let agent_dir = agents_dir.join(&config.agent_id);

        // Validate no path traversal
        if !agent_dir.starts_with(&agents_dir) {
            return Err(AgentInstanceError::InitFailed(
                "Invalid agent directory (path traversal attempt)".to_string(),
            ));
        }

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

        // Initialize session storage
        let storage = match SessionStorage::new(&agent_dir) {
            Ok(s) => {
                info!("Session storage initialized for agent '{}'", config.agent_id);
                Some(s)
            }
            Err(e) => {
                warn!(
                    "Failed to initialize session storage for agent '{}': {}. Sessions will not be persisted.",
                    config.agent_id, e
                );
                None
            }
        };

        info!(
            "Created agent instance '{}' at {:?}",
            config.agent_id, agent_dir
        );

        Ok(Self {
            config,
            state: Arc::new(RwLock::new(AgentState::Idle)),
            agent_dir,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            storage,
            session_manager: None,
        })
    }

    /// Create a new agent instance with SessionManager for SQLite persistence
    ///
    /// Messages are persisted both to JSONL files and to SQLite via SessionManager.
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
    /// If the session exists on disk (JSONL), it will be loaded into memory.
    /// If not, a new session is created both in memory and on disk.
    /// Also syncs with SessionManager (SQLite) if connected.
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

        // Try to load from storage
        let mut loaded_from_disk = false;
        if let Some(ref storage) = self.storage {
            if let Some(loaded) = storage.load_session(&key_str) {
                debug!("Loaded session '{}' from disk with {} messages", key_str, loaded.messages.len());
                sessions.insert(key_str.clone(), SessionData {
                    messages: loaded.messages,
                    created_at: loaded.meta.created_at,
                    last_active_at: chrono::Utc::now(),
                });
                loaded_from_disk = true;
            }
        }

        // Create new session if not loaded from disk
        if !loaded_from_disk {
            let now = chrono::Utc::now();
            sessions.insert(key_str.clone(), SessionData {
                messages: Vec::new(),
                created_at: now,
                last_active_at: now,
            });

            // Create on disk
            if let Some(ref storage) = self.storage {
                if let Err(e) = storage.create_session(&key_str, &self.config.agent_id) {
                    warn!("Failed to create session file for '{}': {}", key_str, e);
                }
            }
        }

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
    /// The message is added to the in-memory session and persisted to disk (JSONL).
    /// If SessionManager is connected, also persists to SQLite.
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

                // Persist to JSONL storage
                if let Some(ref storage) = self.storage {
                    if let Err(e) = storage.append_message(&key_str, role.clone(), content, None) {
                        warn!("Failed to persist message to JSONL '{}': {}", key_str, e);
                    }
                }
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
    /// Clears in-memory messages and marks the session as reset in storage.
    pub async fn reset_session(&self, key: &SessionKey) -> bool {
        let key_str = key.to_key_string();
        let mut sessions = self.sessions.write().await;

        if let Some(data) = sessions.get_mut(&key_str) {
            data.messages.clear();
            data.last_active_at = chrono::Utc::now();

            // Mark as reset in storage
            if let Some(ref storage) = self.storage {
                if let Err(e) = storage.reset_session(&key_str) {
                    warn!("Failed to reset session '{}' in storage: {}", key_str, e);
                }
            }

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

        // If whitelist is empty, allow all (except blacklisted)
        if self.config.tool_whitelist.is_empty() {
            return true;
        }

        // Check whitelist
        self.config.tool_whitelist.contains(&tool_name.to_string())
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
}
