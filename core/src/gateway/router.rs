//! Agent Router
//!
//! Routes incoming requests to the appropriate agent based on session key,
//! channel, or peer information.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

// Re-export new routing types for backward compatibility.
// Existing code using gateway::router::SessionKey will continue to work.
pub use crate::routing::SessionKey as NewSessionKey;
pub use crate::routing::{DmScope, PeerKind};

/// Session key types for hierarchical session management
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionKey {
    /// Main session (cross-device shared)
    Main {
        agent_id: String,
        #[serde(default = "default_main_key")]
        main_key: String,
    },

    /// Per-peer isolation (different GUI windows, chat conversations)
    PerPeer {
        agent_id: String,
        peer_id: String,
    },

    /// Task isolation (cron jobs, webhooks, scheduled tasks)
    Task {
        agent_id: String,
        task_type: String,
        task_id: String,
    },

    /// Ephemeral (single-turn, no persistence)
    Ephemeral {
        agent_id: String,
        ephemeral_id: String,
    },
}

fn default_main_key() -> String {
    "main".to_string()
}

impl SessionKey {
    /// Create a main session key
    pub fn main(agent_id: impl Into<String>) -> Self {
        Self::Main {
            agent_id: agent_id.into(),
            main_key: "main".to_string(),
        }
    }

    /// Create a per-peer session key
    pub fn peer(agent_id: impl Into<String>, peer_id: impl Into<String>) -> Self {
        Self::PerPeer {
            agent_id: agent_id.into(),
            peer_id: peer_id.into(),
        }
    }

    /// Create a task session key
    pub fn task(
        agent_id: impl Into<String>,
        task_type: impl Into<String>,
        task_id: impl Into<String>,
    ) -> Self {
        Self::Task {
            agent_id: agent_id.into(),
            task_type: task_type.into(),
            task_id: task_id.into(),
        }
    }

