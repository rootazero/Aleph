//! `ToolHandler` implementations for builtin / MCP sources.
//!
//! The `extension` submodule was removed 2026-05-20: it was a Phase-2-era
//! placeholder for WASM plugin tools that flowed into `tools::ToolHandlerRegistry`,
//! but the boot wiring (Phase 2 Task 10 — `AppContext` plumbing) never
//! landed. Plugin tools today reach the LLM exclusively via the
//! `tool_metadata::ToolCatalog` path that `run_loop` consults. Re-introduce
//! `ExtensionHandler` only when the Gap 1 unification of Phase-2 vs
//! tool-catalog registries is settled (see CLAUDE.md memory notes).

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError};

pub mod builtin;
pub mod mcp;
pub mod registration;

#[async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError>;
    fn definition(&self) -> ToolDefinition;
}
