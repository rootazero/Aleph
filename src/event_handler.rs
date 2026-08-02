//! Event handler trait for callbacks from Rust to client code.
//!
//! This trait defines the callback interface that Swift/Kotlin clients
//! must implement to receive events from the Aleph core. `UniFFI` will
//! generate a protocol/interface for each target language.

/// Processing states for the Aleph system
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingState {
    /// System is idle, not processing
    Idle,
    /// Listening for hotkey events
    Listening,
    /// Retrieving memories from database (Phase 7 - Task 7.5)
    RetrievingMemory,
    /// AI provider is processing the request (Phase 7 - Task 7.5)
    ProcessingWithAI,
    /// Processing AI request (kept for backward compatibility)
    Processing,
    /// Typewriter animation in progress (Phase 7.2)
    Typewriting,
    /// Operation completed successfully
    Success,
    /// Operation failed with error
    Error,
}

/// Error types for typed error handling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorType {
    /// Network connectivity error
    Network,
    /// Permission denied error
    Permission,
    /// API quota exceeded
    Quota,
    /// Request timeout
    Timeout,
    /// Unknown error type
    Unknown,
}

// ========================================================================
// MCP Startup Report Types (Phase 3.3 - Swift callback)
// ========================================================================

/// MCP server error information for FFI
///
/// Contains details about a server that failed to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerError {
    /// Name of the server that failed
    pub server_name: String,
    /// Human-readable error message
    pub error_message: String,
}

/// MCP startup report for FFI
///
/// Contains information about MCP server startup results,
/// sent to Swift via `on_mcp_startup_complete` callback.
#[derive(Debug, Clone, Default)]
pub struct McpStartupReport {
    /// Names of servers that started successfully
    pub succeeded_servers: Vec<String>,
    /// Servers that failed to start with error details
    pub failed_servers: Vec<McpServerError>,
}

impl McpStartupReport {
    /// Check if all servers started successfully
    #[must_use]
    pub const fn all_succeeded(&self) -> bool {
        self.failed_servers.is_empty()
    }

    /// Get total number of servers (succeeded + failed)
    #[must_use]
    pub const fn total_count(&self) -> usize {
        self.succeeded_servers.len() + self.failed_servers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_processing_state_equality() {
        assert_eq!(ProcessingState::Idle, ProcessingState::Idle);
        assert_ne!(ProcessingState::Idle, ProcessingState::Listening);
    }
}
