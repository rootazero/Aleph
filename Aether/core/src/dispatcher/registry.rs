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

use super::builtin_defs::BUILTIN_COMMANDS;
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

    /// Register system builtin commands (/search, /mcp, /skill, /video, /chat)
    ///
    /// These are always-available slash commands that serve as the entry points
    /// for various capabilities. They are the single source of truth for:
    /// - Settings UI preset rules list
    /// - Command completion root commands
    /// - L3 router tool awareness
    ///
    /// Uses BUILTIN_COMMANDS from builtin_defs module as the single source of truth.
    pub async fn register_builtin_tools(&self) {
        let mut tools = self.tools.write().await;

        for def in BUILTIN_COMMANDS {
            let tool = UnifiedTool::builtin(def.name)
                .with_display_name(def.display_name)
                .with_description(def.description)
                .with_icon(def.icon)
                .with_usage(def.usage)
                .with_localization_key(def.localization_key)
                .with_sort_order(def.sort_order)
                .with_has_subtools(def.has_subtools)
                .with_requires_confirmation(false)
                // Routing config from definition
                .with_routing_regex(def.routing_regex)
                .with_routing_system_prompt(def.routing_system_prompt)
                .with_routing_capabilities(
                    def.routing_capabilities.iter().map(|s| s.to_string()).collect()
                )
                .with_routing_intent_type(def.routing_intent_type)
                .with_routing_strip_prefix(true)
                .with_routing_context_format("markdown");
            tools.insert(tool.id.clone(), tool);
        }

        debug!("Registered {} builtin tools from BUILTIN_COMMANDS", BUILTIN_COMMANDS.len());
    }

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
    ///
    /// Registration order:
    /// 1. Builtin commands (single source of truth for /search, /mcp, etc.)
    /// 2. Native capabilities (search, video execution logic)
    /// 3. System tools (MCP builtin servers)
    /// 4. External MCP tools
    /// 5. Skills
    /// 6. Custom commands from config
    pub async fn refresh_all(
        &self,
        system_tools: &[Arc<dyn SystemTool>],
        mcp_tools: &[(String, Vec<McpToolInfo>)], // (server_name, tools)
        skills: &[SkillInfo],
        rules: &[RoutingRuleConfig],
    ) {
        self.clear().await;

        // 1. Builtin commands first (these are the entry points)
        self.register_builtin_tools().await;

        // 2. Native capabilities (execution logic)
        self.register_native_tools().await;

        // 3. System MCP tools
        self.register_system_tools(system_tools).await;

        // 4. External MCP tools
        for (server_name, tools) in mcp_tools {
            self.register_mcp_tools(tools, server_name, false).await;
        }

        // 5. Skills
        self.register_skills(skills).await;

        // 6. Custom commands from user config
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

    /// List builtin tools only
    ///
    /// Returns the 5 system builtin commands (/search, /mcp, /skill, /video, /chat)
    /// sorted by sort_order.
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
                context_format: t.routing_context_format.clone(),
                skill_id: None,
                skill_version: None,
                workflow: None,
                tools: None,
                knowledge_base: None,
                icon: t.icon.clone(),
                hint: t.usage.clone(),
            })
            .collect()
    }

    /// List all tools for UI display (sorted by sort_order, then name)
    ///
    /// Returns all active tools suitable for Settings UI display.
    pub async fn list_all_for_ui(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut result: Vec<_> = tools
            .values()
            .filter(|t| t.is_active)
            .cloned()
            .collect();
        result.sort_by(|a, b| {
            a.sort_order.cmp(&b.sort_order).then(a.name.cmp(&b.name))
        });
        result
    }

    /// List root-level commands for completion
    ///
    /// Returns builtin commands + custom commands (but not nested MCP/Skill tools).
    pub async fn list_root_commands(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut result: Vec<_> = tools
            .values()
            .filter(|t| {
                t.is_active
                    && (t.is_builtin
                        || matches!(t.source, ToolSource::Custom { .. }))
            })
            .cloned()
            .collect();
        result.sort_by(|a, b| {
            a.sort_order.cmp(&b.sort_order).then(a.name.cmp(&b.name))
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
    async fn test_register_builtin_tools() {
        let registry = ToolRegistry::new();
        registry.register_builtin_tools().await;

        assert_eq!(registry.count().await, 5);

        // Check all 5 builtins are registered
        let builtins = registry.list_builtin_tools().await;
        assert_eq!(builtins.len(), 5);

        // Verify sorted by sort_order
        let names: Vec<_> = builtins.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["search", "mcp", "skill", "video", "chat"]);

        // Check metadata
        let search = registry.get_by_id("builtin:search").await.unwrap();
        assert!(search.is_builtin);
        assert_eq!(search.icon, Some("magnifyingglass".to_string()));
        assert_eq!(search.localization_key, Some("tool.search".to_string()));
        assert_eq!(search.sort_order, 1);

        // Check namespace tools have has_subtools
        let mcp = registry.get_by_id("builtin:mcp").await.unwrap();
        assert!(mcp.has_subtools);

        let skill = registry.get_by_id("builtin:skill").await.unwrap();
        assert!(skill.has_subtools);
    }

    #[tokio::test]
    async fn test_list_root_commands() {
        let registry = ToolRegistry::new();
        registry.register_builtin_tools().await;

        let rules = vec![RoutingRuleConfig {
            regex: "^/en".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Translate to English".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let roots = registry.list_root_commands().await;
        // 5 builtins + 1 custom
        assert_eq!(roots.len(), 6);

        // Builtins should come first (lower sort_order)
        assert!(roots[0].is_builtin);
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
