//! The [`ToolHandler`] trait — the loop-side contract every tool
//! implements.
//!
//! Five methods, all with `&self` (and a `&Value` for
//! `concurrency_claim`, a `&McpServerFilter` for
//! `bind_visible_servers`):
//!   * `Self::invoke` — execute a call.
//!   * `Self::definition` — describe the tool's contract.
//!   * `Self::concurrency_claim` — claim for the actual
//!     implementation and input, not its registry name. Default:
//!     `Global` (the safest answer).
//!   * `Self::bind_visible_servers` — when an MCP handler depends
//!     on the visible-server set, return a child that knows about
//!     it. Default: `None`.
//!   * `Self::fences_output` — `true` when the tool's output must
//!     be wrapped in a presentation fence before the model reads it.
//!     Default: `true` for `Mcp` sources, `false` otherwise.
//!
//! ## Identity surface (must NOT change without an explicit proposal)
//!
//! * The trait is `Send + Sync + 'static` and `#[async_trait]`. The
//!   trait is the boundary the harness dispatches through, and
//!   every production handler implements it. Adding a method is
//!   fine; removing one is a breaking change for every implementor.
//! * `McpServerFilter` is a public, pure type alias — `Arc<dyn Fn(&str) -> bool
//!   + Send + Sync>`. It is the type of the parameter
//!   `bind_visible_servers` accepts; consumers (e.g. the MCP
//!     host) cast their concrete filter to this alias at the trait
//!     boundary.
//!
//! * The signatures of `concurrency_claim(&self, &Value)` and
//!   `bind_visible_servers(&self, &McpServerFilter)` are the
//!   wire boundary with the production Core handlers
//!   (`tools::handlers::builtin` / `mcp` / `registration`). Changing
//!   either signature is a breaking change for every implementor
//!   in the Core crate and is forbidden without an explicit
//!   proposal.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::concurrency::ConcurrencyClaim;
use crate::descriptor::ToolSource;
use crate::tool::{ToolDefinition, ToolError, ToolOutput};

// ============================================================================
// McpServerFilter
// ============================================================================

/// A pure closure an MCP handler may receive in
/// [`ToolHandler::bind_visible_servers`]. Given a server id, returns
/// `true` if the server is visible to the run.
///
/// Public, pure type alias — `Arc<dyn Fn(&str) -> bool + Send + Sync>`.
pub type McpServerFilter = Arc<dyn Fn(&str) -> bool + Send + Sync>;

// ============================================================================
// ToolHandler
// ============================================================================

/// The loop-side contract every tool implements. `Send + Sync + 'static`
/// so the harness can keep handlers across run / dispatch boundaries.
#[async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    /// Execute a call. The `input` is the LLM-side JSON; the handler
    /// validates it against its own `definition().input_schema` and
    /// produces a [`ToolOutput`].
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError>;

    /// Describe the tool's contract. The harness hands this to the
    /// descriptor pipeline; the descriptor and the loop-side
    /// definition must agree on `name` / `source` / `input_schema`
    /// (see `ToolCapabilityDescriptor::matches_definition`).
    fn definition(&self) -> ToolDefinition;

    /// Claim for the actual implementation and input, not its registry name.
    /// `Shared` also admits reads through Plan/side-question gates, so neither
    /// a builtin-looking name nor concurrency metadata alone is sufficient.
    fn concurrency_claim(&self, _input: &Value) -> ConcurrencyClaim {
        ConcurrencyClaim::global()
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
        matches!(self.definition().source, ToolSource::Mcp { .. })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolDefinitionMetadata;
    use async_trait::async_trait;
    use serde_json::json;

    /// Minimal stub for tests. Each test that exercises a default
    /// method can configure the stub.
    struct StubHandler {
        def: ToolDefinition,
    }

    #[async_trait]
    impl ToolHandler for StubHandler {
        async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({"ok": true}),
                metadata: crate::tool::ToolOutputMetadata::default(),
            })
        }

        fn definition(&self) -> ToolDefinition {
            self.def.clone()
        }
    }

    fn builtin_def() -> ToolDefinition {
        ToolDefinition {
            name: "x".to_string(),
            description: "x".to_string(),
            input_schema: json!({"type": "object"}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    fn mcp_def() -> ToolDefinition {
        ToolDefinition {
            name: "x".to_string(),
            description: "x".to_string(),
            input_schema: json!({"type": "object"}),
            source: ToolSource::Mcp {
                server_id: "alpha".to_string(),
            },
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    #[test]
    fn default_concurrency_claim_is_global() {
        let stub = StubHandler { def: builtin_def() };
        assert!(matches!(
            stub.concurrency_claim(&json!({})),
            ConcurrencyClaim::Exclusive {
                scope: crate::concurrency::ExclusiveScope::Global
            }
        ));
    }

    #[test]
    fn default_bind_visible_servers_is_none() {
        let stub = StubHandler { def: mcp_def() };
        let filter: McpServerFilter = Arc::new(|_server| true);
        assert!(stub.bind_visible_servers(&filter).is_none());
    }

    #[test]
    fn default_fences_output_is_mcp_only() {
        let builtin = StubHandler { def: builtin_def() };
        let mcp = StubHandler { def: mcp_def() };
        assert!(!builtin.fences_output());
        assert!(mcp.fences_output());
    }

    #[test]
    fn mcp_server_filter_is_a_pure_alias() {
        // The alias is a public, pure type — verify it accepts a
        // closure and the call returns the expected answer.
        let filter: McpServerFilter = Arc::new(|server| server == "alpha");
        assert!(filter("alpha"));
        assert!(!filter("beta"));
    }
}
