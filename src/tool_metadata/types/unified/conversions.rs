//! Conversion and utility methods for UnifiedTool
//!
//! Includes tool index generation, safety level inference, and prompt line
//! formatting.

use super::UnifiedTool;
use crate::config::ToolSafetyPolicy;
use crate::tool_metadata::types::category::ToolCategory;
use crate::tool_metadata::types::conflict::ToolSource;
use crate::tool_metadata::types::index::{truncate_string, ToolIndexCategory, ToolIndexEntry};
use crate::tool_metadata::types::safety::ToolSafetyLevel;

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

    /// Infer safety level from tool name and category
    ///
    /// Uses heuristics based on common tool naming patterns:
    /// - Read-only: search, query, get, read, list, show, view
    /// - Reversible: create, copy, move, rename, update, set
    /// - Irreversible Low Risk: send, notify, post, publish
    /// - Irreversible High Risk: delete, remove, drop, execute, run, shell
    ///
    /// If a `ToolSafetyPolicy` is provided, uses configurable keywords from policy.
    /// Otherwise, uses hardcoded defaults for backward compatibility.
    #[must_use]
    pub fn infer_safety_level(
        name: &str,
        category: ToolCategory,
        policy: Option<&ToolSafetyPolicy>,
    ) -> ToolSafetyLevel {
        // Use provided policy or default
        let default_policy = ToolSafetyPolicy::default();
        let policy = policy.unwrap_or(&default_policy);

        // Check keyword-based classification using policy
        if policy.is_high_risk(name) {
            return ToolSafetyLevel::IrreversibleHighRisk;
        }

        if policy.is_low_risk(name) {
            return ToolSafetyLevel::IrreversibleLowRisk;
        }

        if policy.is_reversible(name) {
            return ToolSafetyLevel::Reversible;
        }

        if policy.is_readonly(name) {
            return ToolSafetyLevel::ReadOnly;
        }

        // Fall back to category-based inference using policy fallbacks
        let fallback_str = match category {
            ToolCategory::Builtin => &policy.builtin_fallback,
            ToolCategory::Skills => &policy.skill_fallback,
            ToolCategory::Mcp => &policy.mcp_fallback,
            ToolCategory::Custom => &policy.custom_fallback,
            ToolCategory::GeneratedSkill => &policy.custom_fallback,
        };

        // Convert fallback string to ToolSafetyLevel
        Self::parse_safety_level_str(policy.parse_safety_level(fallback_str))
    }

    /// Parse safety level string to enum
    fn parse_safety_level_str(level: &str) -> ToolSafetyLevel {
        match level {
            "readonly" => ToolSafetyLevel::ReadOnly,
            "reversible" => ToolSafetyLevel::Reversible,
            "irreversible_low_risk" => ToolSafetyLevel::IrreversibleLowRisk,
            "irreversible_high_risk" => ToolSafetyLevel::IrreversibleHighRisk,
            _ => ToolSafetyLevel::IrreversibleLowRisk, // Default fallback
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
