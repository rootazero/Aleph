//! Agent Router
//!
//! Routes incoming requests to the appropriate agent based on session key,
//! channel, or peer information.
//!
//! The binding-matching tier delegates to [`crate::routing::resolve_route`] —
//! the SAME routing grammar the inbound message path uses — so the JSON-RPC
//! seam (`agent.run` / `chat.send`) and the inbound path can no longer
//! disagree on the same `[[bindings]]`. The seam's load-bearing behaviors
//! (caller-supplied session key carry-through, explicit agent override, epoch
//! bump) are orthogonal and stay here.

use crate::routing::config::RouteBinding;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::warn;

use super::session_store::SessionStore;

// Re-export unified SessionKey from routing module.
pub use crate::routing::SessionKey;
pub use crate::routing::{DmScope, PeerKind};

/// Agent router for directing requests to appropriate agents
///
/// Routes requests based on:
/// 1. Explicit session key
/// 2. Peer/channel matching
/// 3. Default agent fallback
pub struct AgentRouter {
    /// Original config `[[bindings]]` table, consumed by `resolve_route`.
    bindings: Arc<RwLock<Vec<RouteBinding>>>,
    /// Session config fed to `resolve_route` (DM scope for key shaping).
    session_config: crate::routing::SessionConfig,
    /// Default agent ID
    default_agent: String,
    /// Available agent IDs
    agents: Arc<RwLock<Vec<String>>>,
    /// Optional session store for epoch resolution
    session_store: Option<Arc<dyn SessionStore>>,
}

