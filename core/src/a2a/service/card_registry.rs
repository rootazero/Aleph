use chrono::Utc;
use tokio::sync::RwLock;

use crate::a2a::config::A2AConfig;
use crate::a2a::domain::*;
use crate::a2a::port::*;

/// In-memory registry of known A2A agents.
///
/// Stores `RegisteredAgent` entries and implements the `AgentResolver` trait
/// for lookup operations. Network-dependent methods (`fetch_card`, `resolve_by_intent`)
/// return stub results — they'll be properly wired when the A2AClient and
/// SmartRouter are integrated.
pub struct CardRegistry {
    agents: RwLock<Vec<RegisteredAgent>>,
}

impl Default for CardRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CardRegistry {
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(Vec::new()),
        }
    }

    /// Load agents from config (called at startup).
    ///
    /// Each `A2AAgentEntry` is converted to a `RegisteredAgent` with a
    /// placeholder `AgentCard`. The real card will be fetched lazily via
    /// `fetch_card` once the HTTP client is wired.
    pub async fn load_from_config(&self, config: &A2AConfig) {
        let mut agents = self.agents.write().await;
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

            let card = AgentCard {
                id: slug_from_name(&entry.name),
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

            let agent = RegisteredAgent {
                card,
                trust_level,
                base_url: entry.url.clone(),
                last_seen: Utc::now(),
                health: AgentHealth::Healthy,
            };

            agents.push(agent);
        }
    }
}

/// Convert a human-readable name to a URL-safe slug.
fn slug_from_name(name: &str) -> String {
    name.to_lowercase()
        .replace(|c: char| !c.is_alphanumeric(), "-")
}

#[async_trait::async_trait]
impl AgentResolver for CardRegistry {
    async fn fetch_card(&self, _url: &str) -> A2AResult<AgentCard> {
        // Requires HTTP client — will be properly wired when A2AClient is integrated
        Err(A2AError::InternalError(
            "fetch_card requires HTTP client injection".into(),
        ))
    }

    async fn register(
        &self,
        card: AgentCard,
        base_url: &str,
        trust_level: TrustLevel,
    ) -> A2AResult<()> {
        let mut agents = self.agents.write().await;
        // Remove existing with same ID (upsert semantics)
        agents.retain(|a| a.card.id != card.id);
        agents.push(RegisteredAgent {
            card,
            trust_level,
            base_url: base_url.to_string(),
            last_seen: Utc::now(),
            health: AgentHealth::Healthy,
        });
        Ok(())
    }

    async fn unregister(&self, agent_id: &str) -> A2AResult<()> {
        let mut agents = self.agents.write().await;
        let before = agents.len();
        agents.retain(|a| a.card.id != agent_id);
        if agents.len() == before {
            return Err(A2AError::TaskNotFound(format!(
                "Agent not found: {}",
                agent_id
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

    async fn resolve_by_intent(&self, _intent: &str) -> A2AResult<Option<RegisteredAgent>> {
        // LLM-based routing will be implemented in SmartRouter (Task 12).
        // CardRegistry only provides data storage.
        Ok(None)
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
        assert_eq!(slug_from_name("Hello World!"), "hello-world-");
    }
}
