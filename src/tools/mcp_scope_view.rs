//! P3 Stage I — `McpScopedToolService` layers per-agent MCP scope tools UNDER
//! the existing `AllowlistToolService` gate. Parent's tools take precedence;
//! `extras` fill in tools the parent doesn't expose.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::extension::registry::ToolRegistration;
use crate::session::events::ToolOutput;
use crate::tools::service::{
    to_metadata_form, ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource,
};

pub struct McpScopedToolService {
    parent: Arc<dyn ToolService>,
    extras: Vec<ToolRegistration>,
}

impl McpScopedToolService {
    pub fn new(parent: Arc<dyn ToolService>, extras: Vec<ToolRegistration>) -> Self {
        Self { parent, extras }
    }

    /// Metadata for an extras-only (plugin-provided) tool. The one field that
    /// must not stay default is the wall-clock budget: a definition without one
    /// made the harness treat a slow call as a run-level stall instead of a
    /// recoverable tool error. Plugins declare no budget, so this resolves to
    /// the global default.
    fn extras_metadata(name: &str) -> ToolDefinitionMetadata {
        ToolDefinitionMetadata {
            max_duration_ms: Some(crate::tools::budget::resolve_tool_budget_ms(name, None)),
            ..ToolDefinitionMetadata::default()
        }
    }

    /// Append extras (per-agent MCP-scope tools) that don't shadow a parent
    /// definition. Shared by `list()` and `dispatchable_list()` so the two
    /// views cannot drift on extras handling.
    fn merge_extras(
        mut out: Vec<ToolDefinition>,
        extras: &[ToolRegistration],
    ) -> Vec<ToolDefinition> {
        let parent_names: std::collections::HashSet<String> =
            out.iter().map(|d| d.name.clone()).collect();
        for t in extras {
            if !parent_names.contains(&t.name) {
                out.push(ToolDefinition {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.parameters.clone(),
                    source: ToolSource::Extension {
                        plugin_id: t.plugin_id.clone(),
                    },
                    metadata: Self::extras_metadata(&t.name),
                });
            }
        }
        out
    }
}

#[async_trait]
impl ToolService for McpScopedToolService {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError> {
        // Parent first. Stage I MVP: extras execution deferred to Task 12.
        self.parent.execute(name, input).await
    }

    async fn execute_with_cancel(
        &self,
        name: &str,
        input: serde_json::Value,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        // Delegate to the parent's cancel-aware path so the inner
        // `ScopedToolService` actually threads the token into `LoopTool::execute`.
        self.parent.execute_with_cancel(name, input, cancel).await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        Self::merge_extras(self.parent.list().await, &self.extras)
    }

    async fn dispatchable_list(&self) -> Vec<ToolDefinition> {
        // Forward the parent's dispatchable set (visible + deferred tier)
        // instead of the trait default (`list()`), which silently drops the
        // parent `ScopedToolService`'s deferred MCP names from the
        // name-repairer's candidate set — a correct call to a deferred tool
        // would miss the Exact tier and could be fuzzily rewritten into a
        // different resident tool. Extras merge identically to `list()`.
        Self::merge_extras(self.parent.dispatchable_list().await, &self.extras)
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        if let Some(d) = self.parent.describe(name).await {
            return Some(d);
        }
        self.extras
            .iter()
            .find(|t| t.name == name)
            .map(|t| ToolDefinition {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
                source: ToolSource::Extension {
                    plugin_id: t.plugin_id.clone(),
                },
                metadata: Self::extras_metadata(&t.name),
            })
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
        // every tool it exposes. Extras-only entries currently route execute()
        // through the parent too (Stage I MVP), so deferring is correct here.
        self.parent.call_concurrency_claim(name, input).await
    }

    fn metadata_schema(&self) -> Arc<[crate::tool_metadata::ToolDefinition]> {
        // For Stage I MVP, compute merged schema from list snapshot.
        // AllowlistToolService above us gates which tools the LLM actually sees,
        // so extras not in the allowlist are filtered out there.
        let parent_schema = self.parent.metadata_schema();
        if self.extras.is_empty() {
            return parent_schema;
        }
        // Collect parent names so extras don't shadow existing tools.
        let parent_names: std::collections::HashSet<String> =
            parent_schema.iter().map(|t| t.name.clone()).collect();
        // Build extras defs and convert to metadata form, then merge.
        let extra_defs: Vec<ToolDefinition> = self
            .extras
            .iter()
            .filter(|t| !parent_names.contains(&t.name))
            .map(|t| ToolDefinition {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
                source: ToolSource::Extension {
                    plugin_id: t.plugin_id.clone(),
                },
                metadata: Self::extras_metadata(&t.name),
            })
            .collect();
        if extra_defs.is_empty() {
            return parent_schema;
        }
        let extra_schema = to_metadata_form(&extra_defs);
        let mut merged: Vec<crate::tool_metadata::ToolDefinition> =
            parent_schema.iter().cloned().collect();
        merged.extend(extra_schema.iter().cloned());
        merged.into()
    }
}
