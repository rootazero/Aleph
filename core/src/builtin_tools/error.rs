//! Tool error types

use std::fmt;

/// Error type for tool execution
#[derive(Debug)]
pub enum ToolError {
    /// Network error
    Network(String),
    /// Invalid arguments
    InvalidArgs(String),
    /// Execution failed
    Execution(String),
    /// Execution failed (alias for Execution)
    ExecutionFailed(String),
    /// Resource not found
    NotFound(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::Network(msg) => write!(f, "Network error: {}", msg),
            ToolError::InvalidArgs(msg) => write!(f, "Invalid arguments: {}", msg),
            ToolError::Execution(msg) => write!(f, "Execution error: {}", msg),
            ToolError::ExecutionFailed(msg) => write!(f, "Execution failed: {}", msg),
            ToolError::NotFound(msg) => write!(f, "Not found: {}", msg),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<ToolError> for crate::error::AlephError {
    fn from(e: ToolError) -> Self {
        match e {
            ToolError::Network(msg) => crate::error::AlephError::network(msg),
            ToolError::InvalidArgs(msg) => crate::error::AlephError::tool(msg),
            ToolError::Execution(msg) => crate::error::AlephError::tool(msg),
            ToolError::ExecutionFailed(msg) => crate::error::AlephError::tool(msg),
            ToolError::NotFound(msg) => crate::error::AlephError::NotFound(msg),
        }
    }
}
