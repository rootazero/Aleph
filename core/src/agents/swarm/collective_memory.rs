//! Collective Memory
//!
//! Persists event bus history, providing vector retrieval and structured query capabilities.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::bus::AgentMessageBus;
use super::events::{EventTier, InfoEvent};
use crate::error::{AlephError, Result};

/// Collective Memory
///
/// Stores event history for on-demand retrieval by agents.
/// Provides both structured (SQLite) and semantic (vector) search.
pub struct CollectiveMemory {
    /// Reference to message bus
    bus: Arc<AgentMessageBus>,
    /// Event database for structured queries
    event_db: Arc<RwLock<EventDatabase>>,
}

/// In-memory event database (placeholder for SQLite)
struct EventDatabase {
    events: Vec<InfoEvent>,
    max_events: usize,
}

impl EventDatabase {
    fn new(max_events: usize) -> Self {
        Self {
            events: Vec::with_capacity(max_events),
            max_events,
        }
    }

    fn store_event(&mut self, event: InfoEvent) {
        if self.events.len() >= self.max_events {
            // Remove oldest events
            self.events.drain(0..self.max_events / 10);
        }
        self.events.push(event);
    }

    fn query_recent(&self, count: usize) -> Vec<&InfoEvent> {
        self.events
            .iter()
            .rev()
            .take(count)
            .collect()
    }

    fn query_by_agent(&self, agent_id: &str, count: usize) -> Vec<&InfoEvent> {
        self.events
            .iter()
            .rev()
            .filter(|e| match e {
                InfoEvent::ToolExecuted { agent_id: id, .. } => id == agent_id,
                InfoEvent::FileAccessed { agent_id: id, .. } => id == agent_id,
                InfoEvent::SymbolSearched { agent_id: id, .. } => id == agent_id,
            })
            .take(count)
            .collect()
    }

    fn query_by_path(&self, path_prefix: &str, count: usize) -> Vec<&InfoEvent> {
        self.events
            .iter()
            .rev()
            .filter(|e| match e {
                InfoEvent::FileAccessed { path, .. } => path.starts_with(path_prefix),
                InfoEvent::ToolExecuted { path: Some(p), .. } => p.starts_with(path_prefix),
                _ => false,
            })
            .take(count)
            .collect()
    }

    fn count(&self) -> usize {
        self.events.len()
    }

    fn clear(&mut self) {
        self.events.clear();
    }
}

impl CollectiveMemory {
    /// Create a new collective memory
    pub fn new(bus: Arc<AgentMessageBus>) -> Self {
        Self {
            bus,
            event_db: Arc::new(RwLock::new(EventDatabase::new(10000))),
        }
    }

    /// Create collective memory with custom capacity
    pub fn with_capacity(bus: Arc<AgentMessageBus>, capacity: usize) -> Self {
        Self {
            bus,
            event_db: Arc::new(RwLock::new(EventDatabase::new(capacity))),
        }
    }

    /// Run the collective memory background loop
    pub async fn run(self: Arc<Self>) {
        info!("Starting collective memory");

        // Subscribe to Info events (Tier 3)
        let mut info_rx = match self.bus.subscribe(EventTier::Info).await {
            Ok(rx) => rx,
            Err(e) => {
                warn!("Failed to subscribe to Info events: {}", e);
                return;
            }
        };

        // Process Info events and store them
        loop {
            match info_rx.recv().await {
                Ok(event) => {
                    if let super::events::AgentEvent::Info(info_event) = event {
                        let mut db = self.event_db.write().await;
                        db.store_event(info_event);
                        debug!("Stored event in collective memory");
                    }
                }
                Err(e) => {
                    warn!("Error receiving Info event: {}", e);
                    break;
                }
            }
        }

        info!("Collective memory stopped");
    }

    /// Search team history with optional filters
    pub async fn search_team_history(&self, query: TeamHistoryQuery) -> Result<Vec<String>> {
        let db = self.event_db.read().await;

        let events = match query {
            TeamHistoryQuery::Recent { count } => db.query_recent(count),
            TeamHistoryQuery::ByAgent { agent_id, count } => db.query_by_agent(&agent_id, count),
            TeamHistoryQuery::ByPath { path_prefix, count } => {
                db.query_by_path(&path_prefix, count)
            }
        };

        Ok(events.into_iter().map(|e| self.format_event(e)).collect())
    }

    /// Get event count
    pub async fn event_count(&self) -> usize {
        let db = self.event_db.read().await;
        db.count()
    }

    /// Clear all events
    pub async fn clear(&self) {
        let mut db = self.event_db.write().await;
        db.clear();
    }

    /// Format an event for display
    fn format_event(&self, event: &InfoEvent) -> String {
        match event {
            InfoEvent::ToolExecuted {
                agent_id,
                tool,
                path,
                timestamp,
            } => {
                let path_str = path
                    .as_ref()
                    .map(|p| format!(" on {}", p))
                    .unwrap_or_default();
                format!(
                    "[{}] {} executed {}{}",
                    Self::format_timestamp(*timestamp),
                    agent_id,
                    tool,
                    path_str
                )
            }
            InfoEvent::FileAccessed {
                agent_id,
                path,
                operation,
                timestamp,
            } => {
                format!(
                    "[{}] {} {:?} {}",
                    Self::format_timestamp(*timestamp),
                    agent_id,
                    operation,
                    path
                )
            }
            InfoEvent::SymbolSearched {
                agent_id,
                symbol,
                context,
                timestamp,
            } => {
                let context_str = context
                    .as_ref()
                    .map(|c| format!(" in {}", c))
                    .unwrap_or_default();
                format!(
                    "[{}] {} searched for {}{}",
                    Self::format_timestamp(*timestamp),
                    agent_id,
                    symbol,
                    context_str
                )
            }
        }
    }

