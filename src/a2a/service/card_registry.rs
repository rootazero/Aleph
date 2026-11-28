use std::sync::RwLockReadGuard;

use crate::a2a::adapter::client::http_client::{
    DelegationResponse, DispatchError, PreparedDelegation,
};
use crate::a2a::adapter::client::{A2AClient, A2AClientPool};
use crate::a2a::config::A2AConfig;
use crate::a2a::domain::{A2AError, AgentCard, TrustLevel};
use crate::a2a::port::{A2AResult, AgentHealth, AgentResolver, RegisteredAgent};
use crate::security::ssrf::SsrfPolicy;
use crate::sync_primitives::{RwLock, RwLockWriteGuard};
use chrono::Utc;
use tokio::sync::watch;
use uuid::Uuid;

/// In-memory registry of known A2A agents.
///
/// Stores `RegisteredAgent` entries and implements the `AgentResolver` trait
/// for registration and lookup. Remote card fetching lives in
/// `service::card_refresh`; explicit agent name/alias resolution lives in
/// `service::SmartRouter`.
pub struct CardRegistry {
    state: RwLock<RegistryState>,
    changes: watch::Sender<u64>,
    ssrf_policy: SsrfPolicy,
}

#[derive(Default)]
struct RegistryState {
    agents: Vec<RegisteredAgent>,
    closed: bool,
    revision: u64,
}