    /// Create an ephemeral session key
    pub fn ephemeral(agent_id: impl Into<String>) -> Self {
        Self::Ephemeral {
            agent_id: agent_id.into(),
            ephemeral_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// Get the agent ID from this session key
    pub fn agent_id(&self) -> &str {
        match self {
            Self::Main { agent_id, .. } => agent_id,
            Self::PerPeer { agent_id, .. } => agent_id,
            Self::Task { agent_id, .. } => agent_id,
            Self::Ephemeral { agent_id, .. } => agent_id,
        }
    }

    /// Convert to a string key for storage/lookup
    pub fn to_key_string(&self) -> String {
        match self {
            Self::Main { agent_id, main_key } => format!("agent:{}:{}", agent_id, main_key),
            Self::PerPeer { agent_id, peer_id } => format!("agent:{}:peer:{}", agent_id, peer_id),
            Self::Task {
                agent_id,
                task_type,
                task_id,
            } => format!("agent:{}:{}:{}", agent_id, task_type, task_id),
            Self::Ephemeral {
                agent_id,
                ephemeral_id,
            } => format!("agent:{}:ephemeral:{}", agent_id, ephemeral_id),
        }
    }

    /// Parse a session key from a string
    pub fn from_key_string(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();

        if parts.len() < 3 || parts[0] != "agent" {
            return None;
        }

        let agent_id = parts[1].to_string();

        match parts.get(2) {
            Some(&"peer") if parts.len() >= 4 => Some(Self::PerPeer {
                agent_id,
                peer_id: parts[3..].join(":"),
            }),
            Some(&"ephemeral") if parts.len() >= 4 => Some(Self::Ephemeral {
                agent_id,
                ephemeral_id: parts[3].to_string(),
            }),
            Some(&"cron") | Some(&"webhook") | Some(&"scheduled") if parts.len() >= 4 => {
                Some(Self::Task {
                    agent_id,
                    task_type: parts[2].to_string(),
                    task_id: parts[3].to_string(),
                })
            }
            Some(main_key) => Some(Self::Main {
                agent_id,
                main_key: main_key.to_string(),
            }),
            None => None,
        }
    }
}

// Backward compatibility: convert between old and new SessionKey
impl SessionKey {
    /// Convert legacy SessionKey to new routing SessionKey
    pub fn to_new(&self) -> crate::routing::SessionKey {
        match self {
            Self::Main { agent_id, main_key } => crate::routing::SessionKey::Main {
                agent_id: agent_id.clone(),
                main_key: main_key.clone(),
            },
            Self::PerPeer { agent_id, peer_id } => crate::routing::SessionKey::DirectMessage {
                agent_id: agent_id.clone(),
                channel: String::new(),
                peer_id: peer_id.clone(),
                dm_scope: crate::routing::DmScope::PerPeer,
            },
            Self::Task { agent_id, task_type, task_id } => crate::routing::SessionKey::Task {
                agent_id: agent_id.clone(),
                task_type: task_type.clone(),
                task_id: task_id.clone(),
            },
            Self::Ephemeral { agent_id, ephemeral_id } => crate::routing::SessionKey::Ephemeral {
                agent_id: agent_id.clone(),
                ephemeral_id: ephemeral_id.clone(),
            },
        }
    }

    /// Create legacy SessionKey from new routing SessionKey
    pub fn from_new(key: &crate::routing::SessionKey) -> Self {
        match key {
            crate::routing::SessionKey::Main { agent_id, main_key } => Self::Main {
                agent_id: agent_id.clone(),
                main_key: main_key.clone(),
            },
            crate::routing::SessionKey::DirectMessage { agent_id, peer_id, .. } => Self::PerPeer {
                agent_id: agent_id.clone(),
                peer_id: peer_id.clone(),
            },
            crate::routing::SessionKey::Group { agent_id, peer_id, .. } => Self::PerPeer {
                agent_id: agent_id.clone(),
                peer_id: peer_id.clone(),
            },
            crate::routing::SessionKey::Task { agent_id, task_type, task_id } => Self::Task {
                agent_id: agent_id.clone(),
                task_type: task_type.clone(),
                task_id: task_id.clone(),
            },
            crate::routing::SessionKey::Subagent { parent_key, .. } => Self::from_new(parent_key),
            crate::routing::SessionKey::Ephemeral { agent_id, ephemeral_id } => Self::Ephemeral {
                agent_id: agent_id.clone(),
                ephemeral_id: ephemeral_id.clone(),
            },
        }
    }
}

/// Routing binding configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingBinding {
    /// Pattern to match (e.g., "gui:window1", "cli:*", "telegram:*")
    pub pattern: String,
    /// Target agent ID
    pub agent_id: String,
}

/// Agent router for directing requests to appropriate agents
///
/// Routes requests based on:
/// 1. Explicit session key
/// 2. Peer/channel matching
/// 3. Default agent fallback
pub struct AgentRouter {
    /// Routing bindings (pattern -> agent_id)
    bindings: Arc<RwLock<Vec<RoutingBinding>>>,
    /// Default agent ID
    default_agent: String,
    /// Available agent IDs
    agents: Arc<RwLock<Vec<String>>>,
}

impl AgentRouter {
    /// Create a new router with default "main" agent
    pub fn new() -> Self {
        Self {
            bindings: Arc::new(RwLock::new(Vec::new())),
            default_agent: "main".to_string(),
            agents: Arc::new(RwLock::new(vec!["main".to_string()])),
        }
    }

    /// Create a router with custom default agent
    pub fn with_default(default_agent: impl Into<String>) -> Self {
        let default = default_agent.into();
        Self {
            bindings: Arc::new(RwLock::new(Vec::new())),
            default_agent: default.clone(),
            agents: Arc::new(RwLock::new(vec![default])),
        }
    }

    /// Add a routing binding
    pub async fn add_binding(&self, pattern: impl Into<String>, agent_id: impl Into<String>) {
        let binding = RoutingBinding {
            pattern: pattern.into(),
            agent_id: agent_id.into(),
        };
        self.bindings.write().await.push(binding);
    }

    /// Register an available agent
    pub async fn register_agent(&self, agent_id: impl Into<String>) {
        let id = agent_id.into();
        let mut agents = self.agents.write().await;
        if !agents.contains(&id) {
            agents.push(id);
        }
    }

    /// List available agents
    pub async fn list_agents(&self) -> Vec<String> {
        self.agents.read().await.clone()
    }