    /// Format timestamp for display
    fn format_timestamp(timestamp: u64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let diff = now.saturating_sub(timestamp);

        if diff < 60 {
            format!("{}s ago", diff)
        } else if diff < 3600 {
            format!("{}m ago", diff / 60)
        } else {
            format!("{}h ago", diff / 3600)
        }
    }
}

/// Query types for team history
#[derive(Debug, Clone)]
pub enum TeamHistoryQuery {
    /// Get recent N events
    Recent { count: usize },
    /// Get events by specific agent
    ByAgent { agent_id: String, count: usize },
    /// Get events by path prefix
    ByPath { path_prefix: String, count: usize },
}

impl TeamHistoryQuery {
    /// Parse query from string
    pub fn from_string(query: &str) -> Result<Self> {
        // Simple parsing logic
        if query.starts_with("agent:") {
            let agent_id = query.strip_prefix("agent:").unwrap_or("").trim();
            Ok(Self::ByAgent {
                agent_id: agent_id.to_string(),
                count: 20,
            })
        } else if query.starts_with("path:") {
            let path = query.strip_prefix("path:").unwrap_or("").trim();
            Ok(Self::ByPath {
                path_prefix: path.to_string(),
                count: 20,
            })
        } else if query.starts_with("recent:") {
            let count_str = query.strip_prefix("recent:").unwrap_or("20").trim();
            let count = count_str.parse().unwrap_or(20);
            Ok(Self::Recent { count })
        } else {
            // Default: recent events
            Ok(Self::Recent { count: 20 })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::events::FileOperation;

    #[test]
    fn test_event_database() {
        let mut db = EventDatabase::new(100);

        let event1 = InfoEvent::FileAccessed {
            agent_id: "agent_1".into(),
            path: "/test1".into(),
            operation: FileOperation::Read,
            timestamp: 1,
        };

        let event2 = InfoEvent::FileAccessed {
            agent_id: "agent_2".into(),
            path: "/test2".into(),
            operation: FileOperation::Read,
            timestamp: 2,
        };

        db.store_event(event1);
        db.store_event(event2);

        assert_eq!(db.count(), 2);

        let recent = db.query_recent(1);
        assert_eq!(recent.len(), 1);
    }

    #[test]
    fn test_query_by_agent() {
        let mut db = EventDatabase::new(100);

        db.store_event(InfoEvent::FileAccessed {
            agent_id: "agent_1".into(),
            path: "/test1".into(),
            operation: FileOperation::Read,
            timestamp: 1,
        });

        db.store_event(InfoEvent::FileAccessed {
            agent_id: "agent_2".into(),
            path: "/test2".into(),
            operation: FileOperation::Read,
            timestamp: 2,
        });

        db.store_event(InfoEvent::FileAccessed {
            agent_id: "agent_1".into(),
            path: "/test3".into(),
            operation: FileOperation::Read,
            timestamp: 3,
        });

        let agent1_events = db.query_by_agent("agent_1", 10);
        assert_eq!(agent1_events.len(), 2);
    }

    #[test]
    fn test_query_by_path() {
        let mut db = EventDatabase::new(100);

        db.store_event(InfoEvent::FileAccessed {
            agent_id: "agent_1".into(),
            path: "/src/auth/login.rs".into(),
            operation: FileOperation::Read,
            timestamp: 1,
        });

        db.store_event(InfoEvent::FileAccessed {
            agent_id: "agent_2".into(),
            path: "/src/auth/logout.rs".into(),
            operation: FileOperation::Read,
            timestamp: 2,
        });

        db.store_event(InfoEvent::FileAccessed {
            agent_id: "agent_3".into(),
            path: "/src/core/main.rs".into(),
            operation: FileOperation::Read,
            timestamp: 3,
        });

        let auth_events = db.query_by_path("/src/auth", 10);
        assert_eq!(auth_events.len(), 2);
    }

    #[tokio::test]
    async fn test_collective_memory_creation() {
        let bus = Arc::new(AgentMessageBus::new());
        let memory = CollectiveMemory::new(bus);

        assert_eq!(memory.event_count().await, 0);
    }

    #[tokio::test]
    async fn test_team_history_query_parsing() {
        let query1 = TeamHistoryQuery::from_string("agent:agent_1").unwrap();
        assert!(matches!(query1, TeamHistoryQuery::ByAgent { .. }));

        let query2 = TeamHistoryQuery::from_string("path:/src/auth").unwrap();
        assert!(matches!(query2, TeamHistoryQuery::ByPath { .. }));

        let query3 = TeamHistoryQuery::from_string("recent:50").unwrap();
        assert!(matches!(query3, TeamHistoryQuery::Recent { count: 50 }));

        let query4 = TeamHistoryQuery::from_string("anything").unwrap();
        assert!(matches!(query4, TeamHistoryQuery::Recent { .. }));
    }

    #[tokio::test]
    async fn test_clear_memory() {
        let bus = Arc::new(AgentMessageBus::new());
        let memory = CollectiveMemory::new(bus);

        // Add event manually
        {
            let mut db = memory.event_db.write().await;
            db.store_event(InfoEvent::FileAccessed {
                agent_id: "agent_1".into(),
                path: "/test".into(),
                operation: FileOperation::Read,
                timestamp: 1,
            });
        }

        assert_eq!(memory.event_count().await, 1);

        memory.clear().await;
        assert_eq!(memory.event_count().await, 0);
    }
}
