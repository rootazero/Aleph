//! Query Methods for ToolRegistry
//!
//! Methods for querying and searching tools in the registry.

use crate::config::RoutingRuleConfig;

use super::super::types::{ToolSource, UnifiedTool};
use super::types::ToolStorage;

/// Query functionality for ToolRegistry
pub struct ToolQuery {
    tools: ToolStorage,
}

impl ToolQuery {
    /// Create a new query handler with the given storage
    pub fn new(tools: ToolStorage) -> Self {
        Self { tools }
    }

    /// List all active tools
    ///
    /// Returns all tools where `is_active == true`.
    pub async fn list_all(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        tools.values().filter(|t| t.is_active).cloned().collect()
    }

    /// List builtin tools only
    ///
    /// Returns the system builtin commands sorted by sort_order.
    pub async fn list_builtin_tools(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut builtins: Vec<_> = tools
            .values()
            .filter(|t| t.is_builtin && t.is_active)
            .cloned()
            .collect();
        builtins.sort_by_key(|t| t.sort_order);
        builtins
    }

    /// List preset tools for Settings UI (Flat Namespace Mode)
    ///
    /// Returns all non-Custom tools: Builtin + MCP + Skill + Native
    /// These are the "preset" tools that users can't delete, sorted by priority.
    pub async fn list_preset_tools(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut presets: Vec<_> = tools
            .values()
            .filter(|t| t.is_active && !matches!(t.source, ToolSource::Custom { .. }))
            .cloned()
            .collect();

        // Sort by source priority: Builtin > Native > MCP > Skill
        presets.sort_by(|a, b| {
            let priority_a = match &a.source {
                ToolSource::Builtin => 0,
                ToolSource::Native => 1,
                ToolSource::Mcp { .. } => 2,
                ToolSource::Skill { .. } => 3,
                ToolSource::Custom { .. } => 4,
            };
            let priority_b = match &b.source {
                ToolSource::Builtin => 0,
                ToolSource::Native => 1,
                ToolSource::Mcp { .. } => 2,
                ToolSource::Skill { .. } => 3,
                ToolSource::Custom { .. } => 4,
            };
            priority_a
                .cmp(&priority_b)
                .then(a.sort_order.cmp(&b.sort_order))
                .then(a.name.cmp(&b.name))
        });
        presets
    }

