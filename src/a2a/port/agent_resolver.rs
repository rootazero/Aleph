use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::a2a::domain::{A2AError, AgentCard, TrustLevel};

use super::task_manager::A2AResult;

/// Health status of a registered agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentHealth {
    Healthy,
    Degraded,
    Unreachable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RpcProtocol {
    Legacy,
    V03,
    V1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedEndpoint {
    url: Url,
    protocol: RpcProtocol,
    streaming: bool,
}

impl ResolvedEndpoint {
    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn protocol(&self) -> RpcProtocol {
        self.protocol
    }

    pub fn streaming(&self) -> bool {
        self.streaming
    }

    pub(crate) fn new(url: Url, protocol: RpcProtocol, streaming: bool) -> Self {
        Self {
            url,
            protocol,
            streaming,
        }
    }
}

pub(crate) fn legacy_endpoint(base: &str, streaming: bool) -> A2AResult<ResolvedEndpoint> {
    let base = http_url(base)?;
    let suffix = if streaming { "a2a/stream" } else { "a2a" };
    Ok(ResolvedEndpoint::new(
        append_path(&base, suffix),
        RpcProtocol::Legacy,
        streaming,
    ))
}

pub(crate) fn http_url(raw: &str) -> A2AResult<Url> {
    let url = Url::parse(raw).map_err(|e| A2AError::InvalidParams(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(A2AError::InvalidParams(
            "A2A URLs require HTTP(S) and a host, with no credentials or fragment".into(),
        ));
    }
    Ok(url)
}

pub(crate) fn append_path(base: &Url, suffix: &str) -> Url {
    let mut url = base.clone();
    url.set_path(&format!("{}/{}", base.path().trim_end_matches('/'), suffix));
    url.set_query(None);
    url
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
    #[serde(skip_serializing)]
    auth_token: Option<String>,
    #[serde(skip)]
    endpoint: Option<ResolvedEndpoint>,
    #[serde(skip)]
    pinned_rpc_url: Option<String>,
    #[serde(skip, default = "Uuid::new_v4")]
    snapshot_generation: Uuid,
    #[serde(skip, default = "Uuid::new_v4")]
    invocation_generation: Uuid,
}

impl std::fmt::Debug for RegisteredAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredAgent")
            .field("card", &self.card)
            .field("trust_level", &self.trust_level)
            .field("base_url", &self.base_url)
            .field("last_seen", &self.last_seen)
            .field("health", &self.health)
            .field("endpoint", &self.endpoint)
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
        let endpoint = legacy_endpoint(&base_url, false).ok();
        Self {
            card,
            trust_level,
            base_url,
            last_seen,
            health,
            auth_token,
            endpoint,
            pinned_rpc_url: None,
            snapshot_generation: Uuid::new_v4(),
            invocation_generation: Uuid::new_v4(),
        }
    }

    pub fn endpoint(&self) -> Option<&ResolvedEndpoint> {
        self.endpoint.as_ref()
    }

    pub fn invocation_generation(&self) -> Uuid {
        self.invocation_generation
    }

    pub(crate) fn snapshot_generation(&self) -> Uuid {
        self.snapshot_generation
    }

    pub(crate) fn pinned_rpc_url(&self) -> Option<&str> {
        self.pinned_rpc_url.as_deref()
    }

    pub(crate) fn with_discovery(mut self, pinned_rpc_url: Option<String>) -> Self {
        self.endpoint = None;
        self.pinned_rpc_url = pinned_rpc_url;
        self
    }

    pub(crate) fn with_resolution(mut self, card: AgentCard, endpoint: ResolvedEndpoint) -> Self {
        if self.card.id != card.id || self.endpoint.as_ref() != Some(&endpoint) {
            self.invocation_generation = Uuid::new_v4();
        }
        self.card = card;
        self.endpoint = Some(endpoint);
        self
    }

    pub(crate) fn advance_snapshot(&mut self, invocation_changed: bool) {
        self.snapshot_generation = Uuid::new_v4();
        if invocation_changed {
            self.invocation_generation = Uuid::new_v4();
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
/// directly by `A2AClient` (see `service::card_refresh`); explicit agent name/alias
/// resolution is handled by `service::SmartRouter`.
#[async_trait::async_trait]
pub trait AgentResolver: Send + Sync {
    /// Register a remote agent, optionally attaching an outbound bearer
    /// token that subsequent RPCs to this agent will use.
    ///
    /// `auth_token` is a required parameter rather than a convenience, and
    /// that is deliberate: a tokenless twin (`register`) used to sit beside
    /// this one. It stored `None` unconditionally, so a caller who had a
    /// token and reached for the shorter name got `Ok(())` and learned only
    /// from a 401 on the next outbound RPC that the agent had been
    /// downgraded to anonymous. Deprecating it did not stop that — the
    /// warning is the same colour as every other warning — so it was
    /// removed. Pass `None` when there genuinely is no token, and the choice
    /// is then written down at the call site where it can be reviewed.
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
