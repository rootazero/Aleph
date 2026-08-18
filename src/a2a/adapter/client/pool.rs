use crate::sync_primitives::Arc;
use std::collections::HashMap;

use super::http_client::A2AClient;
use crate::a2a::port::{A2AResult, AgentHealth, RegisteredAgent};
use crate::sync_primitives::AsyncRwLock;

/// Connection pool managing `A2AClient` instances per agent.
///
/// Lazily creates clients on first access and caches them by agent ID.
/// Thread-safe via `AsyncRwLock` (read-heavy, write-rare pattern).
pub struct A2AClientPool {
    clients: AsyncRwLock<HashMap<String, Arc<A2AClient>>>,
}

impl A2AClientPool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            clients: AsyncRwLock::new(HashMap::new()),
        }
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
                if client_matches(client, agent) {
                    return Ok(Arc::clone(client));
                }
            }
        }

        // Slow path: create new client under write lock
        let client = Arc::new(match agent.auth_token() {
            Some(token) => A2AClient::with_auth(&agent.base_url, token),
            None => A2AClient::new(&agent.base_url),
        });
        let mut clients = self.clients.write().await;
        // Double-check: another task may have inserted while we waited (but
        // only reuse it if it, too, matches the requested endpoint/auth).
        if let Some(existing) = clients.get(&agent.card.id) {
            if client_matches(existing, agent) {
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

    /// Health check by fetching the agent card endpoint
    pub async fn health_check(&self, agent_id: &str) -> AgentHealth {
        let client = {
            let clients = self.clients.read().await;
            clients.get(agent_id).map(Arc::clone)
        };

        match client {
            Some(c) => match c.fetch_agent_card().await {
                Ok(_) => AgentHealth::Healthy,
                Err(_) => AgentHealth::Unreachable,
            },
            None => AgentHealth::Unreachable,
        }
    }
}

impl Default for A2AClientPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a cached client still reflects the requested agent's endpoint and
/// auth state. `A2AClient` trims trailing slashes off `base_url` at
/// construction, so compare against the trimmed form.
fn client_matches(client: &A2AClient, agent: &RegisteredAgent) -> bool {
    // Compare token *values*, not just presence — a rotated token with the
    // same presence would otherwise keep serving stale credentials.
    client.base_url() == agent.base_url.trim_end_matches('/')
        && client.auth_token_matches(agent.auth_token.as_deref())
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
        RegisteredAgent::new(card, TrustLevel::Local, url.to_string(), Utc::now(), AgentHealth::Healthy, None)
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
    async fn health_check_unknown_agent() {
        let pool = A2AClientPool::new();
        let result = pool.health_check("unknown").await;
        assert_eq!(result, AgentHealth::Unreachable);
    }
}
