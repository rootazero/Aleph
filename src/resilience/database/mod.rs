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

/// Convert a SQLite i64 (signed) into a `u64` count, surfacing a negative
/// value as `AlephError::config` instead of letting `as u64` silently
/// turn a corrupt or undefined column into `u64::MAX`.
///
/// `COUNT(*)` in SQLite always returns a non-negative i64, but our schema
/// also stores `SUM(...)` of integer columns (see
/// `aggregate_usage_by_agents`); a corrupt row or a future schema change
/// could produce a negative sum, which would otherwise corrupt downstream
/// rollups without any error.
///
/// Centralising the helper keeps every caller consistent: a future call
/// site that needs `i64 -> u64` conversion must use this rather than the
/// bare `as u64` cast.
pub(crate) fn i64_to_u64_count(
    value: i64,
    column: &str,
) -> Result<u64, crate::error::AlephError> {
    if value < 0 {
        return Err(crate::error::AlephError::config(format!(
            "{column} must be non-negative, got {value}"
        )));
    }
    Ok(value as u64)
}
