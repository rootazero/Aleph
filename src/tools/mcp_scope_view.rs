//! P3 Stage I — `McpScopedToolService` layers per-agent MCP scope tools UNDER
//! the existing `AllowlistToolService` gate. Parent's tools take precedence;
//! `extras` fill in tools the parent doesn't expose.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::extension::registry::ToolRegistration;
use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct McpScopedToolService {
    parent: Arc<dyn ToolService>,
    extras: Vec<ToolRegistration>,
}

impl McpScopedToolService {
    pub fn new(parent: Arc<dyn ToolService>, extras: Vec<ToolRegistration>) -> Self {
        Self { parent, extras }
    }

    /// Whether `name` is an extras-only entry — present in `self.extras` but
    /// NOT served by the parent. Stage I MVP cannot dispatch these: there is
    /// no extension-runtime handle carried by this service, so calling them
    /// via `execute` would silently hit the parent's NotFound. We therefore
    /// keep the surfaces (`list` / `describe` / `metadata_schema` /
    /// `dispatchable_list`) and the dispatch (`execute` / `execute_with_cancel`)
    /// in lock-step: either a tool is exposed everywhere AND callable, or it
    /// is absent from all four surfaces and surfaces a clear `NotFound` on
    /// dispatch. Stage II replaces this with an actual extension-runtime
    /// handle; until then, extras are reserved, not advertised.
    async fn is_extras_only(&self, name: &str) -> bool {
        self.extras.iter().any(|t| t.name == name)
            && self.parent.describe(name).await.is_none()
    }
}

#[async_trait]
impl ToolService for McpScopedToolService {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError> {
        // Extras-only entries are not dispatchable in Stage I. Surface a
        // clear NotFound rather than silently forwarding to the parent (which
        // would return the same NotFound but imply the tool was once wired —
        // it was not). The error preserves the name so the model can adjust.
        if self.is_extras_only(name).await {
            return Err(ToolError::NotFound {
                name: name.to_string(),
            });
        }
        self.parent.execute(name, input).await
    }

    async fn execute_with_cancel(
        &self,
        name: &str,
        input: serde_json::Value,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        // Same guard as `execute`: extras-only entries cannot be dispatched,
        // so cancel-aware execution also short-circuits with NotFound.
        if self.is_extras_only(name).await {
            return Err(ToolError::NotFound {
                name: name.to_string(),
            });
        }
        // Delegate to the parent's cancel-aware path so the inner
        // `ScopedToolService` actually threads the token into `LoopTool::execute`.
        self.parent.execute_with_cancel(name, input, cancel).await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        // Stage I MVP: extras are NOT advertised. Keeping the surfaces and
        // dispatch consistent prevents the model from invoking a name that
        // would NotFound. Stage II (extension-runtime handle) re-introduces
        // the merge once dispatch lands.
        self.parent.list().await
    }

    async fn dispatchable_list(&self) -> Vec<ToolDefinition> {
        // Forward the parent's dispatchable set (visible + deferred tier)
        // instead of the trait default (`list()`), which silently drops the
        // parent `ScopedToolService`'s deferred MCP names from the
        // name-repairer's candidate set — a correct call to a deferred tool
        // would miss the Exact tier and could be fuzzily rewritten into a
        // different resident tool.
        self.parent.dispatchable_list().await
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        // Extras are not advertised in Stage I, so `describe` mirrors the
        // parent's view. Future Stage II will add an extras fallback here
        // once dispatch catches up.
        self.parent.describe(name).await
    }

    /// Forwarded: extras add per-agent MCP tools, they do not add a gate.
    /// Every call — parent tool or extra — is executed by the parent, so the
    /// parent's tier is the one in force.
    fn enforced_exec_tier(&self) -> Option<crate::config::types::policies::ExecTier> {
        self.parent.enforced_exec_tier()
    }

    async fn call_concurrency_claim(
        &self,
        name: &str,
        input: &serde_json::Value,
    ) -> crate::tools::concurrency::ConcurrencyClaim {
        // Defer to the parent: it owns the authoritative bounded scope for
        // every tool it exposes.
        self.parent.call_concurrency_claim(name, input).await
    }

    fn metadata_schema(&self) -> Arc<[crate::tool_metadata::ToolDefinition]> {
        // Stage I MVP: pass through the parent's schema. Extras are not
        // dispatchable yet, so they cannot appear in the LLM-visible schema.
        self.parent.metadata_schema()
    }
}