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
            Self::Network(msg) => write!(f, "Network error: {msg}"),
            Self::InvalidArgs(msg) => write!(f, "Invalid arguments: {msg}"),
            Self::Execution(msg) => write!(f, "Execution error: {msg}"),
            Self::ExecutionFailed(msg) => write!(f, "Execution failed: {msg}"),
            Self::NotFound(msg) => write!(f, "Not found: {msg}"),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<ToolError> for crate::error::AlephError {
    fn from(e: ToolError) -> Self {
        match e {
            ToolError::Network(msg) => Self::network(msg),
            ToolError::InvalidArgs(msg) => Self::tool(msg),
            ToolError::Execution(msg) => Self::tool(msg),
            ToolError::ExecutionFailed(msg) => Self::tool(msg),
            ToolError::NotFound(msg) => Self::NotFound(msg),
        }
    }
}