    /// Route a request to an agent
    ///
    /// # Arguments
    ///
    /// * `session_key` - Optional explicit session key
    /// * `channel` - Channel identifier (e.g., "gui:window1", "cli:term1")
    /// * `peer_id` - Optional peer identifier
    ///
    /// # Returns
    ///
    /// The resolved session key for this request
    pub async fn route(
        &self,
        session_key: Option<&str>,
        channel: Option<&str>,
        peer_id: Option<&str>,
    ) -> SessionKey {
        // 1. If explicit session key provided, parse and use it
        if let Some(key_str) = session_key {
            if let Some(key) = SessionKey::from_key_string(key_str) {
                return key;
            }
        }

        // 2. Try to match channel/peer against bindings
        let agent_id = self.resolve_agent(channel, peer_id).await;

        // 3. Create appropriate session key
        match peer_id {
            Some(pid) => SessionKey::peer(&agent_id, pid),
            None => SessionKey::main(&agent_id),
        }
    }

    /// Resolve agent ID from channel/peer
    async fn resolve_agent(&self, channel: Option<&str>, _peer_id: Option<&str>) -> String {
        let bindings = self.bindings.read().await;

        // Try exact match first
        if let Some(ch) = channel {
            for binding in bindings.iter() {
                if binding.pattern == ch {
                    return binding.agent_id.clone();
                }
            }

            // Try wildcard match
            let channel_prefix = ch.split(':').next().unwrap_or("");
            let wildcard = format!("{}:*", channel_prefix);
            for binding in bindings.iter() {
                if binding.pattern == wildcard {
                    return binding.agent_id.clone();
                }
            }
        }

        // Fall back to default
        self.default_agent.clone()
    }

    /// Get the default agent ID
    pub fn default_agent(&self) -> &str {
        &self.default_agent
    }
}

impl Default for AgentRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_key_main() {
        let key = SessionKey::main("main");
        assert_eq!(key.agent_id(), "main");
        assert_eq!(key.to_key_string(), "agent:main:main");
    }

    #[test]
    fn test_session_key_peer() {
        let key = SessionKey::peer("work", "window-123");
        assert_eq!(key.agent_id(), "work");
        assert_eq!(key.to_key_string(), "agent:work:peer:window-123");
    }

    #[test]
    fn test_session_key_task() {
        let key = SessionKey::task("main", "cron", "daily-summary");
        assert_eq!(key.to_key_string(), "agent:main:cron:daily-summary");
    }

    #[test]
    fn test_session_key_parse() {
        let key = SessionKey::from_key_string("agent:main:main").unwrap();
        assert!(matches!(key, SessionKey::Main { .. }));

        let key = SessionKey::from_key_string("agent:work:peer:window-1").unwrap();
        assert!(matches!(key, SessionKey::PerPeer { peer_id, .. } if peer_id == "window-1"));

        let key = SessionKey::from_key_string("agent:main:cron:job-1").unwrap();
        assert!(matches!(key, SessionKey::Task { task_type, .. } if task_type == "cron"));
    }

    #[test]
    fn test_session_key_parse_invalid() {
        assert!(SessionKey::from_key_string("invalid").is_none());
        assert!(SessionKey::from_key_string("agent:").is_none());
    }

    #[tokio::test]
    async fn test_router_default() {
        let router = AgentRouter::new();
        let key = router.route(None, None, None).await;
        assert_eq!(key.agent_id(), "main");
    }

    #[tokio::test]
    async fn test_router_explicit_key() {
        let router = AgentRouter::new();
        let key = router.route(Some("agent:work:main"), None, None).await;
        assert_eq!(key.agent_id(), "work");
    }

    #[tokio::test]
    async fn test_router_binding() {
        let router = AgentRouter::new();
        router.register_agent("work").await;
        router.add_binding("cli:*", "work").await;

        let key = router.route(None, Some("cli:term1"), None).await;
        assert_eq!(key.agent_id(), "work");

        // GUI should still go to default
        let key = router.route(None, Some("gui:window1"), None).await;
        assert_eq!(key.agent_id(), "main");
    }

    #[tokio::test]
    async fn test_router_peer_creates_peer_session() {
        let router = AgentRouter::new();
        let key = router.route(None, Some("gui:window1"), Some("telegram:123")).await;

        assert!(matches!(key, SessionKey::PerPeer { peer_id, .. } if peer_id == "telegram:123"));
    }
}
