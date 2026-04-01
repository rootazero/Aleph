//! Smart Tool Discovery Methods
//!
//! Methods for generating tool indices and smart prompts.

use super::super::types::{ToolIndex, ToolIndexCategory, ToolIndexEntry, UnifiedTool};
use super::types::ToolStorage;

/// Smart discovery functionality for ToolRegistry
pub struct ToolDiscovery {
    tools: ToolStorage,
}

impl ToolDiscovery {
    /// Create a new discovery handler with the given storage
    pub fn new(tools: ToolStorage) -> Self {
        Self { tools }
    }

    /// Generate tool list for LLM prompt
    ///
    /// Returns a markdown-formatted list of all active tools
    /// suitable for injection into L3 router system prompt.
    pub async fn to_prompt_block(&self) -> String {
        let tools = self.tools.read().await;
        let mut lines: Vec<String> = tools
            .values()
            .filter(|t| t.is_active)
            .map(|t| t.to_prompt_line())
            .collect();

        lines.sort(); // Alphabetical order
        lines.join("\n")
    }

    /// Generate lightweight tool index for smart discovery
    ///
    /// Creates a `ToolIndex` containing minimal metadata for all tools.
    /// This is used for token-efficient LLM prompt injection.
    ///
    /// # Arguments
    ///
    /// * `core_tools` - List of tool names that should be marked as core
    pub async fn generate_tool_index(&self, core_tools: &[&str]) -> ToolIndex {
        let tools = self.tools.read().await;
        let mut index = ToolIndex::new();

        for tool in tools.values().filter(|t| t.is_active) {
            let entry = tool.to_index_entry(core_tools);
            index.add(entry);
        }

        index
    }

    /// Generate smart prompt with tool index + filtered full schemas
    ///
    /// This is the main entry point for smart tool discovery.
    /// Returns a prompt that contains:
    /// 1. Full schemas for core tools
    /// 2. Full schemas for filtered tools (if any)
    /// 3. Index-only entries for remaining tools
    ///
    /// # Arguments
    ///
    /// * `core_tools` - Tools that always have full schema
    /// * `filtered_tools` - Additional tools to include with full schema
    ///
    /// # Returns
    ///
    /// Tuple of (tool_definitions, tool_index_prompt)
    /// - tool_definitions: Vec of tools with full schema for function calling
    /// - tool_index_prompt: Markdown prompt for index-only tools
    pub async fn generate_smart_prompt(
        &self,
        core_tools: &[&str],
        filtered_tools: &[&str],
    ) -> (Vec<UnifiedTool>, String) {
        let tools = self.tools.read().await;

        let mut full_schema_tools = Vec::new();
        let mut index = ToolIndex::new();

        for tool in tools.values().filter(|t| t.is_active) {
            let is_core = core_tools.contains(&tool.name.as_str());
            let is_filtered = filtered_tools.contains(&tool.name.as_str());

            if is_core || is_filtered {
                // Include with full schema
                full_schema_tools.push(tool.clone());
            } else {
                // Index only
                let entry = tool.to_index_entry(core_tools);
                index.add(entry);
            }
        }

        // Sort full schema tools by priority
        full_schema_tools.sort_by(|a, b| {
            let priority_a = a.source.priority();
            let priority_b = b.source.priority();
            priority_b.cmp(&priority_a).then(a.name.cmp(&b.name))
        });

        (full_schema_tools, index.to_prompt())
    }

    /// Get full tool definition by name
    ///
    /// Used by the `get_tool_schema` meta tool to provide on-demand schema.
    ///
    /// # Arguments
    ///
    /// * `name` - Tool command name
    ///
    /// # Returns
    ///
    /// Full UnifiedTool if found, None otherwise
    pub async fn get_tool_definition(&self, name: &str) -> Option<UnifiedTool> {
        let tools = self.tools.read().await;
        tools
            .values()
            .find(|t| t.name == name || t.id.ends_with(&format!(":{}", name)))
            .cloned()
    }

    /// List tools by category for the `list_tools` meta tool
    ///
    /// # Arguments
    ///
    /// * `category` - Optional category filter (core, builtin, mcp, skill, custom)
    ///
    /// # Returns
    ///
    /// Vector of tool index entries matching the category
    pub async fn list_tools_by_category(&self, category: Option<&str>) -> Vec<ToolIndexEntry> {
        let tools = self.tools.read().await;
        let core_tools: Vec<&str> = vec![]; // Empty for listing

        tools
            .values()
            .filter(|t| t.is_active)
            .filter(|t| {
                if let Some(cat) = category {
                    let tool_cat = ToolIndexCategory::from(&t.source);
                    tool_cat.display_name().to_lowercase() == cat.to_lowercase()
                } else {
                    true
                }
            })
            .map(|t| t.to_index_entry(&core_tools))
            .collect()
    }
}
