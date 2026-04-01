//! ACP (Agent Client Protocol) module
//!
//! Manages external CLI tools (Claude Code, Codex, Gemini) as ACP harnesses.
//! Supports Tool mode (LLM-dispatched) and Agent mode (direct conversation).

use crate::sync_primitives::Arc;

pub mod harness;
pub mod harnesses;
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

/// Callback for real-time ACP streaming chunks.
pub type AcpChunkCallback = Arc<dyn Fn(&str) + Send + Sync>;
