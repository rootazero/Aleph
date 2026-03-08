use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::a2a::port::{AgentHealth, RegisteredAgent, A2AResult};
use super::http_client::A2AClient;

/// Connection pool managing A2AClient instances per agent.
///
/// Lazily creates clients on first access and caches them by agent ID.
/// Thread-safe via `tokio::sync::RwLock` (read-heavy, write-rare pattern).
pub struct A2AClientPool {
    clients: RwLock<HashMap<String, Arc<A2AClient>>>,
}

impl A2AClientPool {
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
        }
    }

    /// Get or create a client for a registered agent.
    ///
    /// Uses read lock for the fast path (client exists) and only
    /// acquires a write lock when creating a new client.
    pub async fn get_or_create(&self, agent: &RegisteredAgent) -> A2AResult<Arc<A2AClient>> {
        // Fast path: read lock
        {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(&agent.card.id) {
                return Ok(Arc::clone(client));
            }
        }

        // Slow path: create new client under write lock
        let client = Arc::new(A2AClient::new(&agent.base_url));
        let mut clients = self.clients.write().await;
        // Double-check: another task may have inserted while we waited
        if let Some(existing) = clients.get(&agent.card.id) {
            return Ok(Arc::clone(existing));
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

    /// Number of clients in the pool
    pub async fn len(&self) -> usize {
        self.clients.read().await.len()
    }

    /// Whether the pool is empty
    pub async fn is_empty(&self) -> bool {
        self.clients.read().await.is_empty()
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
        RegisteredAgent {
            card: AgentCard {
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
            },
            trust_level: TrustLevel::Local,
            base_url: url.to_string(),
            last_seen: Utc::now(),
            health: AgentHealth::Healthy,
        }
    }

    #[tokio::test]
    async fn new_pool_is_empty() {
        let pool = A2AClientPool::new();
        assert!(pool.is_empty().await);
        assert_eq!(pool.len().await, 0);
    }

    #[tokio::test]
    async fn default_pool_is_empty() {
        let pool = A2AClientPool::default();
        assert!(pool.is_empty().await);
    }

    #[tokio::test]
    async fn get_or_create_adds_client() {
        let pool = A2AClientPool::new();
        let agent = make_agent("agent-1", "http://localhost:9000");

        let client = pool.get_or_create(&agent).await.unwrap();
        assert_eq!(client.base_url(), "http://localhost:9000");
        assert_eq!(pool.len().await, 1);
        assert!(!pool.is_empty().await);
    }

    #[tokio::test]
    async fn get_or_create_returns_same_client() {
        let pool = A2AClientPool::new();
        let agent = make_agent("agent-1", "http://localhost:9000");

        let client1 = pool.get_or_create(&agent).await.unwrap();
        let client2 = pool.get_or_create(&agent).await.unwrap();

        // Same Arc (same pointer)
        assert!(Arc::ptr_eq(&client1, &client2));
        assert_eq!(pool.len().await, 1);
    }

    #[tokio::test]
    async fn get_or_create_multiple_agents() {
        let pool = A2AClientPool::new();
        let a1 = make_agent("agent-1", "http://localhost:9001");
        let a2 = make_agent("agent-2", "http://localhost:9002");

        pool.get_or_create(&a1).await.unwrap();
        pool.get_or_create(&a2).await.unwrap();

        assert_eq!(pool.len().await, 2);
    }

    #[tokio::test]
    async fn remove_client() {
        let pool = A2AClientPool::new();
        let agent = make_agent("agent-1", "http://localhost:9000");

        pool.get_or_create(&agent).await.unwrap();
        assert_eq!(pool.len().await, 1);

        pool.remove("agent-1").await;
        assert_eq!(pool.len().await, 0);
        assert!(pool.is_empty().await);
    }

    #[tokio::test]
    async fn remove_nonexistent_is_noop() {
        let pool = A2AClientPool::new();
        pool.remove("nonexistent").await;
        assert!(pool.is_empty().await);
    }

    #[tokio::test]
    async fn health_check_unknown_agent() {
        let pool = A2AClientPool::new();
        let result = pool.health_check("unknown").await;
        assert_eq!(result, AgentHealth::Unreachable);
    }
}
