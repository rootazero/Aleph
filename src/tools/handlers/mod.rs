//! ToolHandler implementations for builtin / MCP / extension sources.

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError};

pub mod builtin;
pub mod mcp;
pub mod extension;

#[async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError>;
    fn definition(&self) -> ToolDefinition;
}