impl AgentRouter {
    /// Create a new router with default "main" agent
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: Arc::new(RwLock::new(Vec::new())),
            session_config: crate::routing::SessionConfig::default(),
            default_agent: "main".to_string(),
            agents: Arc::new(RwLock::new(vec!["main".to_string()])),
            session_store: None,
        }
    }

    /// Create a router with custom default agent
    pub fn with_default(default_agent: impl Into<String>) -> Self {
        let default = default_agent.into();
        Self {
            bindings: Arc::new(RwLock::new(Vec::new())),
            session_config: crate::routing::SessionConfig::default(),
            default_agent: default.clone(),
            agents: Arc::new(RwLock::new(vec![default])),
            session_store: None,
        }
    }

    /// Set session store for epoch resolution
    pub fn set_session_store(&mut self, sm: Arc<dyn SessionStore>) {
        self.session_store = Some(sm);
    }

    /// Create router from config-driven `RouteBinding` list.
    /// Extracts unique agent IDs and keeps the original binding table for
    /// `resolve_route` — no lossy pattern flattening (the old conversion
    /// silently dropped peer/guild/team/account/workspace scoping).
    pub fn from_bindings(bindings: Vec<RouteBinding>, default_agent: impl Into<String>) -> Self {
        let default = default_agent.into();

        // Extract unique agent IDs (HashSet for O(n) dedup, then collect)
        let mut agent_ids: std::collections::HashSet<String> =
            std::collections::HashSet::from([default.clone()]);
        for b in &bindings {
            agent_ids.insert(b.agent_id.clone());
        }
        let agent_ids: Vec<String> = agent_ids.into_iter().collect();

        Self {
            bindings: Arc::new(RwLock::new(bindings)),
            session_config: crate::routing::SessionConfig::default(),
            default_agent: default,
            agents: Arc::new(RwLock::new(agent_ids)),
            session_store: None,
        }
    }

    /// Add a channel routing binding
    pub async fn add_binding(&self, channel: impl Into<String>, agent_id: impl Into<String>) {
        let binding = RouteBinding {
            agent_id: agent_id.into(),
            match_rule: crate::routing::MatchRule {
                channel: Some(channel.into()),
                ..Default::default()
            },
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
        agent_id: Option<&str>,
    ) -> SessionKey {
        // 1. If explicit session key provided, parse and use it
        if let Some(key_str) = session_key {
            if let Some(key) = SessionKey::from_key_string(key_str) {
                return key;
            }
        }

        // 2. If explicit agent_id provided (e.g., from panel UI), use it directly
        let base_key = if let Some(aid) = agent_id {
            match peer_id {
                Some(pid) => SessionKey::peer(aid, pid),
                None => SessionKey::main(aid),
            }
        } else {
            // 3. Try to match channel/peer against bindings (via the same
            //    grammar the inbound path uses)
            let resolved_agent = self.resolve_agent(channel, peer_id).await;

            // 4. Create appropriate session key
            match peer_id {
                Some(pid) => SessionKey::peer(&resolved_agent, pid),
                None => SessionKey::main(&resolved_agent),
            }
        };

        // 5. No explicit session_key → create a new session (next epoch).
        //    The session is only persisted to DB when the first message is sent.
        //    This ensures refresh/new-chat without conversation leaves no trace.
        if let Some(ref sm) = self.session_store {
            let base_pattern = base_key.base_key_pattern();
            match sm.get_current_epoch(&base_pattern).await {
                Ok(epoch) => return base_key.with_epoch(epoch + 1),
                Err(e) => warn!("Failed to resolve epoch for {}: {}", base_pattern, e),
            }
        }

        base_key
    }

    /// Resolve agent ID from channel/peer through `resolve_route`.
    async fn resolve_agent(&self, channel: Option<&str>, peer_id: Option<&str>) -> String {
        let bindings = self.bindings.read().await;
        let input = crate::routing::RouteInput {
            channel: channel.unwrap_or_default().to_string(),
            account_id: None,
            peer: peer_id.map(|id| crate::routing::RoutePeer {
                kind: crate::routing::RoutePeerKind::Dm,
                id: id.to_string(),
            }),
            guild_id: None,
            team_id: None,
        };
        crate::routing::resolve_route(&bindings, &self.session_config, &self.default_agent, &input)
            .agent_id
    }

    /// Get the default agent ID
    #[must_use]
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
        assert!(matches!(key, SessionKey::DirectMessage { peer_id, .. } if peer_id == "window-1"));

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
        let key = router.route(None, None, None, None).await;
        assert_eq!(key.agent_id(), "main");
    }

    #[tokio::test]
    async fn test_router_explicit_key() {
        let router = AgentRouter::new();
        let key = router
            .route(Some("agent:work:main"), None, None, None)
            .await;
        assert_eq!(key.agent_id(), "work");
    }

    #[tokio::test]
    async fn test_router_binding() {
        let router = AgentRouter::new();
        router.register_agent("work").await;
        // Channel bindings are matched by `resolve_route` (exact channel), the
        // same grammar the inbound path uses — not the legacy `channel:*`
        // prefix-wildcard pattern strings.
        router.add_binding("cli", "work").await;

        let key = router.route(None, Some("cli"), None, None).await;
        assert_eq!(key.agent_id(), "work");

        // GUI should still go to default
        let key = router.route(None, Some("gui:window1"), None, None).await;
        assert_eq!(key.agent_id(), "main");
    }

    #[tokio::test]
    async fn test_router_peer_creates_peer_session() {
        let router = AgentRouter::new();
        let key = router
            .route(None, Some("gui:window1"), Some("telegram:123"), None)
            .await;

        // SessionKey::peer() runs peer_id through sanitize_component, which
        // replaces `:` (and other non-[a-z0-9_-] chars) with `-`. The test
        // asserts the sanitized form to lock that contract.
        assert!(
            matches!(key, SessionKey::DirectMessage { peer_id, .. } if peer_id == "telegram-123")
        );
    }

    #[test]
    fn test_agent_router_from_route_bindings() {
        use crate::routing::config::{MatchRule, RouteBinding};

        let bindings = vec![
            RouteBinding {
                agent_id: "coding".to_string(),
                match_rule: MatchRule {
                    channel: Some("slack".to_string()),
                    account_id: Some("*".to_string()),
                    team_id: Some("T12345".to_string()),
                    ..Default::default()
                },
            },
            RouteBinding {
                agent_id: "main".to_string(),
                match_rule: MatchRule {
                    channel: Some("telegram".to_string()),
                    account_id: Some("*".to_string()),
                    ..Default::default()
                },
            },
        ];

        let router = AgentRouter::from_bindings(bindings, "main");
        // Verify agents are registered
        let rt = tokio::runtime::Runtime::new().unwrap();
        let agents = rt.block_on(router.list_agents());
        assert!(agents.contains(&"coding".to_string()));
        assert!(agents.contains(&"main".to_string()));
    }
}
