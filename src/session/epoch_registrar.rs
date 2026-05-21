//! Narrow trait for registering a new session epoch (generation).
//!
//! Compaction-driven session-split mints a child session key at `epoch + 1`
//! and must make that generation visible to epoch resolution
//! (`SessionStore::get_current_epoch`). The harness depends on this narrow
//! trait rather than the gateway `SessionStore` concrete type, preserving the
//! Core -> Interface dependency direction (CLAUDE.md R1 / P4).

use async_trait::async_trait;

use crate::session::service::SessionId;

/// Persists a session key as a live generation so epoch resolution sees it.
#[async_trait]
pub trait SessionEpochRegistrar: Send + Sync {
    /// Register `key` (typically a child session at the next epoch) so that a
    /// subsequent `get_current_epoch` for its base pattern resolves to it.
    async fn register_epoch(&self, key: &SessionId) -> anyhow::Result<()>;
}