    /// Generate routing rules from builtin tools
    ///
    /// This is the SINGLE SOURCE OF TRUTH for builtin command routing configuration.
    /// Config module should call this instead of maintaining separate hardcoded rules.
    ///
    /// Returns RoutingRuleConfig for each builtin tool that has routing_regex set.
    pub async fn get_builtin_routing_rules(&self) -> Vec<RoutingRuleConfig> {
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|t| t.is_builtin && t.routing_regex.is_some())
            .map(|t| RoutingRuleConfig {
                rule_type: Some("command".to_string()),
                is_builtin: true,
                regex: t.routing_regex.clone().unwrap_or_default(),
                provider: Some("openai".to_string()), // Will be overridden by default_provider
                system_prompt: t.routing_system_prompt.clone(),
                strip_prefix: Some(t.routing_strip_prefix),
                capabilities: if t.routing_capabilities.is_empty() {
                    None
                } else {
                    Some(t.routing_capabilities.clone())
                },
                intent_type: t.routing_intent_type.clone(),
                preferred_model: None,
                context_format: t.routing_context_format.clone(),
                icon: t.icon.clone(),
            })
            .collect()
    }

    /// List all tools for UI display (sorted by sort_order, then name)
    ///
    /// Returns all active tools suitable for Settings UI display.
    pub async fn list_all_for_ui(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut result: Vec<_> = tools.values().filter(|t| t.is_active).cloned().collect();
        result.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.name.cmp(&b.name)));
        result
    }

    /// List root-level commands for UI (Flat Namespace Mode)
    ///
    /// Returns all active tools from all sources for command completion.
    /// This is the primary method for UI command completion display.
    ///
    /// Includes: Builtin + Native + Custom + MCP + Skill
    ///
    /// Source priority order for display:
    /// 1. Builtin (system commands)
    /// 2. Native (system capabilities)
    /// 3. Custom (user-defined rules)
    /// 4. MCP (external tools)
    /// 5. Skill (Claude Agent skills)
    pub async fn list_root_commands(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut result: Vec<_> = tools.values().filter(|t| t.is_active).cloned().collect();

        // Sort by source priority, then sort_order, then name
        result.sort_by(|a, b| {
            // Sort order: Builtin > Native > Custom > MCP > Skill
            let priority_a = match &a.source {
                ToolSource::Builtin => 0,
                ToolSource::Native => 1,
                ToolSource::Custom { .. } => 2,
                ToolSource::Mcp { .. } => 3,
                ToolSource::Skill { .. } => 4,
            };
            let priority_b = match &b.source {
                ToolSource::Builtin => 0,
                ToolSource::Native => 1,
                ToolSource::Custom { .. } => 2,
                ToolSource::Mcp { .. } => 3,
                ToolSource::Skill { .. } => 4,
            };

            priority_a
                .cmp(&priority_b)
                .then(a.sort_order.cmp(&b.sort_order))
                .then(a.name.cmp(&b.name))
        });
        result
    }

    /// List all tools including inactive ones
    pub async fn list_all_with_inactive(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        tools.values().cloned().collect()
    }

    /// List tools by source type
    ///
    /// # Arguments
    ///
    /// * `source_type` - One of "Native", "Mcp", "Skill", "Custom"
    pub async fn list_by_source_type(&self, source_type: &str) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|t| t.is_active && t.source.label() == source_type)
            .cloned()
            .collect()
    }

    /// List tools by MCP server name
    pub async fn list_by_mcp_server(&self, server: &str) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|t| {
                t.is_active && matches!(&t.source, ToolSource::Mcp { server: s } if s == server)
            })
            .cloned()
            .collect()
    }

    /// Get tool by ID
    ///
    /// # Arguments
    ///
    /// * `id` - Full tool ID (e.g., "native:search", "mcp:fs:read_file")
    pub async fn get_by_id(&self, id: &str) -> Option<UnifiedTool> {
        let tools = self.tools.read().await;
        tools.get(id).cloned()
    }

    /// Get tool by name
    ///
    /// Searches for a tool by its command name (not full ID).
    /// Returns the first match if multiple tools have the same name.
    pub async fn get_by_name(&self, name: &str) -> Option<UnifiedTool> {
        let tools = self.tools.read().await;
        tools
            .values()
            .find(|t| t.name == name || t.id.ends_with(&format!(":{}", name)))
            .cloned()
    }

    /// Fuzzy search tools by name or description
    ///
    /// Returns tools where name or description contains the query string.
    /// Results are ordered by relevance (name match first, then description).
    pub async fn search(&self, query: &str) -> Vec<UnifiedTool> {
        let query_lower = query.to_lowercase();
        let tools = self.tools.read().await;

        let mut results: Vec<_> = tools
            .values()
            .filter(|t| {
                t.is_active
                    && (t.name.to_lowercase().contains(&query_lower)
                        || t.description.to_lowercase().contains(&query_lower))
            })
            .cloned()
            .collect();

        // Sort by relevance: name matches first
        results.sort_by(|a, b| {
            let a_name_match = a.name.to_lowercase().contains(&query_lower);
            let b_name_match = b.name.to_lowercase().contains(&query_lower);
            match (a_name_match, b_name_match) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.cmp(&b.name),
            }
        });

        results
    }

    /// Get total tool count
    pub async fn count(&self) -> usize {
        let tools = self.tools.read().await;
        tools.len()
    }

    /// Get active tool count
    pub async fn active_count(&self) -> usize {
        let tools = self.tools.read().await;
        tools.values().filter(|t| t.is_active).count()
    }
}
