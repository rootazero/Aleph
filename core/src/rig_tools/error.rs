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
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::Network(msg) => write!(f, "Network error: {}", msg),
            ToolError::InvalidArgs(msg) => write!(f, "Invalid arguments: {}", msg),
            ToolError::Execution(msg) => write!(f, "Execution error: {}", msg),
        }
    }
}

impl std::error::Error for ToolError {}
