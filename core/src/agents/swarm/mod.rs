//! Swarm Intelligence Module
//!
//! Implements horizontal agent collaboration through:
//! - Event bus for agent-to-agent communication
//! - Semantic aggregation for information density
//! - Context injection for situational awareness
//! - Collective memory for shared knowledge

pub mod events;
pub mod bus;
pub mod aggregator;
pub mod rules;
pub mod context_injector;
pub mod collective_memory;
pub mod coordinator;
pub mod tools;

pub use events::{AgentEvent, CriticalEvent, ImportantEvent, InfoEvent, EventTier, FileOperation};
pub use bus::AgentMessageBus;
pub use aggregator::{SemanticAggregator, IntelligenceLayer};
pub use rules::{AggregationRule, EventPattern, RuleEngine};
pub use context_injector::{ContextInjector, SwarmContextEntry};
pub use collective_memory::{CollectiveMemory, TeamHistoryQuery};
pub use coordinator::{SwarmCoordinator, SwarmConfig, SwarmStatistics};
pub use tools::GetTeamActivityTool;