impl Default for CardRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CardRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::with_ssrf_policy(SsrfPolicy::default())
    }

    pub fn with_ssrf_policy(ssrf_policy: SsrfPolicy) -> Self {
        Self {
            state: RwLock::new(RegistryState::default()),
            changes: watch::channel(0).0,
            ssrf_policy,
        }
    }

    /// Subscribe before reading the snapshot to observe subsequent changes.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub async fn snapshot(&self) -> Vec<RegisteredAgent> {
        self.read_state().agents.clone()
    }

    pub async fn snapshot_with_revision(&self) -> (Vec<RegisteredAgent>, u64) {
        let state = self.read_state();
        (state.agents.clone(), state.revision)
    }

    /// Check the invocation generation immediately under the registry mutation lock.
    pub fn is_current(&self, agent_id: &str, generation: Uuid) -> bool {
        self.read_state()
            .agents
            .iter()
            .any(|agent| agent.card.id == agent_id && agent.invocation_generation() == generation)
    }

    pub(crate) fn projected_health(&self, agent_id: &str, generation: Uuid) -> Option<AgentHealth> {
        self.read_state()
            .agents
            .iter()
            .find(|agent| agent.card.id == agent_id && agent.invocation_generation() == generation)
            .map(|agent| agent.health)
    }

    pub(crate) fn dispatch_target(
        &self,
        agent_id: &str,
        generation: Uuid,
    ) -> A2AResult<RegisteredAgent> {
        self.read_state()
            .agents
            .iter()
            .find(|agent| agent.card.id == agent_id && agent.invocation_generation() == generation)
            .cloned()
            .ok_or_else(stale_dispatch)
    }

    /// Hand the prepared request to its dispatch owner while the registration
    /// is still current. The HTTP task owns the request after admission.
    pub(crate) fn start_dispatch(
        &self,
        agent_id: &str,
        generation: Uuid,
        prepared: PreparedDelegation,
    ) -> A2AResult<tokio::task::JoinHandle<Result<DelegationResponse, DispatchError>>> {
        let state = self.read_state();
        if !state
            .agents
            .iter()
            .any(|agent| agent.card.id == agent_id && agent.invocation_generation() == generation)
        {
            return Err(stale_dispatch());
        }
        let dispatch = tokio::spawn(prepared.send());
        drop(state);
        Ok(dispatch)
    }

    fn read_state(&self) -> RwLockReadGuard<'_, RegistryState> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_state(&self) -> RwLockWriteGuard<'_, RegistryState> {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub async fn close(&self) {
        let mut state = self.write_state();
        if !state.closed {
            state.closed = true;
            state.agents.clear();
            self.publish_change(state);
        }
    }

    fn publish_change(&self, mut state: RwLockWriteGuard<'_, RegistryState>) {
        state.revision += 1;
        let revision = state.revision;
        drop(state);
        // The watch channel is a wakeup; a concurrent mutator can publish first.
        self.changes.send_if_modified(|published| {
            if *published >= revision {
                return false;
            }
            *published = revision;
            true
        });
    }

    pub(crate) fn discovery_client(&self, agent: &RegisteredAgent) -> A2AClient {
        A2AClientPool::client_for(agent, self.ssrf_policy.clone())
    }

    pub async fn add_remote(&self, url: &str, token: Option<String>) -> A2AResult<AgentCard> {
        let mut agent = RegisteredAgent::new(
            placeholder_card(String::new(), String::new()),
            TrustLevel::infer_from_url(url),
            url.to_owned(),
            Utc::now(),
            AgentHealth::Healthy,
            token,
        )
        .with_discovery(None);
        let client = self.discovery_client(&agent);
        let (mut card, endpoint) = client.discover(None).await?;
        if card.id.is_empty() {
            card.id = slug_from_name(&card.name);
        }
        if card.id.is_empty() {
            return Err(A2AError::InvalidParams(
                "A2A agent card has no usable name or ID".into(),
            ));
        }
        agent = agent.with_resolution(card.clone(), endpoint);
        if !self.upsert(agent).await {
            return Err(A2AError::AgentUnreachable("A2A registry is closed".into()));
        }
        Ok(card)
    }

    /// Load agents from config (called at startup).
    ///
    /// Each `A2AAgentEntry` is converted to a `RegisteredAgent` with a
    /// placeholder `AgentCard`. The card refresh worker resolves its endpoint.
    pub async fn load_from_config(&self, config: &A2AConfig) {
        let mut configured = Vec::new();
        let mut seen_slugs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for entry in &config.agents {
            let trust_level = entry
                .trust_level
                .as_deref()
                .and_then(|s| serde_json::from_str::<TrustLevel>(&format!("\"{s}\"")).ok())
                .unwrap_or_else(|| TrustLevel::infer_from_url(&entry.url));

            let slug = slug_from_name(&entry.name);
            if !seen_slugs.insert(slug.clone()) {
                tracing::warn!(
                    name = %entry.name,
                    slug = %slug,
                    "A2A config agent skipped: duplicate slug ID"
                );
                continue;
            }

            let card = placeholder_card(slug, entry.name.clone());

            let agent = RegisteredAgent::new(
                card,
                trust_level,
                entry.url.clone(),
                Utc::now(),
                AgentHealth::Healthy,
                entry.token.clone(),
            )
            .with_discovery(entry.rpc_url.clone());

            configured.push(agent);
        }
        if configured.is_empty() {
            return;
        }
        let mut state = self.write_state();
        if state.closed {
            return;
        }
        for agent in configured {
            state
                .agents
                .retain(|a| a.card.id != agent.card.id && a.base_url != agent.base_url);
            state.agents.push(agent);
        }
        self.publish_change(state);
    }

    /// Insert or replace a fully-formed remote agent entry.
    ///
    /// Differs from [`AgentResolver::register_with_token`] in what counts as
    /// "the same agent": that one evicts by card id, this one by card id *or*
    /// base URL. So re-adding a config-declared agent replaces the
    /// placeholder card that was registered against its URL, rather than
    /// leaving two entries pointed at one endpoint.
    pub async fn upsert(&self, mut agent: RegisteredAgent) -> bool {
        let mut state = self.write_state();
        if state.closed {
            return false;
        }
        agent.advance_snapshot(true);
        state
            .agents
            .retain(|a| a.card.id != agent.card.id && a.base_url != agent.base_url);
        state.agents.push(agent);
        self.publish_change(state);
        true
    }

    /// Apply a worker result only while its source record is still current.
    pub(crate) async fn replace_if_current(
        &self,
        source: &RegisteredAgent,
        mut updated: RegisteredAgent,
    ) -> bool {
        let mut state = self.write_state();
        let Some(index) = state
            .agents
            .iter()
            .position(|agent| agent.snapshot_generation() == source.snapshot_generation())
        else {
            return false;
        };
        if state
            .agents
            .iter()
            .enumerate()
            .any(|(i, agent)| i != index && agent.card.id == updated.card.id)
        {
            return false;
        }
        updated.advance_snapshot(false);
        state.agents[index] = updated;
        self.publish_change(state);
        true
    }

    pub(crate) async fn record_health_if_current(
        &self,
        source: &RegisteredAgent,
        health: AgentHealth,
    ) {
        let mut state = self.write_state();
        if let Some(agent) = state.agents.iter_mut().find(|agent| {
            agent.snapshot_generation() == source.snapshot_generation()
                && agent.last_seen == source.last_seen
        }) {
            let changed = agent.health != health;
            agent.health = health;
            agent.last_seen = Utc::now();
            if changed {
                self.publish_change(state);
            }
        }
    }
}

fn stale_dispatch() -> A2AError {
    A2AError::AgentUnreachable("A2A registration changed before dispatch".into())
}

