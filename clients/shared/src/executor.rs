//! Local tool execution
//!
//! Provides trait for executing tools on the client side.

use crate::Result;
use async_trait::async_trait;
use serde_json::Value;

/// Local tool executor trait
///
/// Clients implement this to provide platform-specific tool execution.
#[async_trait]
pub trait LocalExecutor: Send + Sync {
    /// Execute a tool locally
    ///
    /// # Arguments
    ///
    /// * `tool_name` - The tool identifier (e.g., "shell:exec")
    /// * `args` - Tool arguments as JSON
    ///
    /// # Returns
    ///
    /// Tool execution result as JSON
    async fn execute(&self, tool_name: &str, args: Value) -> Result<Value>;
}
