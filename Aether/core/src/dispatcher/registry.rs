//! Tool Registry - Unified Tool Aggregation
//!
//! Aggregates tools from all sources (Native, MCP, Skills, Custom) into
//! a single queryable registry.

use crate::config::RoutingRuleConfig;
use crate::mcp::types::McpToolInfo;
use crate::services::tools::SystemTool;
use crate::skills::SkillInfo;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::types::{ToolSource, UnifiedTool};

/// Unified Tool Registry
///
/// Thread-safe registry that aggregates tools from all sources:
/// - Native capabilities (Search, Video)
/// - MCP servers (System Tools + External)
/// - Skills (Claude Agent Skills)
/// - Custom commands (user-defined rules)
///
/// # Thread Safety
///
/// Uses `Arc<RwLock<HashMap>>` for concurrent read access with
/// exclusive write access during refresh operations.
///
/// # Usage
///
/// ```rust,ignore
/// let registry = ToolRegistry::new();
///
/// // Register tools from various sources
/// registry.register_native_tools().await;
/// registry.register_mcp_tools(&mcp_tools).await;
/// registry.register_skills(&skills).await;
/// registry.register_custom_commands(&rules).await;
///
/// // Query tools
/// let all = registry.list_all().await;
/// let mcp_only = registry.list_by_source_type("Mcp").await;
/// let tool = registry.get_by_name("search").await;
/// ```
pub struct ToolRegistry {
    /// Tool storage: id -> UnifiedTool
    tools: Arc<RwLock<HashMap<String, UnifiedTool>>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // =========================================================================
    // Registration Methods
    // =========================================================================

