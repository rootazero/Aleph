use crate::sync_primitives::Arc;
use std::collections::HashMap;

use super::http_client::A2AClient;
use crate::a2a::port::{A2AResult, AgentHealth, RegisteredAgent};
use crate::security::ssrf::SsrfPolicy;
use crate::sync_primitives::AsyncRwLock;

/// Connection pool managing `A2AClient` instances per agent.
///
/// Lazily creates clients on first access and caches them by agent ID.
/// Thread-safe via `AsyncRwLock` (read-heavy, write-rare pattern).
pub struct A2AClientPool {
    clients: AsyncRwLock<HashMap<String, Arc<A2AClient>>>,
    policy: SsrfPolicy,
}

impl A2AClientPool {
    #[must_use]
    pub fn new() -> Self {
        Self::with_ssrf_policy(SsrfPolicy::default())
    }

    pub fn with_ssrf_policy(policy: SsrfPolicy) -> Self {
        Self {
            clients: AsyncRwLock::new(HashMap::new()),
            policy,
        }
    }

    pub(crate) fn client_for(agent: &RegisteredAgent, policy: SsrfPolicy) -> A2AClient {
        A2AClient::for_agent(agent, policy)
    }

    /// Get or create a client for a registered agent.
    ///
    /// Uses read lock for the fast path (client exists) and only
    /// acquires a write lock when creating a new client.
    pub async fn get_or_create(&self, agent: &RegisteredAgent) -> A2AResult<Arc<A2AClient>> {
        // Fast path: read lock. Only reuse the cached client if its baked-in
        // endpoint and auth still match the requested agent — otherwise a
        // rotated token or changed `base_url` would be silently served with
        // stale credentials.
        {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(&agent.card.id) {
                if client.matches(agent) {
                    return Ok(Arc::clone(client));
                }
            }
        }

        // Slow path: create new client under write lock
        let client = Arc::new(Self::client_for(agent, self.policy.clone()));
        let mut clients = self.clients.write().await;
        // Double-check: another task may have inserted while we waited (but
        // only reuse it if it, too, matches the requested endpoint/auth).
        if let Some(existing) = clients.get(&agent.card.id) {
            if existing.matches(agent) {
                return Ok(Arc::clone(existing));
            }
        }
        clients.insert(agent.card.id.clone(), Arc::clone(&client));
        Ok(client)
    }

    /// Remove a client from the pool (e.g. after unregistering an agent)
    pub async fn remove(&self, agent_id: &str) {
        let mut clients = self.clients.write().await;
        clients.remove(agent_id);
    }

    /// Probe the immutable record that was selected for this health pass.
    pub async fn health_check(&self, agent: &RegisteredAgent) -> AgentHealth {
        match self.get_or_create(agent).await {
            Ok(client) if client.probe(agent).await.is_ok() => AgentHealth::Healthy,
            _ => AgentHealth::Unreachable,
        }
    }
}

impl Default for A2AClientPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::{AgentCard, TrustLevel};
    use chrono::Utc;

    fn make_agent(id: &str, url: &str) -> RegisteredAgent {
        let card = AgentCard {
            id: id.to_string(),
            name: format!("Agent {}", id),
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
        };
        RegisteredAgent::new(
            card,
            TrustLevel::Local,
            url.to_string(),
            Utc::now(),
            AgentHealth::Healthy,
            None,
        )
    }

    // NOTE: `len`/`is_empty` were cut as unused accessors (`dafc57bc6`). Pool
    // occupancy is now asserted through the observable that actually matters —
    // whether `get_or_create` hands back the SAME `Arc` — which is a stronger
    // statement than a count anyway.

    #[tokio::test]
    async fn default_and_new_both_yield_a_working_pool() {
        let agent = make_agent("agent-1", "http://localhost:9000");
        for pool in [A2AClientPool::new(), A2AClientPool::default()] {
            let client = pool.get_or_create(&agent).await.unwrap();
            assert_eq!(client.base_url(), "http://localhost:9000");
        }
    }

    #[tokio::test]
    async fn get_or_create_returns_same_client() {
        let pool = A2AClientPool::new();
        let agent = make_agent("agent-1", "http://localhost:9000");

        let client1 = pool.get_or_create(&agent).await.unwrap();
        let client2 = pool.get_or_create(&agent).await.unwrap();

        // Same Arc (same pointer) — i.e. it was cached, not rebuilt.
        assert!(Arc::ptr_eq(&client1, &client2));
    }

    #[tokio::test]
    async fn distinct_agents_get_distinct_clients() {
        let pool = A2AClientPool::new();
        let a1 = make_agent("agent-1", "http://localhost:9001");
        let a2 = make_agent("agent-2", "http://localhost:9002");

        let c1 = pool.get_or_create(&a1).await.unwrap();
        let c2 = pool.get_or_create(&a2).await.unwrap();

        assert!(!Arc::ptr_eq(&c1, &c2));
        assert_eq!(c1.base_url(), "http://localhost:9001");
        assert_eq!(c2.base_url(), "http://localhost:9002");
    }

    #[tokio::test]
    async fn remove_evicts_so_the_next_get_rebuilds() {
        let pool = A2AClientPool::new();
        let agent = make_agent("agent-1", "http://localhost:9000");

        let before = pool.get_or_create(&agent).await.unwrap();
        pool.remove("agent-1").await;
        let after = pool.get_or_create(&agent).await.unwrap();

        assert!(
            !Arc::ptr_eq(&before, &after),
            "a removed client must be rebuilt, not served from the cache"
        );
    }

    #[tokio::test]
    async fn remove_nonexistent_leaves_other_entries_alone() {
        let pool = A2AClientPool::new();
        let agent = make_agent("agent-1", "http://localhost:9000");

        let before = pool.get_or_create(&agent).await.unwrap();
        pool.remove("nonexistent").await;
        let after = pool.get_or_create(&agent).await.unwrap();

        assert!(Arc::ptr_eq(&before, &after));
    }

    #[tokio::test]
    async fn health_check_rejects_blocked_destination() {
        let pool = A2AClientPool::new();
        let result = pool
            .health_check(&make_agent("blocked", "http://127.0.0.1:1"))
            .await;
        assert_eq!(result, AgentHealth::Unreachable);
    }
}
