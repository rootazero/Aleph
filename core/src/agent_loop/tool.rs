//! LoopTool trait and LoopToolRegistry.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// =============================================================================
// ToolResult
// =============================================================================

/// Outcome of a tool execution.
#[derive(Debug, Clone)]
pub enum ToolResult {
    Success { output: Value },
    Error { error: String, retryable: bool },
}

// =============================================================================
// ToolDefinition (local, minimal)
// =============================================================================

/// Lightweight tool definition for LLM function calling.
///
/// Intentionally simpler than `crate::dispatcher::ToolDefinition` —
/// no category, confirmation, or strict-mode fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

// =============================================================================
// LoopTool trait
// =============================================================================

/// Unified tool trait — one trait to rule them all.
///
/// Every tool (built-in, MCP, skill, extension) implements this single trait.
/// Schema is returned as a raw JSON value so tools can describe themselves
/// without pulling in heavy schema crates.
#[async_trait]
pub trait LoopTool: Send + Sync {
    /// Unique tool name used in function calls.
    fn name(&self) -> &str;

    /// Human-readable description for LLM.
    fn description(&self) -> &str;

    /// JSON Schema for input parameters.
    fn schema(&self) -> Value;

    /// Execute the tool with the given input.
    async fn execute(&self, input: Value) -> ToolResult;
}

// =============================================================================
// LoopToolRegistry
// =============================================================================

/// Flat registry mapping tool names to trait objects.
pub struct LoopToolRegistry {
    tools: HashMap<String, Box<dyn LoopTool>>,
}

impl LoopToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool. Overwrites any existing tool with the same name.
    pub fn register(&mut self, tool: Box<dyn LoopTool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn LoopTool> {
        self.tools.get(name).map(|b| b.as_ref())
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Execute a tool by name.
    pub async fn execute(&self, name: &str, input: Value) -> ToolResult {
        match self.get(name) {
            Some(tool) => tool.execute(input).await,
            None => ToolResult::Error {
                error: format!("unknown tool: {}", name),
                retryable: false,
            },
        }
    }

    /// Collect definitions for all registered tools (sorted by name for determinism).
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self
            .tools
            .values()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.schema(),
            })
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }
}

impl Default for LoopToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A trivial echo tool for testing.
    struct EchoTool;

    #[async_trait]
    impl LoopTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes the input back"
        }
        fn schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }
        async fn execute(&self, input: Value) -> ToolResult {
            ToolResult::Success { output: input }
        }
    }

    /// A tool that always fails — for testing error paths.
    struct FailTool;

    #[async_trait]
    impl LoopTool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::Error {
                error: "intentional failure".into(),
                retryable: true,
            }
        }
    }

    #[tokio::test]
    async fn test_minimal_tool_execute() {
        let tool = EchoTool;
        let input = json!({ "message": "hello" });
        let result = tool.execute(input.clone()).await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output, input);
            }
            ToolResult::Error { .. } => panic!("expected success"),
        }
    }

    #[tokio::test]
    async fn test_minimal_tool_registry() {
        let mut registry = LoopToolRegistry::new();
        assert!(registry.is_empty());

        registry.register(Box::new(EchoTool));
        registry.register(Box::new(FailTool));
        assert_eq!(registry.len(), 2);

        // Get existing tool
        assert!(registry.get("echo").is_some());
        assert_eq!(registry.get("echo").unwrap().name(), "echo");

        // Get non-existent tool
        assert!(registry.get("nope").is_none());

        // Execute existing tool
        let result = registry.execute("echo", json!({ "message": "hi" })).await;
        match result {
            ToolResult::Success { output } => {
                assert_eq!(output, json!({ "message": "hi" }));
            }
            ToolResult::Error { .. } => panic!("expected success"),
        }

        // Execute unknown tool
        let result = registry.execute("unknown", json!({})).await;
        match result {
            ToolResult::Error {
                error, retryable, ..
            } => {
                assert!(error.contains("unknown tool"));
                assert!(!retryable);
            }
            ToolResult::Success { .. } => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn test_registry_schemas() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry.register(Box::new(FailTool));

        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 2);

        // Sorted by name: echo < fail
        assert_eq!(defs[0].name, "echo");
        assert_eq!(defs[0].description, "Echoes the input back");
        assert_eq!(
            defs[0].parameters["required"],
            json!(["message"])
        );

        assert_eq!(defs[1].name, "fail");
        assert_eq!(defs[1].description, "Always fails");
    }
}
