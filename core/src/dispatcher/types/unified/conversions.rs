//! Conversion and utility methods for UnifiedTool
//!
//! Includes conversions from ToolDefinition, tool index generation,
//! safety level inference, and prompt line formatting.

use super::UnifiedTool;
use crate::config::ToolSafetyPolicy;
use crate::dispatcher::types::category::ToolCategory;
use crate::dispatcher::types::conflict::ToolSource;
use crate::dispatcher::types::definition::ToolDefinition;
use crate::dispatcher::types::index::{truncate_string, ToolIndexCategory, ToolIndexEntry};
use crate::dispatcher::types::safety::ToolSafetyLevel;

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

    // =========================================================================
    // Conversion from AgentTool Types
    // =========================================================================

    /// Create UnifiedTool from ToolDefinition (AgentTool interface)
    ///
    /// Converts `AgentTool` definitions to `UnifiedTool` for unified
    /// registry management. The source is automatically determined from
    /// the tool's category:
    /// - `ToolCategory::Builtin` → `ToolSource::Builtin`
    /// - `ToolCategory::Mcp` → `ToolSource::Mcp { server: service_name }`
    /// - `ToolCategory::Skills` → `ToolSource::Skill { id: tool_name }`
    /// - `ToolCategory::Custom` → `ToolSource::Custom { rule_index: 0 }`
    ///
    /// # Arguments
    ///
    /// * `def` - The tool definition from an AgentTool implementation
    /// * `service_name` - Optional service grouping name. For MCP tools, this
    ///   should be the actual MCP server name (e.g., "github", "filesystem").
    ///   For native tools, this is a grouping name (e.g., "filesystem", "git").
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Native tool
    /// let tool = FileReadTool::new(ctx);
    /// let unified = UnifiedTool::from_tool_definition(tool.definition(), Some("filesystem"));
    ///
    /// // MCP tool
    /// let mcp_bridge = McpToolBridge::new(tool_def, client, "github".to_string());
    /// let unified = UnifiedTool::from_tool_definition(mcp_bridge.definition(), Some("github"));
    /// ```
    pub fn from_tool_definition(def: ToolDefinition, service_name: Option<&str>) -> Self {
        // Determine ToolSource from ToolCategory
        let (source, id) = match def.category {
            ToolCategory::Builtin => {
                let id = format!("builtin:{}", def.name);
                (ToolSource::Builtin, id)
            }
            ToolCategory::Mcp => {
                let server = service_name.unwrap_or("unknown").to_string();
                let id = format!("mcp:{}:{}", server, def.name);
                (ToolSource::Mcp { server }, id)
            }
            ToolCategory::Skills => {
                let skill_id = service_name.unwrap_or(&def.name).to_string();
                let id = format!("skill:{}", skill_id);
                (
                    ToolSource::Skill {
                        id: skill_id.clone(),
                    },
                    id,
                )
            }
            ToolCategory::Custom => {
                let id = format!("custom:{}", def.name);
                (ToolSource::Custom { rule_index: 0 }, id)
            }
            ToolCategory::GeneratedSkill => {
                let id = format!("generated:{}", def.name);
                (ToolSource::Custom { rule_index: 0 }, id)
            }
        };

        let icon = Self::icon_for_category(def.category);
        // Use default policy for safety level inference (policy can be injected via Config)
        let safety_level = Self::infer_safety_level(&def.name, def.category, None);

        // Determine intent type based on source
        let intent_type = match &source {
            ToolSource::Builtin => format!("builtin:{}", def.name),
            ToolSource::Native => format!("native:{}", def.name),
            ToolSource::Mcp { server } => format!("mcp:{}:{}", server, def.name),
            ToolSource::Skill { id } => format!("skill:{}", id),
            ToolSource::Custom { .. } => format!("custom:{}", def.name),
            ToolSource::Plugin { plugin_id } => format!("plugin:{}:{}", plugin_id, def.name),
        };

        let mut tool = Self::new(&id, &def.name, &def.description, source)
            .with_display_name(&def.name)
            .with_parameters_schema(def.parameters.clone())
            .with_requires_confirmation(def.requires_confirmation)
            .with_safety_level(safety_level)
            .with_icon(icon)
            .with_usage(format!("/{} [args]", def.name))
            // Generate routing regex for flat namespace
            .with_routing_regex(format!(r"^/{}\s*", regex::escape(&def.name)))
            .with_routing_intent_type(intent_type)
            .with_routing_strip_prefix(true);

        if let Some(svc) = service_name {
            tool = tool.with_service_name(svc);
        }

        tool
    }

    /// Get icon for a tool category
    fn icon_for_category(category: ToolCategory) -> &'static str {
        // Delegate to ToolCategory's built-in icon method
        category.icon()
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
    pub fn to_prompt_line(&self) -> String {
        let source_badge = match &self.source {
            ToolSource::Native => " [Native - Preferred]".to_string(),
            ToolSource::Builtin => " [Builtin - Preferred]".to_string(),
            ToolSource::Mcp { server } => format!(" [MCP:{}]", server),
            ToolSource::Skill { id } => format!(" [Skill:{}]", id),
            ToolSource::Custom { .. } => " [Custom]".to_string(),
            ToolSource::Plugin { plugin_id } => format!(" [Plugin:{}]", plugin_id),
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
