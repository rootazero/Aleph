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

    /// Retire the state that belonged to `superseded` and only to the epoch it
    /// names — today, the `/btw` side session derived from it.
    ///
    /// [`register_epoch`] makes a *new* generation visible; this is the other
    /// half of the same moment. A `/btw` side session's key is derived from
    /// its conversation's key **including the epoch**, so the instant routing
    /// starts resolving to `epoch + 1`, the side session at the old epoch stops
    /// being derivable by anything: no surface can list it, reach it, seed it
    /// or delete it. A compaction split reaches that state with no user action
    /// at all, which is why the split cannot simply leave this to the
    /// user-facing retirement seam — nobody retired anything.
    ///
    /// Deliberately narrow, and deliberately *not* the full
    /// `terminate_session_continuations`: a split is not a user closing a
    /// conversation, so the loop and goal keyed to it must keep running.
    ///
    /// Best-effort by contract — a miss costs disk, never a crossed side
    /// thread. The default is a no-op so test doubles need not implement it;
    /// the production implementor is the gateway session store, which is the
    /// only thing that holds the handle this needs.
    ///
    /// [`register_epoch`]: SessionEpochRegistrar::register_epoch
    async fn retire_superseded(&self, superseded: &SessionId) {
        let _ = superseded;
    }
}
