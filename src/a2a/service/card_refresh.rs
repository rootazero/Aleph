//! One-shot startup refresh of placeholder Agent Cards.
//!
//! `CardRegistry::load_from_config` seeds config-declared agents with
//! placeholder cards (no skills/description, `version = "unknown"`). This
//! module fetches each agent's real Agent Card once at startup and upserts it,
//! so smart routing and `a2a_agents list` see real skill data.

use crate::sync_primitives::Arc;

use super::card_registry::CardRegistry;
use crate::a2a::adapter::client::A2AClient;
use crate::a2a::port::{AgentHealth, AgentResolver, RegisteredAgent};
use crate::a2a::sub_agent::A2ASubAgent;

/// Fetch the real Agent Card for every registered agent and upsert it.
///
/// Agents whose card cannot be fetched keep their placeholder entry (still
/// routable by name). Returns the number of cards successfully refreshed.
pub async fn refresh_all_cards(registry: &CardRegistry) -> usize {
    let agents = match registry.list_agents().await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "A2A card refresh: failed to list agents");
            return 0;
        }
    };

    let mut refreshed = 0usize;
    for agent in agents {
        let client = match &agent.auth_token {
            Some(token) => A2AClient::with_auth(&agent.base_url, token),
            None => A2AClient::new(&agent.base_url),
        };
        match client.fetch_agent_card().await {
            Ok(card) => {
                registry
                    .upsert(RegisteredAgent {
                        card,
                        trust_level: agent.trust_level,
                        base_url: agent.base_url.clone(),
                        last_seen: chrono::Utc::now(),
                        health: AgentHealth::Healthy,
                        auth_token: agent.auth_token.clone(),
                    })
                    .await;
                refreshed += 1;
            }
            Err(e) => {
                tracing::warn!(
                    agent = %agent.card.name,
                    url = %agent.base_url,
                    error = %e,
                    "A2A card refresh: keeping placeholder card"
                );
            }
        }
    }
    refreshed
}

/// Spawn a background task that runs one card-refresh pass at startup.
///
/// After the pass, refreshes the `A2ASubAgent` name cache so `can_handle`
/// matches newly-discovered skill names and aliases. Non-blocking.
pub fn spawn_card_refresh(registry: Arc<CardRegistry>, sub_agent: Arc<A2ASubAgent>) {
    tokio::spawn(async move {
        let n = refresh_all_cards(&registry).await;
        if n > 0 {
            sub_agent.refresh_agent_names().await;
        }
        tracing::info!(refreshed = n, "A2A startup card refresh complete");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::config::{A2AAgentEntry, A2AConfig};
    use crate::a2a::domain::AgentCard;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config_with_agent(name: &str, url: &str) -> A2AConfig {
        A2AConfig {
            enabled: true,
            agents: vec![A2AAgentEntry {
                name: name.to_string(),
                url: url.to_string(),
                trust_level: None,
                token: None,
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn refresh_empty_registry_returns_zero() {
        let registry = CardRegistry::new();
        assert_eq!(refresh_all_cards(&registry).await, 0);
    }

    #[tokio::test]
    async fn refresh_unreachable_keeps_placeholder() {
        let registry = CardRegistry::new();
        // 127.0.0.1:1 — nothing listens there.
        registry
            .load_from_config(&config_with_agent("Ghost", "http://127.0.0.1:1"))
            .await;

        assert_eq!(refresh_all_cards(&registry).await, 0);

        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].card.version, "unknown");
    }

    #[tokio::test]
    async fn refresh_success_replaces_placeholder() {
        let server = MockServer::start().await;
        let real_card = AgentCard {
            id: "real-helper".to_string(),
            name: "Helper".to_string(),
            version: "2.3.1".to_string(),
            description: Some("Real card".to_string()),
            provider: None,
            documentation_url: None,
            interfaces: vec![],
            skills: vec![],
            security: vec![],
            extensions: vec![],
            default_input_modes: vec!["text".to_string()],
            default_output_modes: vec!["text".to_string()],
        };
        Mock::given(method("GET"))
            .and(path("/.well-known/agent-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&real_card))
            .mount(&server)
            .await;

        let registry = CardRegistry::new();
        registry
            .load_from_config(&config_with_agent("Helper", &server.uri()))
            .await;
        // Placeholder before refresh.
        assert_eq!(
            registry.list_agents().await.unwrap()[0].card.version,
            "unknown"
        );

        let n = refresh_all_cards(&registry).await;
        assert_eq!(n, 1);

        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].card.version, "2.3.1");
        assert_eq!(agents[0].card.description.as_deref(), Some("Real card"));
    }
}