fn placeholder_card(id: String, name: String) -> AgentCard {
    AgentCard {
        id,
        name,
        version: "unknown".into(),
        description: None,
        provider: None,
        documentation_url: None,
        interfaces: vec![],
        skills: vec![],
        security: vec![],
        extensions: vec![],
        default_input_modes: vec!["text".into()],
        default_output_modes: vec!["text".into()],
    }
}

/// Convert a human-readable name to a URL-safe slug.
fn slug_from_name(name: &str) -> String {
    let raw: String = name
        .to_lowercase()
        .replace(|c: char| !c.is_alphanumeric(), "-");
    // Collapse consecutive hyphens and trim leading/trailing hyphens
    let mut result = String::with_capacity(raw.len());
    let mut prev_hyphen = true; // treat start as hyphen to trim leading
    for c in raw.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push('-');
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }
    // Trim trailing hyphen
    if result.ends_with('-') {
        result.pop();
    }
    result
}

#[async_trait::async_trait]
impl AgentResolver for CardRegistry {
    async fn register_with_token(
        &self,
        card: AgentCard,
        base_url: &str,
        trust_level: TrustLevel,
        auth_token: Option<String>,
    ) -> A2AResult<()> {
        let mut state = self.write_state();
        if state.closed {
            return Err(A2AError::AgentUnreachable("A2A registry is closed".into()));
        }
        // Remove existing with same ID (upsert semantics)
        state.agents.retain(|a| a.card.id != card.id);
        state.agents.push(RegisteredAgent::new(
            card,
            trust_level,
            base_url.to_string(),
            Utc::now(),
            AgentHealth::Healthy,
            auth_token,
        ));
        self.publish_change(state);
        Ok(())
    }

    async fn unregister(&self, agent_id: &str) -> A2AResult<()> {
        let mut state = self.write_state();
        let before = state.agents.len();
        state.agents.retain(|a| a.card.id != agent_id);
        if state.agents.len() == before {
            return Err(A2AError::InvalidParams(format!(
                "Agent not found: {agent_id}"
            )));
        }
        self.publish_change(state);
        Ok(())
    }

    async fn list_agents(&self) -> A2AResult<Vec<RegisteredAgent>> {
        Ok(self.snapshot().await)
    }

