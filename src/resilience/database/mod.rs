/// SQLite database for resilience state management
///
/// Provides the StateDatabase struct and CRUD operations for:
/// - Agent events (`events`)
/// - Agent tasks (`tasks`)
/// - Task traces (`traces`)
/// - Subagent sessions (`sessions`)
///
/// Schema migration utilities are in the `migration` submodule.
mod channel_offsets;
mod events;
mod group_chat;
mod memory_events;
pub mod migration;
mod paired_users;
mod replay;
mod sessions;
mod state_database;
mod tasks;
mod traces;

pub use state_database::{MemoryStats, StateDatabase, DEFAULT_EMBEDDING_DIM};
