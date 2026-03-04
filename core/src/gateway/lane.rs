//! Lane-based concurrency control for Gateway RPC methods.
//!
//! Prevents one category of RPC methods from starving others by partitioning
//! methods into lanes, each with its own semaphore-based concurrency limit.
//!
//! # Lanes
//!
//! - **Query**: Read-only queries (health, echo, config.get, etc.)
//! - **Execute**: Agent execution (agent.run, chat.send, poe.run, etc.)
//! - **Mutate**: State mutations (config.patch, memory.store, etc.)
//! - **System**: System management (plugins.install, skills.delete, etc.)

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Traffic lane for categorizing RPC methods.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Lane {
    /// Read-only queries (health, echo, config.get, models.list, etc.)
    Query,
    /// Agent execution (agent.run, chat.send, poe.run, poe.prepare)
    Execute,
    /// State mutations (config.patch, config.apply, config.set, memory.store, memory.delete, session.compact, session.delete)
    Mutate,
    /// System management (plugins.install, plugins.uninstall, skills.install, skills.delete, logs.setLevel)
    System,
}

impl Lane {
    /// Map an RPC method name to its corresponding lane.
    ///
    /// Returns `Lane::Query` for unrecognized methods.
    pub fn for_method(method: &str) -> Self {
        match method {
            // Execute lane
            "agent.run" | "chat.send" | "poe.run" | "poe.prepare" => Lane::Execute,

            // Mutate lane
            "config.patch" | "config.apply" | "config.set" | "memory.store"
            | "memory.delete" | "session.compact" | "session.delete" => Lane::Mutate,

            // System lane
            "plugins.install" | "plugins.uninstall" | "skills.install"
            | "skills.delete" | "logs.setLevel" => Lane::System,

            // Everything else is a query
            _ => Lane::Query,
        }
    }
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Lane::Query => write!(f, "Query"),
            Lane::Execute => write!(f, "Execute"),
            Lane::Mutate => write!(f, "Mutate"),
            Lane::System => write!(f, "System"),
        }
    }
}

/// Configuration for lane concurrency limits.
#[derive(Clone, Debug)]
pub struct LaneConfig {
    /// Maximum concurrent query requests.
    pub query_concurrency: usize,
    /// Maximum concurrent execute requests.
    pub execute_concurrency: usize,
    /// Maximum concurrent mutate requests.
    pub mutate_concurrency: usize,
    /// Maximum concurrent system requests.
    pub system_concurrency: usize,
    /// Timeout in seconds for acquiring a lane permit.
    pub acquire_timeout_secs: u64,
}

impl Default for LaneConfig {
    fn default() -> Self {
        Self {
            query_concurrency: 50,
            execute_concurrency: 5,
            mutate_concurrency: 10,
            system_concurrency: 3,
            acquire_timeout_secs: 30,
        }
    }
}

/// Errors returned by lane operations.
#[derive(Debug)]
pub enum LaneError {
    /// The lane is congested and the permit could not be acquired within the timeout.
    Congested(Lane),
}

impl fmt::Display for LaneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaneError::Congested(lane) => {
                write!(f, "lane {} is congested, could not acquire permit", lane)
            }
        }
    }
}

impl std::error::Error for LaneError {}

/// Lane-based concurrency manager.
///
/// Each lane has its own semaphore. Acquiring a permit on one lane does not
/// affect availability on other lanes, preventing one category of RPC methods
/// from starving others.
pub struct LaneManager {
    /// One semaphore per lane.
    lanes: HashMap<Lane, Arc<Semaphore>>,
    /// Timeout for acquiring a permit.
    timeout: Duration,
}

impl LaneManager {
    /// Create a new `LaneManager` from the given configuration.
    pub fn new(config: LaneConfig) -> Self {
        let mut lanes = HashMap::new();
        lanes.insert(Lane::Query, Arc::new(Semaphore::new(config.query_concurrency)));
        lanes.insert(
            Lane::Execute,
            Arc::new(Semaphore::new(config.execute_concurrency)),
        );
        lanes.insert(
            Lane::Mutate,
            Arc::new(Semaphore::new(config.mutate_concurrency)),
        );
        lanes.insert(
            Lane::System,
            Arc::new(Semaphore::new(config.system_concurrency)),
        );

        Self {
            lanes,
            timeout: Duration::from_secs(config.acquire_timeout_secs),
        }
    }

