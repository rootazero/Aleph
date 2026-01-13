//! Native Tool Registry
//!
//! Manages registration and execution of native `AgentTool` implementations.
//! This registry stores tools and provides methods for:
//! - Tool registration
//! - Tool execution by name
//! - Tool definition retrieval for LLM
//! - Tool querying by category

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::traits::{AgentTool, ToolCategory, ToolDefinition, ToolResult};
use crate::error::{AetherError, Result};

// =============================================================================
// Native Tool Registry
// =============================================================================

/// Registry for native AgentTool implementations
///
/// Thread-safe registry that stores and manages native tools.
/// Provides methods for registration, execution, and querying.
///
/// # Usage
///
/// ```rust,ignore
/// let registry = NativeToolRegistry::new();
///
/// // Register tools
/// registry.register(Arc::new(FileReadTool::new(config))).await;
/// registry.register(Arc::new(GitStatusTool::new(config))).await;
///
/// // Execute a tool
/// let result = registry.execute("file_read", r#"{"path": "/tmp/test.txt"}"#).await?;
///
/// // Get all definitions for LLM
/// let definitions = registry.get_definitions().await;
/// ```
pub struct NativeToolRegistry {
    /// Tool storage: name -> Arc<dyn AgentTool>
    tools: Arc<RwLock<HashMap<String, Arc<dyn AgentTool>>>>,
}

