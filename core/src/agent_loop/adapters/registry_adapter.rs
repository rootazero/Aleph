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
    /// Default working directory for bash/code_exec tools (agent workspace)
    default_working_dir: Option<String>,
}

/// Tools that should have working_dir injected when not specified by LLM
const WORKING_DIR_TOOLS: &[&str] = &["bash", "code_exec"];

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
        // Inject default working_dir for bash/code_exec if not provided by LLM
        let input = if WORKING_DIR_TOOLS.contains(&self.name.as_str()) {
            if let Some(ref dir) = self.default_working_dir {
                let mut obj = match input {
                    Value::Object(m) => m,
                    _ => serde_json::Map::new(),
                };
                // Inject workspace if working_dir is missing, null, or relative
                let should_inject = match obj.get("working_dir") {
                    None => true,
                    Some(v) if v.is_null() => true,
                    Some(Value::String(s)) => {
                        let s = s.trim();
                        s.is_empty() || s == "." || s == "./" || !s.starts_with('/')
                    }
                    _ => false,
                };
                if should_inject {
                    obj.insert("working_dir".to_string(), Value::String(dir.clone()));
                }
                Value::Object(obj)
            } else {
                input
            }
        } else {
            input
        };
        match self.registry.execute_tool(&self.name, input).await {
            Ok(output) => {
                // agent_switch should terminate the current loop so the next
                // message is routed to the newly-selected agent.
                if self.name == "agent_switch" {
                    ToolResult::SuccessAndStopLoop { output }
                } else {
                    ToolResult::Success { output }
                }
            }
            Err(e) => {
                tracing::warn!(tool = %self.name, error = %e, "Tool execution failed");
                ToolResult::Error {
                    error: e.to_string(),
                    retryable: true,
                }
            }
        }
    }
}

/// Build a `LoopToolRegistry` from an executor `ToolRegistry` + `UnifiedTool` list.
///
/// Each `UnifiedTool` becomes a `LoopTool` that delegates execution to the
/// shared `ToolRegistry`. Only active tools are included.
///
/// `default_working_dir` is injected into bash/code_exec tools when the LLM
/// doesn't specify a working_dir (defaults to agent workspace).
pub fn build_registry_from_tools<R: ToolRegistry + 'static>(
    tool_registry: Arc<R>,
    unified_tools: &[UnifiedTool],
    default_working_dir: Option<String>,
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
            default_working_dir: default_working_dir.clone(),
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

        let registry = build_registry_from_tools(tool_registry, &tools, None);
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

        let registry = build_registry_from_tools(tool_registry, &tools, None);
        let result = registry.execute("search", json!({"q": "rust"})).await;

        match result {
            ToolResult::Success { output } | ToolResult::SuccessAndStopLoop { output } => assert_eq!(output["found"], 42),
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

        let registry = build_registry_from_tools(tool_registry, &tools, None);
        assert_eq!(registry.len(), 1);
        assert!(registry.get("active").is_some());
        assert!(registry.get("disabled").is_none());
    }
}