    /// Acquire a permit for the lane corresponding to the given RPC method.
    ///
    /// Returns an `OwnedSemaphorePermit` that releases the slot on drop,
    /// or `LaneError::Congested` if the timeout elapses.
    pub async fn acquire(&self, method: &str) -> Result<OwnedSemaphorePermit, LaneError> {
        let lane = Lane::for_method(method);
        let semaphore = self
            .lanes
            .get(&lane)
            .expect("all lanes are initialized in new()")
            .clone();

        match tokio::time::timeout(self.timeout, semaphore.acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_closed)) => {
                // Semaphore closed — treat as congested
                Err(LaneError::Congested(lane))
            }
            Err(_elapsed) => Err(LaneError::Congested(lane)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_acquire_query_lane() {
        let manager = LaneManager::new(LaneConfig::default());
        let result = manager.acquire("health").await;
        assert!(result.is_ok(), "should acquire query lane for 'health'");
    }

    #[tokio::test]
    async fn test_lane_for_method() {
        // Query lane (explicit + default fallback)
        assert_eq!(Lane::for_method("health"), Lane::Query);
        assert_eq!(Lane::for_method("echo"), Lane::Query);
        assert_eq!(Lane::for_method("config.get"), Lane::Query);
        assert_eq!(Lane::for_method("models.list"), Lane::Query);
        assert_eq!(Lane::for_method("unknown.method"), Lane::Query);

        // Execute lane
        assert_eq!(Lane::for_method("agent.run"), Lane::Execute);
        assert_eq!(Lane::for_method("chat.send"), Lane::Execute);
        assert_eq!(Lane::for_method("poe.run"), Lane::Execute);
        assert_eq!(Lane::for_method("poe.prepare"), Lane::Execute);

        // Mutate lane
        assert_eq!(Lane::for_method("config.patch"), Lane::Mutate);
        assert_eq!(Lane::for_method("config.apply"), Lane::Mutate);
        assert_eq!(Lane::for_method("config.set"), Lane::Mutate);
        assert_eq!(Lane::for_method("memory.store"), Lane::Mutate);
        assert_eq!(Lane::for_method("memory.delete"), Lane::Mutate);
        assert_eq!(Lane::for_method("session.compact"), Lane::Mutate);
        assert_eq!(Lane::for_method("session.delete"), Lane::Mutate);

        // System lane
        assert_eq!(Lane::for_method("plugins.install"), Lane::System);
        assert_eq!(Lane::for_method("plugins.uninstall"), Lane::System);
        assert_eq!(Lane::for_method("skills.install"), Lane::System);
        assert_eq!(Lane::for_method("skills.delete"), Lane::System);
        assert_eq!(Lane::for_method("logs.setLevel"), Lane::System);
    }

    #[tokio::test]
    async fn test_execute_lane_saturation() {
        let config = LaneConfig {
            execute_concurrency: 1,
            acquire_timeout_secs: 1,
            ..Default::default()
        };
        let manager = LaneManager::new(config);

        // Hold the only permit
        let _permit = manager
            .acquire("agent.run")
            .await
            .expect("first acquire should succeed");

        // Second acquire should time out
        let result = manager.acquire("chat.send").await;
        assert!(result.is_err(), "second acquire should fail (lane saturated)");
        match result.unwrap_err() {
            LaneError::Congested(lane) => {
                assert_eq!(lane, Lane::Execute);
            }
        }
    }

    #[tokio::test]
    async fn test_different_lanes_independent() {
        let config = LaneConfig {
            execute_concurrency: 1,
            acquire_timeout_secs: 1,
            ..Default::default()
        };
        let manager = LaneManager::new(config);

        // Saturate execute lane
        let _permit = manager
            .acquire("agent.run")
            .await
            .expect("execute acquire should succeed");

        // Query lane should still work
        let result = manager.acquire("health").await;
        assert!(
            result.is_ok(),
            "query lane should be independent of execute lane"
        );
    }
}
