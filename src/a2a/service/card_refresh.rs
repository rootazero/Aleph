//! One-shot startup refresh of placeholder Agent Cards, plus an optional
//! periodic agent health monitor.
//!
//! `CardRegistry::load_from_config` seeds config-declared agents with
//! placeholder cards (no skills/description, `version = "unknown"`). This
//! module fetches each agent's real Agent Card once at startup and upserts it,
//! so explicit agent name/alias resolution and `a2a_agents list` see current
//! Agent Card data.
//!
//! The health monitor (opt-in via `a2a.health_check_interval_secs > 0`) keeps
//! each agent's `health` field current by probing reachability on an interval.

use std::time::Duration;

use crate::sync_primitives::Arc;

use super::card_registry::CardRegistry;
use crate::a2a::adapter::client::A2AClientPool;
use crate::a2a::port::AgentHealth;

/// Discover each registered card and replace the record if it is still current.
pub async fn refresh_all_cards(registry: &CardRegistry) -> usize {
    let mut refreshed = 0;
    for agent in registry.snapshot().await {
        let client = registry.discovery_client(&agent);
        let fallback = (agent.card.version == "unknown").then_some(&agent.card);
        match client.discover(fallback).await {
            Ok((card, endpoint)) => {
                let mut updated = agent.clone().with_resolution(card, endpoint);
                updated.health = AgentHealth::Healthy;
                updated.last_seen = chrono::Utc::now();
                if registry.replace_if_current(&agent, updated).await {
                    refreshed += 1;
                }
            }
            Err(error) => {
                tracing::warn!(agent = %agent.card.name, %error, "A2A card refresh failed");
                registry
                    .record_health_if_current(&agent, AgentHealth::Unreachable)
                    .await;
            }
        }
    }
    refreshed
}

/// Spawn a background task that runs one card-refresh pass at startup.
///
/// Non-blocking — never delays startup.
pub fn spawn_card_refresh(registry: Arc<CardRegistry>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let n = refresh_all_cards(&registry).await;
        tracing::info!(refreshed = n, "A2A startup card refresh complete");
    })
}

/// Probe the selected record and apply liveness to the same registration.
pub async fn health_check_pass(registry: &CardRegistry, pool: &A2AClientPool) -> usize {
    let agents = registry.snapshot().await;
    for agent in &agents {
        let health = pool.health_check(agent).await;
        registry.record_health_if_current(agent, health).await;
    }
    agents.len()
}

/// Spawn the periodic agent health monitor.
///
/// Probes all registered agents every `interval` and updates their health.
/// The caller is responsible for only invoking this when the configured
/// interval is non-zero and owns the returned worker.
pub fn spawn_health_monitor(
    registry: Arc<CardRegistry>,
    pool: Arc<A2AClientPool>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Skip the immediate first tick — startup card refresh already probed.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let n = health_check_pass(&registry, &pool).await;
            tracing::debug!(probed = n, "A2A agent health monitor pass complete");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::config::{A2AAgentEntry, A2AConfig};
    use crate::a2a::domain::AgentCard;
    use crate::a2a::port::AgentResolver;
    use crate::security::ssrf::SsrfPolicy;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config_with_agent(name: &str, url: &str) -> A2AConfig {
        A2AConfig {
            enabled: true,
            agents: vec![A2AAgentEntry {
                name: name.to_string(),
                url: url.to_string(),
                rpc_url: None,
                trust_level: None,
                token: None,
            }],
            ..Default::default()
        }
    }

    fn test_policy() -> SsrfPolicy {
        SsrfPolicy {
            enabled: false,
            ..SsrfPolicy::default()
        }
    }

    #[tokio::test]
    async fn refresh_empty_registry_returns_zero() {
        let registry = CardRegistry::with_ssrf_policy(test_policy());
        assert_eq!(refresh_all_cards(&registry).await, 0);
    }

    #[tokio::test]
    async fn refresh_unreachable_keeps_placeholder() {
        let registry = CardRegistry::with_ssrf_policy(test_policy());
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

        let registry = CardRegistry::with_ssrf_policy(test_policy());
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

    #[tokio::test]
    async fn refresh_response_cannot_overwrite_a_removed_or_replaced_registration() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::oneshot;

        for replace in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (requested, request_seen) = oneshot::channel();
            let (respond, response_allowed) = oneshot::channel();
            let remote = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 1024];
                assert!(stream.read(&mut request).await.unwrap() > 0);
                requested.send(()).unwrap();
                response_allowed.await.unwrap();
                let card = serde_json::json!({
                    "id": "stale-card", "name": "Stale", "version": "1",
                    "interfaces": [], "skills": [], "security": [], "extensions": [],
                    "defaultInputModes": [], "defaultOutputModes": []
                })
                .to_string();
                stream.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{card}",
                    card.len()
                ).as_bytes()).await.unwrap();
            });
            let registry = Arc::new(CardRegistry::with_ssrf_policy(test_policy()));
            registry
                .load_from_config(&config_with_agent("Helper", &url))
                .await;
            let refresh = tokio::spawn({
                let registry = registry.clone();
                async move { refresh_all_cards(&registry).await }
            });
            tokio::time::timeout(Duration::from_secs(5), request_seen)
                .await
                .unwrap()
                .unwrap();
            if replace {
                registry
                    .load_from_config(&config_with_agent("Replacement", &url))
                    .await;
            } else {
                registry.unregister("helper").await.unwrap();
            }
            respond.send(()).unwrap();
            assert_eq!(refresh.await.unwrap(), 0);
            remote.await.unwrap();
            let ids: Vec<_> = registry
                .snapshot()
                .await
                .into_iter()
                .map(|entry| entry.card.id)
                .collect();
            if replace {
                assert_eq!(ids, ["replacement"]);
            } else {
                assert!(ids.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn health_pass_marks_reachable_agent_healthy() {
        let server = MockServer::start().await;
        let card = AgentCard {
            id: "live-helper".to_string(),
            name: "Helper".to_string(),
            version: "1.0.0".to_string(),
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
        Mock::given(method("GET"))
            .and(path("/.well-known/agent-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&card))
            .mount(&server)
            .await;

        let registry = CardRegistry::with_ssrf_policy(test_policy());
        registry
            .load_from_config(&config_with_agent("Helper", &server.uri()))
            .await;
        let pool = A2AClientPool::with_ssrf_policy(test_policy());

        assert_eq!(health_check_pass(&registry, &pool).await, 1);

        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].health, AgentHealth::Healthy);
    }

    #[tokio::test]
    async fn health_pass_marks_unreachable_agent_unreachable() {
        let registry = CardRegistry::with_ssrf_policy(test_policy());
        // 127.0.0.1:1 — nothing listens there.
        registry
            .load_from_config(&config_with_agent("Ghost", "http://127.0.0.1:1"))
            .await;
        let pool = A2AClientPool::with_ssrf_policy(test_policy());

        assert_eq!(health_check_pass(&registry, &pool).await, 1);

        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].health, AgentHealth::Unreachable);
    }
}
