//! P3 Stage I — `McpScopedToolService` layers per-agent MCP scope tools UNDER
//! the existing `AllowlistToolService` gate. Parent's tools take precedence;
//! `extras` fill in tools the parent doesn't expose.

use crate::sync_primitives::Arc;

use async_trait::async_trait;

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
}

#[async_trait]
impl ToolService for McpScopedToolService {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError> {
        // Parent first. Stage I MVP: extras execution deferred to Task 12.
        self.parent.execute(name, input).await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        let mut out = self.parent.list().await;
        let parent_names: std::collections::HashSet<String> =
            out.iter().map(|d| d.name.clone()).collect();
        for t in &self.extras {
            if !parent_names.contains(&t.name) {
                out.push(ToolDefinition {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.parameters.clone(),
                    source: ToolSource::Extension {
                        plugin_id: t.plugin_id.clone(),
                    },
                    metadata: ToolDefinitionMetadata::default(),
                });
            }
        }
        out
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
                metadata: ToolDefinitionMetadata::default(),
            })
    }

    async fn is_call_concurrent_safe(&self, name: &str, input: &serde_json::Value) -> bool {
        // Parent (typically AllowlistToolService → ScopedToolService) owns
        // the authoritative answer for any tool it exposes. Extras-only
        // entries currently route execute() through the parent too (Stage I
        // MVP), so deferring is correct here as well.
        self.parent.is_call_concurrent_safe(name, input).await
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
                metadata: ToolDefinitionMetadata::default(),
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
