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
use crate::sync_primitives::Arc;
use crate::tools::service::{ToolDefinition, ToolError};

pub mod builtin;
pub mod mcp;
pub mod registration;

/// Which MCP servers one run may see, by server id: face ⑤ of
/// `extension::visibility`, built once per run by the run loop's MCP join.
pub(crate) type McpServerFilter = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError>;
    fn definition(&self) -> ToolDefinition;

    /// Claim for the actual implementation and input, not its registry name.
    /// `Shared` also admits reads through Plan/side-question gates, so neither
    /// a builtin-looking name nor concurrency metadata alone is sufficient.
    fn concurrency_claim(&self, _input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
        crate::tools::concurrency::ConcurrencyClaim::global()
    }

    /// This handler bound to the MCP servers one run may see, for a handler
    /// whose behaviour depends on that set. Only the MCP bridge's capability
    /// builtins answer `Some` (they enumerate or resolve servers at call
    /// time); a per-server MCP tool is gated whole at the join instead, and
    /// every other handler never touches a server. `None` = join as-is.
    fn bind_visible_servers(&self, _visible: &McpServerFilter) -> Option<Arc<dyn ToolHandler>> {
        None
    }

    /// Whether this handler's output must be fence-quoted before being shown to
    /// the model. Only MCP-sourced tools answer `true`: a tool's raw payload is
    /// arbitrary server-controlled text, so any `<function_results>` /
    /// `<tool_result>` boundary it could include is untrusted and the harness
    /// needs to know to wrap the output rather than pass it through verbatim.
    fn fences_output(&self) -> bool {
        matches!(
            self.definition().source,
            crate::tools::service::ToolSource::Mcp { .. }
        )
    }
}