impl Default for NativeToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // =========================================================================
    // Registration Methods
    // =========================================================================

    /// Register a native tool
    ///
    /// # Arguments
    ///
    /// * `tool` - Arc-wrapped AgentTool implementation
    ///
    /// # Returns
    ///
    /// The tool name that was registered
    pub async fn register(&self, tool: Arc<dyn AgentTool>) -> String {
        let name = tool.name().to_string();
        let mut tools = self.tools.write().await;

        if tools.contains_key(&name) {
            warn!("Overwriting existing tool: {}", name);
        }

        debug!(
            "Registering native tool: {} (category: {})",
            name,
            tool.category()
        );
        tools.insert(name.clone(), tool);
        name
    }

    /// Register multiple tools at once
    ///
    /// # Arguments
    ///
    /// * `tools` - Iterator of Arc-wrapped AgentTool implementations
    ///
    /// # Returns
    ///
    /// Number of tools registered
    pub async fn register_all<I>(&self, tools: I) -> usize
    where
        I: IntoIterator<Item = Arc<dyn AgentTool>>,
    {
        let mut count = 0;
        for tool in tools {
            self.register(tool).await;
            count += 1;
        }
        info!("Registered {} native tools", count);
        count
    }

    /// Unregister a tool by name
    ///
    /// # Returns
    ///
    /// `true` if tool was found and removed, `false` otherwise
    pub async fn unregister(&self, name: &str) -> bool {
        let mut tools = self.tools.write().await;
        let removed = tools.remove(name).is_some();
        if removed {
            debug!("Unregistered native tool: {}", name);
        }
        removed
    }

    /// Clear all registered tools
    pub async fn clear(&self) {
        let mut tools = self.tools.write().await;
        tools.clear();
        debug!("Cleared all native tools");
    }

    // =========================================================================
    // Incremental Update Methods (Phase 2.3)
    // =========================================================================

    /// Remove tools by category
    ///
    /// This enables incremental updates - only refresh tools of a specific category
    /// instead of clearing and re-registering everything.
    ///
    /// # Arguments
    ///
    /// * `category` - The tool category to remove
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_by_category(&self, category: ToolCategory) -> usize {
        let mut tools = self.tools.write().await;
        let initial_count = tools.len();

        tools.retain(|_, tool| tool.category() != category);

        let removed = initial_count - tools.len();
        debug!(
            category = ?category,
            removed = removed,
            "Removed tools by category"
        );
        removed
    }

    /// Remove tools whose names start with a specific prefix
    ///
    /// Useful for removing MCP server tools (format: "server_name:tool_name")
    ///
    /// # Arguments
    ///
    /// * `prefix` - The name prefix to match (e.g., "github:" for github server tools)
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_by_name_prefix(&self, prefix: &str) -> usize {
        let mut tools = self.tools.write().await;
        let initial_count = tools.len();

        tools.retain(|name, _| !name.starts_with(prefix));

        let removed = initial_count - tools.len();
        debug!(
            prefix = prefix,
            removed = removed,
            "Removed tools by name prefix"
        );
        removed
    }

    /// Remove all MCP tools (tools with ":" in the name)
    ///
    /// MCP tools follow the naming convention "server_name:tool_name"
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_mcp_tools(&self) -> usize {
        let mut tools = self.tools.write().await;
        let initial_count = tools.len();

        tools.retain(|name, _| !name.contains(':'));

        let removed = initial_count - tools.len();
        debug!(removed = removed, "Removed all MCP tools");
        removed
    }

    // =========================================================================
    // Execution Methods
    // =========================================================================

    /// Execute a tool by name with JSON arguments
    ///
    /// # Arguments
    ///
    /// * `name` - Tool name to execute
    /// * `args` - JSON string containing tool parameters
    ///
    /// # Returns
    ///
    /// * `Ok(ToolResult)` - Execution result
    /// * `Err(AetherError::ToolNotFound)` - Tool not registered
    pub async fn execute(&self, name: &str, args: &str) -> Result<ToolResult> {
        let tools = self.tools.read().await;

        let tool = tools.get(name).ok_or_else(|| AetherError::ToolNotFound {
            name: name.to_string(),
            suggestion: Some(self.suggest_similar_tool(name, &tools)),
        })?;

        debug!("Executing native tool: {} with args: {}", name, args);

        // Clone the Arc to release the read lock before execution
        let tool = Arc::clone(tool);
        drop(tools);

        tool.execute(args).await
    }

    /// Check if a tool requires confirmation before execution
    ///
    /// # Returns
    ///
    /// * `Some(true)` - Tool requires confirmation
    /// * `Some(false)` - Tool does not require confirmation
    /// * `None` - Tool not found
    pub async fn requires_confirmation(&self, name: &str) -> Option<bool> {
        let tools = self.tools.read().await;
        tools.get(name).map(|t| t.requires_confirmation())
    }

    // =========================================================================
    // Query Methods
    // =========================================================================

    /// Get a tool by name
    pub async fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        let tools = self.tools.read().await;
        tools.get(name).cloned()
    }

    /// Check if a tool is registered
    pub async fn contains(&self, name: &str) -> bool {
        let tools = self.tools.read().await;
        tools.contains_key(name)
    }

    /// Get all registered tool names
    pub async fn names(&self) -> Vec<String> {
        let tools = self.tools.read().await;
        tools.keys().cloned().collect()
    }

    /// Get the number of registered tools
    pub async fn count(&self) -> usize {
        let tools = self.tools.read().await;
        tools.len()
    }

    /// Get all tool definitions for LLM prompt generation
    ///
    /// Returns definitions sorted by category then name.
    pub async fn get_definitions(&self) -> Vec<ToolDefinition> {
        let tools = self.tools.read().await;
        let mut definitions: Vec<_> = tools.values().map(|t| t.definition()).collect();

        // Sort by category then name
        definitions.sort_by(|a, b| {
            a.category
                .display_name()
                .cmp(b.category.display_name())
                .then(a.name.cmp(&b.name))
        });

        definitions
    }

    /// Get tool definitions filtered by category
    pub async fn get_definitions_by_category(&self, category: ToolCategory) -> Vec<ToolDefinition> {
        let tools = self.tools.read().await;
        let mut definitions: Vec<_> = tools
            .values()
            .filter(|t| t.category() == category)
            .map(|t| t.definition())
            .collect();

        definitions.sort_by(|a, b| a.name.cmp(&b.name));
        definitions
    }

    /// Get tools that require confirmation
    pub async fn get_confirmation_tools(&self) -> Vec<ToolDefinition> {
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|t| t.requires_confirmation())
            .map(|t| t.definition())
            .collect()
    }

    /// Convert all definitions to OpenAI function calling format
    pub async fn to_openai_tools(&self) -> Vec<serde_json::Value> {
        self.get_definitions()
            .await
            .into_iter()
            .map(|d| d.to_openai_function())
            .collect()
    }

    /// Convert all definitions to Anthropic tool format
    pub async fn to_anthropic_tools(&self) -> Vec<serde_json::Value> {
        self.get_definitions()
            .await
            .into_iter()
            .map(|d| d.to_anthropic_tool())
            .collect()
    }

    // =========================================================================
    // Helper Methods
    // =========================================================================

    /// Suggest a similar tool name for error messages
    fn suggest_similar_tool(&self, name: &str, tools: &HashMap<String, Arc<dyn AgentTool>>) -> String {
        let name_lower = name.to_lowercase();

        // Find tools with similar names
        let suggestions: Vec<_> = tools
            .keys()
            .filter(|k| {
                let k_lower = k.to_lowercase();
                k_lower.contains(&name_lower) || name_lower.contains(&k_lower)
            })
            .take(3)
            .cloned()
            .collect();

        if suggestions.is_empty() {
            format!(
                "Available tools: {}",
                tools.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        } else {
            format!("Did you mean: {}?", suggestions.join(", "))
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// Mock tool for testing
    struct MockTool {
        name: String,
        category: ToolCategory,
        requires_confirmation: bool,
    }

    impl MockTool {
        fn new(name: &str, category: ToolCategory) -> Self {
            Self {
                name: name.to_string(),
                category,
                requires_confirmation: false,
            }
        }

        fn with_confirmation(mut self, requires: bool) -> Self {
            self.requires_confirmation = requires;
            self
        }
    }

    #[async_trait]
    impl AgentTool for MockTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                &self.name,
                format!("Mock {} tool", self.name),
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                self.category,
            )
            .with_confirmation(self.requires_confirmation)
        }

        async fn execute(&self, args: &str) -> Result<ToolResult> {
            Ok(ToolResult::success(format!(
                "Executed {} with args: {}",
                self.name, args
            )))
        }

        fn requires_confirmation(&self) -> bool {
            self.requires_confirmation
        }

        fn category(&self) -> ToolCategory {
            self.category
        }
    }

    #[tokio::test]
    async fn test_registry_new() {
        let registry = NativeToolRegistry::new();
        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn test_register_tool() {
        let registry = NativeToolRegistry::new();
        let tool = Arc::new(MockTool::new("test", ToolCategory::Native));

        let name = registry.register(tool).await;

        assert_eq!(name, "test");
        assert_eq!(registry.count().await, 1);
        assert!(registry.contains("test").await);
    }

    #[tokio::test]
    async fn test_register_all() {
        let registry = NativeToolRegistry::new();
        let tools: Vec<Arc<dyn AgentTool>> = vec![
            Arc::new(MockTool::new("tool1", ToolCategory::Native)),
            Arc::new(MockTool::new("tool2", ToolCategory::Native)),
            Arc::new(MockTool::new("tool3", ToolCategory::Native)),
        ];

        let count = registry.register_all(tools).await;

        assert_eq!(count, 3);
        assert_eq!(registry.count().await, 3);
    }

    #[tokio::test]
    async fn test_unregister_tool() {
        let registry = NativeToolRegistry::new();
        registry
            .register(Arc::new(MockTool::new("test", ToolCategory::Native)))
            .await;

        assert!(registry.unregister("test").await);
        assert!(!registry.contains("test").await);
        assert!(!registry.unregister("test").await); // Already removed
    }

    #[tokio::test]
    async fn test_execute_tool() {
        let registry = NativeToolRegistry::new();
        registry
            .register(Arc::new(MockTool::new("test", ToolCategory::Native)))
            .await;

        let result = registry.execute("test", r#"{"key": "value"}"#).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.is_success());
        assert!(result.content.contains("test"));
    }

    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let registry = NativeToolRegistry::new();

        let result = registry.execute("unknown", "{}").await;

        assert!(result.is_err());
        match result {
            Err(AetherError::ToolNotFound { name, .. }) => {
                assert_eq!(name, "unknown");
            }
            _ => panic!("Expected ToolNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_get_definitions() {
        let registry = NativeToolRegistry::new();
        registry
            .register(Arc::new(MockTool::new("b_tool", ToolCategory::Native)))
            .await;
        registry
            .register(Arc::new(MockTool::new("a_tool", ToolCategory::Native)))
            .await;

        let definitions = registry.get_definitions().await;

        assert_eq!(definitions.len(), 2);
        // Should be sorted by category then name
        assert_eq!(definitions[0].name, "a_tool"); // Filesystem comes before Git
        assert_eq!(definitions[1].name, "b_tool");
    }

    #[tokio::test]
    async fn test_get_definitions_by_category() {
        let registry = NativeToolRegistry::new();
        registry
            .register(Arc::new(MockTool::new("native1", ToolCategory::Native)))
            .await;
        registry
            .register(Arc::new(MockTool::new("native2", ToolCategory::Native)))
            .await;
        registry
            .register(Arc::new(MockTool::new("builtin1", ToolCategory::Builtin)))
            .await;

        let native_tools = registry
            .get_definitions_by_category(ToolCategory::Native)
            .await;

        assert_eq!(native_tools.len(), 2);
        assert!(native_tools.iter().all(|d| d.category == ToolCategory::Native));
    }

    #[tokio::test]
    async fn test_get_confirmation_tools() {
        let registry = NativeToolRegistry::new();
        registry
            .register(Arc::new(
                MockTool::new("safe", ToolCategory::Native).with_confirmation(false),
            ))
            .await;
        registry
            .register(Arc::new(
                MockTool::new("dangerous", ToolCategory::Native).with_confirmation(true),
            ))
            .await;

        let confirmation_tools = registry.get_confirmation_tools().await;

        assert_eq!(confirmation_tools.len(), 1);
        assert_eq!(confirmation_tools[0].name, "dangerous");
    }

    #[tokio::test]
    async fn test_requires_confirmation() {
        let registry = NativeToolRegistry::new();
        registry
            .register(Arc::new(
                MockTool::new("safe", ToolCategory::Native).with_confirmation(false),
            ))
            .await;
        registry
            .register(Arc::new(
                MockTool::new("dangerous", ToolCategory::Native).with_confirmation(true),
            ))
            .await;

        assert_eq!(registry.requires_confirmation("safe").await, Some(false));
        assert_eq!(registry.requires_confirmation("dangerous").await, Some(true));
        assert_eq!(registry.requires_confirmation("unknown").await, None);
    }

    #[tokio::test]
    async fn test_to_openai_tools() {
        let registry = NativeToolRegistry::new();
        registry
            .register(Arc::new(MockTool::new("test", ToolCategory::Native)))
            .await;

        let tools = registry.to_openai_tools().await;

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "test");
    }

    #[tokio::test]
    async fn test_to_anthropic_tools() {
        let registry = NativeToolRegistry::new();
        registry
            .register(Arc::new(MockTool::new("test", ToolCategory::Native)))
            .await;

        let tools = registry.to_anthropic_tools().await;

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "test");
        assert!(tools[0]["input_schema"].is_object());
    }

    #[tokio::test]
    async fn test_names() {
        let registry = NativeToolRegistry::new();
        registry
            .register(Arc::new(MockTool::new("a", ToolCategory::Native)))
            .await;
        registry
            .register(Arc::new(MockTool::new("b", ToolCategory::Native)))
            .await;

        let mut names = registry.names().await;
        names.sort();

        assert_eq!(names, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn test_clear() {
        let registry = NativeToolRegistry::new();
        registry
            .register(Arc::new(MockTool::new("test", ToolCategory::Native)))
            .await;

        registry.clear().await;

        assert_eq!(registry.count().await, 0);
    }
}
