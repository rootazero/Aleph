/// SQLite database for resilience state management
///
/// Provides the StateDatabase struct and CRUD operations for:
/// - Agent events (`events`)
/// - Agent tasks (`tasks`)
/// - Task traces (`traces`)
/// - Subagent sessions (`sessions`)
///
/// Schema migration utilities are in the `migration` submodule.

mod events;
mod memory_events;
mod poe_events;
mod poe_trust;
pub mod migration;
mod sessions;
mod state_database;
mod tasks;
mod traces;

pub use poe_trust::TrustScoreRow;
pub use state_database::{MemoryStats, StateDatabase, DEFAULT_EMBEDDING_DIM};
