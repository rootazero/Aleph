use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::a2a::domain::*;

use super::task_manager::A2AResult;

/// Health status of a registered agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentHealth {
    Healthy,
    Degraded,
    Unreachable,
}

/// A registered remote agent with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredAgent {
    pub card: AgentCard,
    pub trust_level: TrustLevel,
    pub base_url: String,
    pub last_seen: DateTime<Utc>,
    pub health: AgentHealth,
}

/// Port for discovering and managing remote A2A agents.
///
/// Supports both explicit registration and intent-based smart routing.
#[async_trait::async_trait]
pub trait AgentResolver: Send + Sync {
    /// Fetch an agent card from a remote URL (e.g. `/.well-known/agent-card.json`)
    async fn fetch_card(&self, url: &str) -> A2AResult<AgentCard>;

    /// Register a remote agent with a trust level
    async fn register(
        &self,
        card: AgentCard,
        base_url: &str,
        trust_level: TrustLevel,
    ) -> A2AResult<()>;

    /// Unregister an agent by ID
    async fn unregister(&self, agent_id: &str) -> A2AResult<()>;

    /// List all registered agents
    async fn list_agents(&self) -> A2AResult<Vec<RegisteredAgent>>;

    /// Look up a registered agent by its ID
    async fn resolve_by_id(&self, agent_id: &str) -> A2AResult<Option<RegisteredAgent>>;

    /// LLM smart routing: match best agent from natural language description
    async fn resolve_by_intent(&self, intent: &str) -> A2AResult<Option<RegisteredAgent>>;
}
