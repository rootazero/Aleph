//! Conversion and utility methods for `UnifiedTool`
//!
//! Includes tool index generation, safety level inference, and prompt line
//! formatting.

use super::UnifiedTool;
use crate::tool_metadata::types::conflict::ToolSource;
use crate::tool_metadata::types::index::{truncate_string, ToolIndexCategory, ToolIndexEntry};

impl UnifiedTool {
    // =========================================================================
    // Tool Index Methods (Smart Tool Discovery)
    // =========================================================================

    /// Convert to lightweight index entry for smart discovery
    ///
    /// Creates a minimal representation suitable for LLM prompt injection.
    /// The summary is truncated to 50 characters for token efficiency.
    ///
    /// # Arguments
    ///
    /// * `core_tools` - List of tool names that should be marked as core
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let tool = UnifiedTool::new(...);
    /// let entry = tool.to_index_entry(&["search", "file_ops"]);
    /// ```
    #[must_use]
    pub fn to_index_entry(&self, core_tools: &[&str]) -> ToolIndexEntry {
        let category = ToolIndexCategory::from(&self.source);
        let summary = truncate_string(&self.description, 50);

        // Extract keywords from name and description
        let mut keywords = Vec::new();

        // Add name parts as keywords
        for part in self.name.split([':', '_', '-']) {
            if part.len() > 2 {
                keywords.push(part.to_lowercase());
            }
        }

        // Check if this is a core tool
        let is_core = core_tools.contains(&self.name.as_str());

        ToolIndexEntry {
            name: self.name.clone(),
            category: if is_core {
                ToolIndexCategory::Core
            } else {
                category
            },
            summary,
            keywords,
            is_core,
        }
    }

    /// Format tool for LLM prompt inclusion
    ///
    /// Returns a markdown-formatted line for system prompt injection.
    /// Builtin and Native tools are marked as "Preferred" to guide L3 routing priority.
    #[must_use]
    pub fn to_prompt_line(&self) -> String {
        let source_badge = match &self.source {
            ToolSource::Native => " [Native - Preferred]".to_string(),
            ToolSource::Builtin => " [Builtin - Preferred]".to_string(),
            ToolSource::Mcp { server } => format!(" [MCP:{server}]"),
            ToolSource::Skill { id } => format!(" [Skill:{id}]"),
            ToolSource::Custom { .. } => " [Custom]".to_string(),
            ToolSource::Plugin { plugin_id } => format!(" [Plugin:{plugin_id}]"),
        };

        let params = match &self.parameters_schema {
            Some(schema) => {
                // Extract parameter hints from schema
                if let Some(props) = schema.get("properties") {
                    let hints: Vec<String> = props
                        .as_object()
                        .map(|obj| obj.keys().cloned().collect())
                        .unwrap_or_default();
                    if !hints.is_empty() {
                        format!(" (args: {})", hints.join(", "))
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            }
            None => String::new(),
        };

        format!(
            "- **{}**{}: {}{}",
            self.name, source_badge, self.description, params
        )
    }
}
