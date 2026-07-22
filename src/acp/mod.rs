//! ACP (Agent Client Protocol) module
//!
//! Manages external CLI tools (Claude Code, Codex, Gemini) as ACP adapters.
//! Supports Tool mode (LLM-dispatched) and Agent mode (direct conversation).

use crate::sync_primitives::Arc;

pub mod adapter;
pub mod adapters;
pub mod incoming;
pub mod manager;
#[cfg(test)]
pub mod mock_server;
pub mod output_format;
pub mod protocol;
pub mod session;
pub mod transport;

#[cfg(test)]
mod tests;

/// Events emitted by the ACP manager for session persistence.
///
/// `session_name` is `None` for the default unnamed session and `Some(name)`
/// for parallel named sessions in the same `(harness, cwd)` slot — mirrors
/// acpx's `-s backend` / `-s frontend` shape.
#[derive(Debug, Clone)]
pub enum AcpSessionEvent {
    Created {
        harness_id: String,
        acp_session_id: String,
        cwd: String,
        session_name: Option<String>,
    },
    Removed {
        harness_id: String,
        cwd: String,
        session_name: Option<String>,
    },
}

/// Persistence hook for session state changes.
pub type PersistenceHook = Arc<dyn Fn(AcpSessionEvent) + Send + Sync>;

/// Lightweight notification fired whenever the live session pool changes.
///
/// Used by the gateway to publish `acp.sessions.changed` so panels can
/// re-fetch via `acp.sessions.list` without polling. Kept distinct from
/// `PersistenceHook` so disk-persistence and live-broadcast can be wired
/// independently.
pub type GatewayChangeHook = Arc<dyn Fn() + Send + Sync>;

/// Callback for real-time ACP streaming chunks.
pub type AcpChunkCallback = Arc<dyn Fn(&str) + Send + Sync>;
