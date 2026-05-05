//! ToolService — consumer-side façade over tool dispatch.
//!
//! See: docs/superpowers/specs/2026-04-18-tool-service-facade-design.md

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::session::events::ToolOutput;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool not found: {name}")]
    NotFound { name: String },

    #[error("permission denied for tool {name}: {reason}")]
    PermissionDenied { name: String, reason: String },

    #[error("invalid input for tool {name}: {cause}")]
    ValidationFailed { name: String, cause: String },

    #[error("tool {name} execution failed: {cause}")]
    Execution { name: String, cause: String },

    #[error("tool {name} timed out after {elapsed_ms}ms")]
    Timeout { name: String, elapsed_ms: u64 },

    #[error("tool {name} transport error: {cause}")]
    Transport { name: String, cause: String },

    #[error("duplicate tool name: {name}")]
    Duplicate { name: String },

    #[error("{0}")]
    Other(String),
}

impl ToolError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout { .. } | Self::Transport { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolSource {
    Builtin,
    Mcp { server_id: String },
    Extension { plugin_id: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinitionMetadata {
    #[serde(default)]
    pub hidden_from_llm: bool,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub source: ToolSource,
    #[serde(default)]
    pub metadata: ToolDefinitionMetadata,
}

#[async_trait]
pub trait ToolService: Send + Sync + 'static {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError>;

    async fn list(&self) -> Vec<ToolDefinition>;

    async fn describe(&self, name: &str) -> Option<ToolDefinition>;

    /// Return the dispatcher-form tool schema the LLM expects, as an `Arc`
    /// for O(1) per-turn cloning. Implementations cache internally and
    /// invalidate on their own mutation signal (e.g., registry snapshot
    /// change for `CoreDispatch`, MCP `poll_changes()` for `ScopedToolService`).
    ///
    /// REQUIRED — no default impl. A default returning empty would silently
    /// hide the LLM's tool list on any forgotten override. Test mocks must
    /// also implement, typically returning `std::sync::Arc::from([])`.
    fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]>;
}

/// Convert a slice of loop-side `ToolDefinition`s into the dispatcher-side
/// `ToolDefinition` representation expected by LLM providers.
///
/// This is the single source of truth for the conversion. Per Stage 2
/// of the 12-module roadmap, `ToolService` impls cache the output of this
/// helper (keyed on their internal mutation signal) so each turn's tool
/// list is an O(1) `Arc::clone` rather than an O(n) `Vec` allocation.
///
/// Information loss (e.g., `ToolSource::Mcp` collapses to `category: Builtin`,
/// `metadata.requires_approval` is dropped) is preserved as-is from the
/// pre-Stage-2 behavior. Fixing the lossy mapping is out of Stage 2 scope.
pub fn to_dispatcher_form(defs: &[ToolDefinition]) -> Arc<[crate::dispatcher::ToolDefinition]> {
    defs.iter()
        .map(|def| crate::dispatcher::ToolDefinition {
            name: def.name.clone(),
            description: def.description.clone(),
            parameters: def.input_schema.clone(),
            requires_confirmation: false,
            category: crate::dispatcher::ToolCategory::Builtin,
            llm_context: None,
            strict: false,
        })
        .collect::<Vec<_>>()
        .into()
}

#[cfg(test)]
mod dispatcher_form_tests {
    use super::*;
    use crate::dispatcher::{ToolCategory, ToolDefinition as DispatcherToolDefinition};
    use serde_json::json;

    fn loop_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("desc {name}"),
            input_schema: json!({"type": "object"}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    #[test]
    fn empty_input_yields_empty_arc() {
        let out = to_dispatcher_form(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn single_def_converts_field_by_field() {
        let inputs = vec![loop_def("alpha")];
        let out = to_dispatcher_form(&inputs);
        assert_eq!(out.len(), 1);
        let d: &DispatcherToolDefinition = &out[0];
        assert_eq!(d.name, "alpha");
        assert_eq!(d.description, "desc alpha");
        assert_eq!(d.parameters, json!({"type": "object"}));
        assert!(!d.requires_confirmation);
        assert!(matches!(d.category, ToolCategory::Builtin));
        assert!(d.llm_context.is_none());
        assert!(!d.strict);
    }

    #[test]
    fn preserves_order_for_multi_input() {
        let inputs = vec![loop_def("a"), loop_def("b"), loop_def("c")];
        let out = to_dispatcher_form(&inputs);
        let names: Vec<&str> = out.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }
}
