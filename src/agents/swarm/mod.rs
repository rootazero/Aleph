//! Swarm Intelligence Module
//!
//! Implements horizontal agent collaboration through:
//! - Event bus for agent-to-agent communication
//! - Semantic aggregation for information density
//! - Context injection for situational awareness
//! - Collective memory for shared knowledge

pub mod aggregator;
pub mod bus;
pub mod collective_memory;
pub mod context_injector;
pub mod context_provider;
pub mod coordinator;
pub mod events;
pub mod rules;
pub mod tasks;
pub mod tools;

pub use aggregator::{IntelligenceLayer, SemanticAggregator, SlidingWindow};
pub use bus::AgentMessageBus;
pub use collective_memory::{CollectiveMemory, TeamHistoryQuery};
pub use context_injector::{ContextInjector, SwarmContextEntry};
pub use context_provider::SwarmContextProvider;
pub use coordinator::{SwarmConfig, SwarmCoordinator, SwarmStatistics};
pub use events::{AgentEvent, CriticalEvent, EventTier, FileOperation, ImportantEvent, InfoEvent};
pub use rules::{AggregationRule, EventPattern, RuleEngine};
pub use tools::GetTeamActivityTool;
