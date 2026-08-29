use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::a2a::domain::{AgentCard, TrustLevel};

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
#[derive(Clone, Serialize, Deserialize)]
pub struct RegisteredAgent {
    pub card: AgentCard,
    pub trust_level: TrustLevel,
    pub base_url: String,
    pub last_seen: DateTime<Utc>,
    pub health: AgentHealth,
    /// Auth token for outbound requests to this agent
    ///
    /// SECURITY: never logged, never serialized through Debug. Callers that
    /// need the raw token must go through `auth_token()` (which is gated to
    /// trusted callers). Public-field access is intentionally avoided to
    /// prevent accidental leakage via `{:?}` or `serde_json::to_string`.
    /// See A2A-R3-21.
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_token: Option<String>,
}

impl std::fmt::Debug for RegisteredAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredAgent")
            .field("card", &self.card)
            .field("trust_level", &self.trust_level)
            .field("base_url", &self.base_url)
            .field("last_seen", &self.last_seen)
            .field("health", &self.health)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl RegisteredAgent {
    /// Construct a `RegisteredAgent` (used by registry / tests).
    pub fn new(
        card: AgentCard,
        trust_level: TrustLevel,
        base_url: String,
        last_seen: DateTime<Utc>,
        health: AgentHealth,
        auth_token: Option<String>,
    ) -> Self {
        Self {
            card,
            trust_level,
            base_url,
            last_seen,
            health,
            auth_token,
        }
    }

    /// Read-only accessor for the auth token. Only callers that actually
    /// need to authenticate outbound calls should use this.
    #[must_use]
    pub fn auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }
}

/// Port for discovering and managing remote A2A agents.
///
/// Provides explicit registration and lookup. Remote card fetching is handled
/// directly by `A2AClient` (see `service::card_refresh`); intent-based routing
/// is handled by `service::SmartRouter`.
#[async_trait::async_trait]
pub trait AgentResolver: Send + Sync {
    /// Register a remote agent with a trust level.
    ///
    /// **Deprecated**: prefer [`register_with_token`] when the caller has an
    /// outbound bearer token. This method always stores `None` for the token,
    /// which silently downgrades authenticated agents to anonymous — the
    /// caller sees `Ok(())` and may not realise the next outbound RPC will
    /// be rejected with 401.
    #[deprecated(
        since = "0.1.0",
        note = "Use register_with_token so the outbound bearer token is preserved"
    )]
    async fn register(
        &self,
        card: AgentCard,
        base_url: &str,
        trust_level: TrustLevel,
    ) -> A2AResult<()>;

    /// Register a remote agent, optionally attaching an outbound bearer
    /// token that subsequent RPCs to this agent will use.
    async fn register_with_token(
        &self,
        card: AgentCard,
        base_url: &str,
        trust_level: TrustLevel,
        auth_token: Option<String>,
    ) -> A2AResult<()>;

    /// Unregister an agent by ID
    async fn unregister(&self, agent_id: &str) -> A2AResult<()>;

    /// List all registered agents
    async fn list_agents(&self) -> A2AResult<Vec<RegisteredAgent>>;

    /// Look up a registered agent by its ID
    async fn resolve_by_id(&self, agent_id: &str) -> A2AResult<Option<RegisteredAgent>>;
}
