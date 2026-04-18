//! ACP (Agent Client Protocol) module
//!
//! Manages external CLI tools (Claude Code, Codex, Gemini) as ACP adapters.
//! Supports Tool mode (LLM-dispatched) and Agent mode (direct conversation).

use crate::sync_primitives::Arc;

pub mod adapter;
pub mod adapters;
pub mod manager;
#[cfg(test)]
pub mod mock_server;
pub mod protocol;
pub mod session;
pub mod transport;

#[cfg(test)]
mod tests;

/// Events emitted by the ACP manager for session persistence.
#[derive(Debug, Clone)]
pub enum AcpSessionEvent {
    Created {
        harness_id: String,
        acp_session_id: String,
        cwd: String,
    },
    Updated {
        harness_id: String,
        acp_session_id: String,
    },
    Removed {
        harness_id: String,
        cwd: String,
    },
}

/// Persistence hook for session state changes.
pub type PersistenceHook = Arc<dyn Fn(AcpSessionEvent) + Send + Sync>;

/// Callback for real-time ACP streaming chunks.
pub type AcpChunkCallback = Arc<dyn Fn(&str) + Send + Sync>;
