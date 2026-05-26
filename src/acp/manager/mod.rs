//! AcpAdapterManager — lifecycle management for ACP harness sessions.
//!
//! Supports runtime dynamic harness registration and unregistration.
//!
//! ## Layout
//! - [`persistence`] — disk persistence + `wire_persistence` boot helper
//! - [`session_key`] — `SessionKey` + path canonicalization
//! - [`harness_admin`] — harness CRUD / queries / hooks / `restore_sessions` / `list_sessions`
//! - [`lifecycle`] — `ensure_session` / `prompt` / `cancel` / control RPCs / `shutdown_*`
//! - this module — struct definitions + `SessionSnapshot` + `Default` impl

mod harness_admin;
mod lifecycle;
mod persistence;
mod session_key;

#[cfg(test)]
mod tests;

pub use persistence::{load_persisted_sessions, save_persisted_sessions, wire_persistence};
pub use session_key::SessionKey;

use std::collections::HashMap;

use tokio::sync::{Mutex as AsyncMutex, RwLock};

use crate::acp::adapter::AcpAdapter;
use crate::acp::session::{AcpSession, CancelHandle};
use crate::config::types::acp::AcpAdapterEntry;
use crate::sync_primitives::Arc;

// =============================================================================
// SessionEntry
// =============================================================================

/// Pooled session slot.
///
/// `session` is an `Arc<AsyncMutex<AcpSession>>` so concurrent prompts to the
/// same (harness, cwd) serialize via the inner mutex instead of racing
/// remove/re-insert on the map. Different keys still progress in parallel.
///
/// `cancel` is a cloned handle that writes `session/cancel` directly to the
/// child's stdin — it does NOT acquire `session`, so it can interrupt an
/// in-flight prompt that currently owns the inner mutex.
#[derive(Clone)]
pub struct SessionEntry {
    pub session: Arc<AsyncMutex<AcpSession>>,
    pub cancel: CancelHandle,
}

impl SessionEntry {
    pub(super) fn new(session: AcpSession) -> Self {
        let cancel = session.cancel_handle();
        Self {
            session: Arc::new(AsyncMutex::new(session)),
            cancel,
        }
    }
}

// =============================================================================
// AcpAdapterManager
// =============================================================================

/// Manages ACP harness registrations and active sessions.
///
/// Supports two execution modes:
/// - **NativeAcp**: Persistent subprocess with ACP protocol (Gemini).
///   Uses lazy-start sessions that are automatically respawned if dead.
/// - **Oneshot**: Fresh process per prompt (Claude Code, Codex).
///   No persistent session needed.
///
/// All harness and config state is behind `RwLock` for runtime dynamic management.
pub struct AcpAdapterManager {
    pub(super) adapters: RwLock<HashMap<String, Arc<dyn AcpAdapter>>>,
    pub(super) configs: RwLock<HashMap<String, AcpAdapterEntry>>,
    /// Active sessions for NativeAcp harnesses, keyed by (harness_id, cwd).
    ///
    /// Outer `RwLock` guards the map shape (insert/remove). Each entry's
    /// inner `AsyncMutex` serializes operations on that specific session.
    pub(super) sessions: RwLock<HashMap<SessionKey, SessionEntry>>,
    /// Optional persistence callback for session state changes.
    pub(super) persistence_hook: RwLock<Option<crate::acp::PersistenceHook>>,
    /// Optional broadcast hook so the gateway can push
    /// `acp.sessions.changed` whenever the pool mutates. Kept separate from
    /// `persistence_hook` so disk-persistence and live-broadcast wire up
    /// independently (one consumer can exist without the other).
    pub(super) gateway_change_hook: RwLock<Option<crate::acp::GatewayChangeHook>>,
}

impl Default for AcpAdapterManager {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// SessionSnapshot
// =============================================================================

/// Lightweight view of a pooled ACP session — used by gateway RPC + panel.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionSnapshot {
    pub harness_id: String,
    pub cwd: String,
    /// `None` for the default unnamed session.
    pub session_name: Option<String>,
    pub acp_session_id: Option<String>,
    pub alive: bool,
    pub state: crate::acp::protocol::AcpSessionState,
}