    /// Register built-in native tools (Search, Video)
    ///
    /// These are always-available capabilities that don't require
    /// external services or configuration.
    pub async fn register_native_tools(&self) {
        let mut tools = self.tools.write().await;

        // Search capability
        let search = UnifiedTool::new(
            "native:search",
            "search",
            "Search the web for real-time information, news, and facts",
            ToolSource::Native,
        )
        .with_display_name("Web Search")
        .with_parameters_schema(json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query keywords"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results",
                    "default": 5
                }
            },
            "required": ["query"]
        }))
        .with_requires_confirmation(false);

        tools.insert(search.id.clone(), search);

        // Video capability
        let video = UnifiedTool::new(
            "native:video",
            "video",
            "Extract and analyze YouTube video transcripts",
            ToolSource::Native,
        )
        .with_display_name("Video Transcript")
        .with_parameters_schema(json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "YouTube video URL"
                }
            },
            "required": ["url"]
        }))
        .with_requires_confirmation(false);

        tools.insert(video.id.clone(), video);

        debug!("Registered {} native tools", 2);
    }

    /// Register MCP tools from tool info list
    ///
    /// # Arguments
    ///
    /// * `mcp_tools` - List of MCP tool info from McpClient
    /// * `server_name` - Name of the MCP server (e.g., "fs", "git", "github")
    /// * `is_builtin` - Whether this is a builtin System Tool
    pub async fn register_mcp_tools(
        &self,
        mcp_tools: &[McpToolInfo],
        server_name: &str,
        is_builtin: bool,
    ) {
        let mut tools = self.tools.write().await;

        for tool_info in mcp_tools {
            let id = format!("mcp:{}:{}", server_name, tool_info.name);

            let tool = UnifiedTool::new(
                &id,
                &tool_info.name,
                &tool_info.description,
                ToolSource::Mcp {
                    server: server_name.to_string(),
                },
            )
            .with_service_name(&tool_info.service_name)
            .with_requires_confirmation(tool_info.requires_confirmation);

            // Mark builtin system tools for clarity
            let tool = if is_builtin {
                tool.with_display_name(format!("{} (System)", tool_info.name))
            } else {
                tool
            };

            tools.insert(id, tool);
        }

        debug!(
            "Registered {} MCP tools from server '{}'",
            mcp_tools.len(),
            server_name
        );
    }

    /// Register MCP tools from SystemTool instances
    ///
    /// Converts SystemTool's McpTool list to UnifiedTool entries.
    pub async fn register_system_tools(&self, system_tools: &[Arc<dyn SystemTool>]) {
        let mut tools = self.tools.write().await;

        for service in system_tools {
            let service_name = service.name();
            let mcp_tools = service.list_tools();

            for mcp_tool in mcp_tools {
                let id = format!("mcp:{}:{}", service_name, mcp_tool.name);

                let tool = UnifiedTool::new(
                    &id,
                    &mcp_tool.name,
                    &mcp_tool.description,
                    ToolSource::Mcp {
                        server: service_name.to_string(),
                    },
                )
                .with_display_name(format!("{}:{}", service_name, mcp_tool.name))
                .with_service_name(service_name)
                .with_parameters_schema(mcp_tool.input_schema.clone())
                .with_requires_confirmation(mcp_tool.requires_confirmation);

                tools.insert(id, tool);
            }
        }

        debug!(
            "Registered system tools from {} services",
            system_tools.len()
        );
    }

    /// Register skills from SkillInfo list
    ///
    /// # Arguments
    ///
    /// * `skills` - List of installed skill info
    pub async fn register_skills(&self, skills: &[SkillInfo]) {
        let mut tools = self.tools.write().await;

        for skill in skills {
            let id = format!("skill:{}", skill.id);

            let tool = UnifiedTool::new(
                &id,
                &skill.id, // Use skill ID as command name
                &skill.description,
                ToolSource::Skill {
                    id: skill.id.clone(),
                },
            )
            .with_display_name(&skill.name);

            tools.insert(id, tool);
        }

        debug!("Registered {} skills", skills.len());
    }

    /// Register custom commands from config rules
    ///
    /// Only rules with ^/ prefix patterns are registered as tools.
    ///
    /// # Arguments
    ///
    /// * `rules` - Routing rules from config.toml
    pub async fn register_custom_commands(&self, rules: &[RoutingRuleConfig]) {
        let mut tools = self.tools.write().await;
        let mut count = 0;

        for (index, rule) in rules.iter().enumerate() {
            // Only register slash commands as tools
            if !rule.regex.starts_with("^/") {
                continue;
            }

            // Extract command name from regex pattern
            // e.g., "^/translate" -> "translate"
            let command_name = extract_command_name(&rule.regex);
            if command_name.is_empty() {
                warn!(
                    "Could not extract command name from pattern: {}",
                    rule.regex
                );
                continue;
            }

            let id = format!("custom:{}", command_name);

            // Use system_prompt as description if available, otherwise generic
            let description = rule
                .system_prompt
                .as_ref()
                .map(|s| truncate_description(s, 100))
                .unwrap_or_else(|| format!("Custom command /{}", command_name));

            let tool = UnifiedTool::new(
                &id,
                &command_name,
                description,
                ToolSource::Custom { rule_index: index },
            )
            .with_display_name(format!("/{}", command_name));

            tools.insert(id, tool);
            count += 1;
        }

        debug!("Registered {} custom commands", count);
    }

    /// Clear all registered tools
    pub async fn clear(&self) {
        let mut tools = self.tools.write().await;
        tools.clear();
        debug!("Cleared all tools from registry");
    }

    /// Refresh all tools (clear and re-register)
    ///
    /// This is a convenience method that should be called when configuration
    /// changes or MCP connections are updated.
    pub async fn refresh_all(
        &self,
        system_tools: &[Arc<dyn SystemTool>],
        mcp_tools: &[(String, Vec<McpToolInfo>)], // (server_name, tools)
        skills: &[SkillInfo],
        rules: &[RoutingRuleConfig],
    ) {
        self.clear().await;
        self.register_native_tools().await;
        self.register_system_tools(system_tools).await;

        for (server_name, tools) in mcp_tools {
            self.register_mcp_tools(tools, server_name, false).await;
        }

        self.register_skills(skills).await;
        self.register_custom_commands(rules).await;

        let count = self.tools.read().await.len();
        info!("Tool registry refreshed: {} total tools", count);
    }

    // =========================================================================
    // Query Methods
    // =========================================================================

    /// List all active tools
    ///
    /// Returns all tools where `is_active == true`.
    pub async fn list_all(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        tools
            .values()
            .filter(|t| t.is_active)
            .cloned()
            .collect()
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
                t.is_active
                    && matches!(&t.source, ToolSource::Mcp { server: s } if s == server)
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

    // =========================================================================
    // Tool State Management
    // =========================================================================

    /// Set tool active state
    ///
    /// # Arguments
    ///
    /// * `id` - Tool ID
    /// * `active` - Whether the tool should be active
    ///
    /// # Returns
    ///
    /// `true` if tool was found and updated, `false` otherwise
    pub async fn set_tool_active(&self, id: &str, active: bool) -> bool {
        let mut tools = self.tools.write().await;
        if let Some(tool) = tools.get_mut(id) {
            tool.is_active = active;
            debug!("Tool '{}' active state set to {}", id, active);
            true
        } else {
            false
        }
    }

    // =========================================================================
    // Prompt Generation
    // =========================================================================

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
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Extract command name from regex pattern
///
/// Examples:
/// - "^/translate" -> "translate"
/// - "^/(?i)code" -> "code"
/// - "^/draw\\s+" -> "draw"
fn extract_command_name(pattern: &str) -> String {
    // Remove common regex prefixes and patterns
    let cleaned = pattern
        .trim_start_matches("^/")
        .trim_start_matches("(?i)")
        .trim_start_matches('(');

    // Take characters until we hit a regex special character
    cleaned
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// Truncate description to max length, adding ellipsis
fn truncate_description(s: &str, max_len: usize) -> String {
    let s = s.trim();
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_registry_new() {
        let registry = ToolRegistry::new();
        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn test_register_native_tools() {
        let registry = ToolRegistry::new();
        registry.register_native_tools().await;

        assert_eq!(registry.count().await, 2);

        let search = registry.get_by_name("search").await;
        assert!(search.is_some());
        assert_eq!(search.unwrap().source, ToolSource::Native);

        let video = registry.get_by_name("video").await;
        assert!(video.is_some());
    }

    #[tokio::test]
    async fn test_register_skills() {
        let registry = ToolRegistry::new();

        let skills = vec![
            SkillInfo {
                id: "refine-text".to_string(),
                name: "Refine Text".to_string(),
                description: "Improve and polish writing".to_string(),
                allowed_tools: vec![],
            },
            SkillInfo {
                id: "code-review".to_string(),
                name: "Code Review".to_string(),
                description: "Review code for issues".to_string(),
                allowed_tools: vec![],
            },
        ];

        registry.register_skills(&skills).await;

        assert_eq!(registry.count().await, 2);

        let tool = registry.get_by_id("skill:refine-text").await;
        assert!(tool.is_some());
        let tool = tool.unwrap();
        assert!(matches!(tool.source, ToolSource::Skill { .. }));
    }

    #[tokio::test]
    async fn test_register_custom_commands() {
        let registry = ToolRegistry::new();

        let rules = vec![
            RoutingRuleConfig {
                regex: "^/translate".to_string(),
                provider: Some("openai".to_string()),
                system_prompt: Some("You are a translator.".to_string()),
                ..Default::default()
            },
            RoutingRuleConfig {
                regex: "^/code".to_string(),
                provider: Some("claude".to_string()),
                system_prompt: Some("You are a code assistant.".to_string()),
                ..Default::default()
            },
            RoutingRuleConfig {
                regex: ".*".to_string(), // Catch-all, should not be registered
                provider: Some("openai".to_string()),
                system_prompt: None,
                ..Default::default()
            },
        ];

        registry.register_custom_commands(&rules).await;

        assert_eq!(registry.count().await, 2); // Only slash commands

        let translate = registry.get_by_name("translate").await;
        assert!(translate.is_some());
        assert!(matches!(
            translate.unwrap().source,
            ToolSource::Custom { rule_index: 0 }
        ));
    }

    #[tokio::test]
    async fn test_list_by_source_type() {
        let registry = ToolRegistry::new();
        registry.register_native_tools().await;

        let skills = vec![SkillInfo {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: "Test skill".to_string(),
            allowed_tools: vec![],
        }];
        registry.register_skills(&skills).await;

        let native = registry.list_by_source_type("Native").await;
        assert_eq!(native.len(), 2);

        let skill = registry.list_by_source_type("Skill").await;
        assert_eq!(skill.len(), 1);
    }

    #[tokio::test]
    async fn test_search() {
        let registry = ToolRegistry::new();
        registry.register_native_tools().await;

        let results = registry.search("search").await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "search");

        let results = registry.search("web").await;
        assert!(!results.is_empty()); // Should match description
    }

    #[tokio::test]
    async fn test_set_tool_active() {
        let registry = ToolRegistry::new();
        registry.register_native_tools().await;

        // Deactivate search
        let updated = registry.set_tool_active("native:search", false).await;
        assert!(updated);

        // Should not appear in active list
        let all = registry.list_all().await;
        assert!(!all.iter().any(|t| t.id == "native:search"));

        // Should appear in full list
        let all_with_inactive = registry.list_all_with_inactive().await;
        assert!(all_with_inactive.iter().any(|t| t.id == "native:search"));
    }

    #[tokio::test]
    async fn test_to_prompt_block() {
        let registry = ToolRegistry::new();
        registry.register_native_tools().await;

        let prompt = registry.to_prompt_block().await;
        assert!(prompt.contains("**search**"));
        assert!(prompt.contains("**video**"));
    }

    #[test]
    fn test_extract_command_name() {
        assert_eq!(extract_command_name("^/translate"), "translate");
        assert_eq!(extract_command_name("^/(?i)code"), "code");
        assert_eq!(extract_command_name("^/draw\\s+"), "draw");
        assert_eq!(extract_command_name("^/my-command"), "my-command");
        assert_eq!(extract_command_name("^/test_cmd"), "test_cmd");
    }

    #[test]
    fn test_truncate_description() {
        assert_eq!(truncate_description("Short", 100), "Short");
        assert_eq!(
            truncate_description("This is a very long description that should be truncated", 20),
            "This is a very lo..."
        );
    }
}
