/// SQLite database for resilience state management
///
/// Provides the StateDatabase struct and CRUD operations for:
/// - Agent tasks (`tasks`)
/// - Task traces (`traces`)
///
/// Schema migration utilities are in the `migration` submodule.
mod channel_offsets;
mod group_chat;
mod memory_events;
pub mod migration;
mod state_database;
mod tasks;
mod traces;

pub use state_database::{StateDatabase, DEFAULT_EMBEDDING_DIM};
pub use traces::AgentUsageTotal;
