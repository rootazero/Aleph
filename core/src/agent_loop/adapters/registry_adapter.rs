//! Adapter bridging executor::ToolRegistry to LoopTool.
//!
//! Wraps `ToolRegistry::execute_tool()` + `UnifiedTool` metadata into
//! LoopTool instances for use in the agent loop.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::dispatcher::UnifiedTool;
use crate::executor::ToolRegistry;

use super::super::tool::{LoopTool, LoopToolRegistry, ToolResult};

/// A LoopTool backed by a shared ToolRegistry.
///
/// Each instance holds the metadata from a `UnifiedTool` (name, description,
/// schema) and delegates execution to `ToolRegistry::execute_tool()`.
struct RegistryToolAdapter<R: ToolRegistry + 'static> {
    name: String,
    description: String,
    schema: Value,
    registry: Arc<R>,
}

#[async_trait]
impl<R: ToolRegistry + 'static> LoopTool for RegistryToolAdapter<R> {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    async fn execute(&self, input: Value) -> ToolResult {
        match self.registry.execute_tool(&self.name, input).await {
            Ok(output) => ToolResult::Success { output },
            Err(e) => ToolResult::Error {
                error: e.to_string(),
                retryable: true,
            },
        }
    }
}

/// Build a `LoopToolRegistry` from an executor `ToolRegistry` + `UnifiedTool` list.
///
/// Each `UnifiedTool` becomes a `LoopTool` that delegates execution to the
/// shared `ToolRegistry`. Only active tools are included.
pub fn build_registry_from_tools<R: ToolRegistry + 'static>(
    tool_registry: Arc<R>,
    unified_tools: &[UnifiedTool],
) -> LoopToolRegistry {
    let mut registry = LoopToolRegistry::new();

    for tool in unified_tools {
        if !tool.is_active {
            continue;
        }

        let schema = tool
            .parameters_schema
            .clone()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}}));

        registry.register(Box::new(RegistryToolAdapter {
            name: tool.name.clone(),
            description: tool.description.clone(),
            schema,
            registry: Arc::clone(&tool_registry),
        }));
    }

    registry
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;
    use crate::executor::ToolRegistry;
    use serde_json::json;
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;

    /// Mock ToolRegistry for testing.
    struct MockRegistry {
        results: HashMap<String, Value>,
    }

    impl ToolRegistry for MockRegistry {
        fn get_tool(&self, _name: &str) -> Option<&UnifiedTool> {
            None // Not needed for execution
        }

        fn execute_tool(
            &self,
            tool_name: &str,
            _arguments: Value,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<Value>> + Send + '_>> {
            let result = self.results.get(tool_name).cloned();
            let name = tool_name.to_string();
            Box::pin(async move {
                result.ok_or_else(|| crate::error::AlephError::tool_not_found(&name))
            })
        }
        // workspace_handle, smart_recall_config_handle, session_context_handle,
        // tool_policy_handle all have default implementations returning None
    }

    fn make_unified_tool(name: &str, desc: &str) -> UnifiedTool {
        let mut tool = UnifiedTool::new(
            format!("native:{}", name),
            name,
            desc,
            ToolSource::Native,
        );
        tool.parameters_schema = Some(json!({"type": "object", "properties": {}}));
        tool
    }

    #[tokio::test]
    async fn test_build_registry_from_tools() {
        let mut results = HashMap::new();
        results.insert("search".to_string(), json!({"found": 42}));

        let tool_registry = Arc::new(MockRegistry { results });
        let tools = vec![
            make_unified_tool("search", "Search for things"),
            make_unified_tool("memory", "Query memory"),
        ];

        let registry = build_registry_from_tools(tool_registry, &tools);
        assert_eq!(registry.len(), 2);
        assert!(registry.get("search").is_some());
        assert!(registry.get("memory").is_some());
    }

    #[tokio::test]
    async fn test_registry_adapter_execute() {
        let mut results = HashMap::new();
        results.insert("search".to_string(), json!({"found": 42}));

        let tool_registry = Arc::new(MockRegistry { results });
        let tools = vec![make_unified_tool("search", "Search")];

        let registry = build_registry_from_tools(tool_registry, &tools);
        let result = registry.execute("search", json!({"q": "rust"})).await;

        match result {
            ToolResult::Success { output } => assert_eq!(output["found"], 42),
            ToolResult::Error { error, .. } => panic!("expected success: {}", error),
        }
    }

    #[tokio::test]
    async fn test_inactive_tools_excluded() {
        let tool_registry = Arc::new(MockRegistry {
            results: HashMap::new(),
        });

        let mut inactive = make_unified_tool("disabled", "Should not appear");
        inactive.is_active = false;

        let tools = vec![
            make_unified_tool("active", "Active tool"),
            inactive,
        ];

        let registry = build_registry_from_tools(tool_registry, &tools);
        assert_eq!(registry.len(), 1);
        assert!(registry.get("active").is_some());
        assert!(registry.get("disabled").is_none());
    }
}