    async fn resolve_by_id(&self, agent_id: &str) -> A2AResult<Option<RegisteredAgent>> {
        let state = self.read_state();
        Ok(state.agents.iter().find(|a| a.card.id == agent_id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_card(id: &str, name: &str) -> AgentCard {
        AgentCard {
            id: id.to_string(),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: Some("Test agent".to_string()),
            provider: None,
            documentation_url: None,
            interfaces: vec![],
            skills: vec![],
            security: vec![],
            extensions: vec![],
            default_input_modes: vec!["text".to_string()],
            default_output_modes: vec!["text".to_string()],
        }
    }

    #[tokio::test]
    async fn health_changes_publish_without_revoking_dispatch_generation() {
        let registry = CardRegistry::new();
        let mut changes = registry.subscribe();
        registry
            .register_with_token(
                sample_card("peer", "Peer"),
                "https://example.org",
                TrustLevel::Trusted,
                None,
            )
            .await
            .unwrap();
        let (agents, register_revision) = registry.snapshot_with_revision().await;
        let peer = &agents[0];
        let generation = peer.invocation_generation();
        assert_eq!(*changes.borrow_and_update(), register_revision);

        registry
            .record_health_if_current(peer, AgentHealth::Unreachable)
            .await;
        let (agents, health_revision) = registry.snapshot_with_revision().await;
        assert_eq!(agents[0].health, AgentHealth::Unreachable);
        assert_eq!(agents[0].invocation_generation(), generation);
        assert!(registry.is_current("peer", generation));
        assert!(health_revision > register_revision);
        assert_eq!(*changes.borrow_and_update(), health_revision);

        registry.close().await;
        assert!(!registry.is_current("peer", generation));
        assert!(registry.snapshot().await.is_empty());
        assert!(*changes.borrow_and_update() > health_revision);
    }

    #[tokio::test]
    async fn register_and_list_agents() {
        let registry = CardRegistry::new();
        let card = sample_card("agent-1", "Agent One");

        registry
            .register_with_token(card, "http://localhost:9000", TrustLevel::Local, None)
            .await
            .unwrap();

        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].card.id, "agent-1");
        assert_eq!(agents[0].base_url, "http://localhost:9000");
        assert_eq!(agents[0].trust_level, TrustLevel::Local);
    }

    #[tokio::test]
    async fn register_replaces_existing_agent() {
        let registry = CardRegistry::new();

        let card_v1 = sample_card("agent-1", "Agent v1");
        registry
            .register_with_token(card_v1, "http://localhost:9000", TrustLevel::Local, None)
            .await
            .unwrap();

        let card_v2 = sample_card("agent-1", "Agent v2");
        registry
            .register_with_token(card_v2, "http://localhost:9001", TrustLevel::Trusted, None)
            .await
            .unwrap();

        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].card.name, "Agent v2");
        assert_eq!(agents[0].base_url, "http://localhost:9001");
        assert_eq!(agents[0].trust_level, TrustLevel::Trusted);
    }

    #[tokio::test]
    async fn unregister_removes_agent() {
        let registry = CardRegistry::new();
        let card = sample_card("agent-1", "Agent One");
        registry
            .register_with_token(card, "http://localhost:9000", TrustLevel::Local, None)
            .await
            .unwrap();

        registry.unregister("agent-1").await.unwrap();

        let agents = registry.list_agents().await.unwrap();
        assert!(agents.is_empty());
    }

    #[tokio::test]
    async fn unregister_nonexistent_returns_error() {
        let registry = CardRegistry::new();
        let result = registry.unregister("ghost").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resolve_by_id_finds_agent() {
        let registry = CardRegistry::new();
        let card = sample_card("agent-1", "Agent One");
        registry
            .register_with_token(card, "http://localhost:9000", TrustLevel::Local, None)
            .await
            .unwrap();

        let agent = registry.resolve_by_id("agent-1").await.unwrap();
        assert!(agent.is_some());
        assert_eq!(agent.unwrap().card.name, "Agent One");
    }

    #[tokio::test]
    async fn resolve_by_id_returns_none_for_unknown() {
        let registry = CardRegistry::new();
        let agent = registry.resolve_by_id("nonexistent").await.unwrap();
        assert!(agent.is_none());
    }

    #[tokio::test]
    async fn load_from_config() {
        use crate::a2a::config::{A2AAgentEntry, A2AConfig};

        let config = A2AConfig {
            enabled: true,
            agents: vec![
                A2AAgentEntry {
                    name: "Local Helper".to_string(),
                    url: "http://localhost:9000".to_string(),
                    rpc_url: None,
                    trust_level: None,
                    token: None,
                },
                A2AAgentEntry {
                    name: "Remote Service".to_string(),
                    url: "https://api.example.com/a2a".to_string(),
                    rpc_url: None,
                    trust_level: Some("public".to_string()),
                    token: Some("tok-123".to_string()),
                },
            ],
            ..Default::default()
        };

        let registry = CardRegistry::new();
        registry.load_from_config(&config).await;

        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 2);

        // First agent: trust inferred from localhost URL
        assert_eq!(agents[0].card.name, "Local Helper");
        assert_eq!(agents[0].trust_level, TrustLevel::Local);

        // Second agent: trust explicitly set to public
        assert_eq!(agents[1].card.name, "Remote Service");
        assert_eq!(agents[1].trust_level, TrustLevel::Public);
    }

    #[test]
    fn slug_from_name_converts_correctly() {
        assert_eq!(slug_from_name("My Agent"), "my-agent");
        assert_eq!(slug_from_name("code_review-v2"), "code-review-v2");
        assert_eq!(slug_from_name("Hello World!"), "hello-world");
        assert_eq!(slug_from_name("  Hello  World  "), "hello-world");
        assert_eq!(slug_from_name("!!!test!!!"), "test");
    }

    #[tokio::test]
    async fn upsert_preserves_auth_token_and_replaces_by_url() {
        let registry = CardRegistry::new();
        registry
            .upsert(RegisteredAgent::new(
                sample_card("real-id", "Helper"),
                TrustLevel::Trusted,
                "https://h.example.com".to_string(),
                Utc::now(),
                AgentHealth::Healthy,
                Some("tok-abc".to_string()),
            ))
            .await;

        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].auth_token(), Some("tok-abc"));

        // Re-upsert at the same base URL with a different card id replaces in
        // place (no duplicate) — the placeholder-vs-real-id case.
        registry
            .upsert(RegisteredAgent::new(
                sample_card("new-id", "Helper v2"),
                TrustLevel::Trusted,
                "https://h.example.com".to_string(),
                Utc::now(),
                AgentHealth::Healthy,
                Some("tok-xyz".to_string()),
            ))
            .await;
        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1, "same base_url must replace, not duplicate");
        assert_eq!(agents[0].card.id, "new-id");
        assert_eq!(agents[0].auth_token(), Some("tok-xyz"));
    }
}
