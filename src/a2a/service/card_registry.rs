use crate::a2a::config::A2AConfig;
use crate::a2a::domain::{A2AError, AgentCard, TrustLevel};
use crate::a2a::port::{A2AResult, AgentHealth, AgentResolver, RegisteredAgent};
use crate::sync_primitives::AsyncRwLock;
use chrono::Utc;

/// In-memory registry of known A2A agents.
///
/// Stores `RegisteredAgent` entries and implements the `AgentResolver` trait
/// for registration and lookup. Remote card fetching lives in
/// `service::card_refresh`; intent routing lives in `service::SmartRouter`.
pub struct CardRegistry {
    agents: AsyncRwLock<Vec<RegisteredAgent>>,
}

impl Default for CardRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CardRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            agents: AsyncRwLock::new(Vec::new()),
        }
    }

    /// Load agents from config (called at startup).
    ///
    /// Each `A2AAgentEntry` is converted to a `RegisteredAgent` with a
    /// placeholder `AgentCard`. The real card will be fetched lazily via
    /// `fetch_card` once the HTTP client is wired.
    pub async fn load_from_config(&self, config: &A2AConfig) {
        let mut agents = self.agents.write().await;
        let mut seen_slugs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for entry in &config.agents {
            let trust_level = entry
                .trust_level
                .as_deref()
                .and_then(|s| match s {
                    "local" => Some(TrustLevel::Local),
                    "trusted" => Some(TrustLevel::Trusted),
                    "public" => Some(TrustLevel::Public),
                    _ => None,
                })
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

            let card = AgentCard {
                id: slug,
                name: entry.name.clone(),
                version: "unknown".to_string(),
                description: None,
                provider: None,
                documentation_url: None,
                interfaces: vec![],
                skills: vec![],
                security: vec![],
                extensions: vec![],
                default_input_modes: vec!["text".to_string()],
                default_output_modes: vec!["text".to_string()],
            };

            let agent = RegisteredAgent::new(
                card,
                trust_level,
                entry.url.clone(),
                Utc::now(),
                AgentHealth::Healthy,
                entry.token.clone(),
            );

            // Upsert: remove existing with same ID to avoid duplicates on config reload
            agents.retain(|a| a.card.id != agent.card.id);
            agents.push(agent);
        }
    }

    /// Insert or replace a fully-formed remote agent entry.
    ///
    /// Unlike [`AgentResolver::register`], this preserves the agent's
    /// `auth_token` (the trait method cannot carry one). Existing entries with
    /// the same card id *or* base URL are removed first, so re-adding a
    /// config-declared agent by URL cleanly replaces its placeholder card.
    pub async fn upsert(&self, agent: RegisteredAgent) {
        let mut agents = self.agents.write().await;
        agents.retain(|a| a.card.id != agent.card.id && a.base_url != agent.base_url);
        agents.push(agent);
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
    #[allow(deprecated)]
    async fn register(
        &self,
        card: AgentCard,
        base_url: &str,
        trust_level: TrustLevel,
    ) -> A2AResult<()> {
        // Deprecated: always passes `None` for the auth token. New callers
        // should use `register_with_token` so an authenticated agent does not
        // silently downgrade to anonymous outbound RPC.
        self.register_with_token(card, base_url, trust_level, None).await
    }

    async fn register_with_token(
        &self,
        card: AgentCard,
        base_url: &str,
        trust_level: TrustLevel,
        auth_token: Option<String>,
    ) -> A2AResult<()> {
        let mut agents = self.agents.write().await;
        // Remove existing with same ID (upsert semantics)
        agents.retain(|a| a.card.id != card.id);
        agents.push(RegisteredAgent::new(
            card,
            trust_level,
            base_url.to_string(),
            Utc::now(),
            AgentHealth::Healthy,
            auth_token,
        ));
        Ok(())
    }

    async fn unregister(&self, agent_id: &str) -> A2AResult<()> {
        let mut agents = self.agents.write().await;
        let before = agents.len();
        agents.retain(|a| a.card.id != agent_id);
        if agents.len() == before {
            return Err(A2AError::InvalidParams(format!(
                "Agent not found: {agent_id}"
            )));
        }
        Ok(())
    }

    async fn list_agents(&self) -> A2AResult<Vec<RegisteredAgent>> {
        let agents = self.agents.read().await;
        Ok(agents.clone())
    }

    async fn resolve_by_id(&self, agent_id: &str) -> A2AResult<Option<RegisteredAgent>> {
        let agents = self.agents.read().await;
        Ok(agents.iter().find(|a| a.card.id == agent_id).cloned())
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
    async fn register_and_list_agents() {
        let registry = CardRegistry::new();
        let card = sample_card("agent-1", "Agent One");

        registry
            .register(card, "http://localhost:9000", TrustLevel::Local)
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
            .register(card_v1, "http://localhost:9000", TrustLevel::Local)
            .await
            .unwrap();

        let card_v2 = sample_card("agent-1", "Agent v2");
        registry
            .register(card_v2, "http://localhost:9001", TrustLevel::Trusted)
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
            .register(card, "http://localhost:9000", TrustLevel::Local)
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
            .register(card, "http://localhost:9000", TrustLevel::Local)
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
                    trust_level: None,
                    token: None,
                },
                A2AAgentEntry {
                    name: "Remote Service".to_string(),
                    url: "https://api.example.com/a2a".to_string(),
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
