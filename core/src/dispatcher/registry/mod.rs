//! Tool Registry - Unified Tool Aggregation
//!
//! Aggregates tools from all sources (Native, MCP, Skills, Custom) into
//! a single queryable registry.

mod conflict;
mod discovery;
mod helpers;
mod query;
mod registration;
mod state;
mod types;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::RoutingRuleConfig;
use crate::mcp::types::McpToolInfo;
use crate::skills::SkillInfo;

use super::types::{ToolIndex, ToolIndexEntry, ToolSourceType, UnifiedTool};
use conflict::ConflictResolver;
use discovery::ToolDiscovery;
use query::ToolQuery;
use registration::ToolRegistrar;
use state::ToolState;
use types::ToolStorage;

// Re-export helpers for tests

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
/// registry.register_builtin_tools().await;
/// registry.register_agent_tools(&native_tools, "filesystem").await;
/// registry.register_mcp_tools(&mcp_tools, "server", false).await;
/// registry.register_skills(&skills).await;
/// registry.register_custom_commands(&rules).await;
///
/// // Query tools
/// let all = registry.list_all().await;
/// let mcp_only = registry.list_by_source_type("Mcp").await;
/// let tool = registry.get_by_name("search").await;
/// ```
pub struct ToolRegistry {
    /// Registrar for tool registration
    registrar: ToolRegistrar,
    /// Conflict resolver for handling name conflicts
    conflict_resolver: ConflictResolver,
    /// Query handler for tool queries
    query: ToolQuery,
    /// State manager for tool state operations
    state: ToolState,
    /// Discovery handler for smart tool discovery
    discovery: ToolDiscovery,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        let tools: ToolStorage = Arc::new(RwLock::new(HashMap::new()));
        Self {
            registrar: ToolRegistrar::new(Arc::clone(&tools)),
            conflict_resolver: ConflictResolver::new(Arc::clone(&tools)),
            query: ToolQuery::new(Arc::clone(&tools)),
            state: ToolState::new(Arc::clone(&tools)),
            discovery: ToolDiscovery::new(tools),
        }
    }

    // =========================================================================
    // Registration Methods
    // =========================================================================

    /// Register builtin tools
    pub async fn register_builtin_tools(&self) {
        self.registrar
            .register_builtin_tools(&self.conflict_resolver)
            .await;
    }

    /// Register MCP tools from tool info list (Flat Namespace Mode)
    pub async fn register_mcp_tools(
        &self,
        mcp_tools: &[McpToolInfo],
        server_name: &str,
        is_builtin: bool,
    ) {
        self.registrar
            .register_mcp_tools(mcp_tools, server_name, is_builtin, &self.conflict_resolver)
            .await;
    }

    /// Register native AgentTools (DEPRECATED)
    #[deprecated(note = "Use rig-core tools and register_mcp_tools instead")]
    #[allow(deprecated)]
    pub async fn register_agent_tools<T>(&self, tools: &[Arc<T>], service_name: &str) {
        self.registrar.register_agent_tools(tools, service_name).await;
    }

    /// Register skills from SkillInfo list (Flat Namespace Mode)
    pub async fn register_skills(&self, skills: &[SkillInfo]) {
        self.registrar
            .register_skills(skills, &self.conflict_resolver)
            .await;
    }

    /// Register custom commands from config rules
    pub async fn register_custom_commands(&self, rules: &[RoutingRuleConfig]) {
        self.registrar.register_custom_commands(rules).await;
    }

    // =========================================================================
    // Conflict Resolution (Flat Namespace)
    // =========================================================================

    /// Check if a command name conflicts with an existing tool
    pub async fn check_conflict(&self, name: &str) -> Option<super::types::ConflictInfo> {
        self.conflict_resolver.check_conflict(name).await
    }

    /// Resolve a naming conflict between two tools
    pub fn resolve_conflict(
        &self,
        name: &str,
        conflict: &super::types::ConflictInfo,
        new_source: &super::types::ToolSource,
    ) -> super::types::ConflictResolution {
        self.conflict_resolver
            .resolve_conflict(name, conflict, new_source)
    }

    /// Apply conflict resolution by renaming an existing tool
    pub async fn rename_existing_tool(&self, existing_id: &str, new_name: &str) -> bool {
        self.conflict_resolver
            .rename_existing_tool(existing_id, new_name)
            .await
    }

    /// Register a tool with automatic conflict resolution
    pub async fn register_with_conflict_resolution(&self, tool: UnifiedTool) -> String {
        self.conflict_resolver
            .register_with_conflict_resolution(tool)
            .await
    }

    // =========================================================================
    // State Management
    // =========================================================================

    /// Clear all registered tools
    pub async fn clear(&self) {
        self.state.clear().await;
    }

    /// Atomic refresh - build new HashMap and replace in one operation
    pub async fn refresh_atomic(&self, new_tools: Vec<UnifiedTool>) {
        self.state.refresh_atomic(new_tools).await;
    }

    /// Remove all tools of a specific source type
    pub async fn remove_by_source_type(&self, source_type: ToolSourceType) -> usize {
        self.state.remove_by_source_type(source_type).await
    }

    /// Remove tools from a specific MCP server
    pub async fn remove_by_mcp_server(&self, server_name: &str) -> usize {
        self.state.remove_by_mcp_server(server_name).await
    }

    /// Remove all skill tools
    pub async fn remove_skills(&self) -> usize {
        self.state.remove_skills().await
    }

    /// Remove all custom commands
    pub async fn remove_custom_commands(&self) -> usize {
        self.state.remove_custom_commands().await
    }

    /// Remove all MCP tools (from all servers)
    pub async fn remove_all_mcp_tools(&self) -> usize {
        self.state.remove_all_mcp_tools().await
    }

    /// Remove all native tools
    pub async fn remove_native_tools(&self) -> usize {
        self.state.remove_native_tools().await
    }

    /// Refresh all tools from all sources
    pub async fn refresh_all(
        &self,
        mcp_tools: &[(String, Vec<McpToolInfo>)],
        skills: &[SkillInfo],
        rules: &[RoutingRuleConfig],
    ) {
        self.state
            .refresh_all(
                mcp_tools,
                skills,
                rules,
                &self.registrar,
                &self.conflict_resolver,
            )
            .await;
    }

    /// Set tool active state
    pub async fn set_tool_active(&self, id: &str, active: bool) -> bool {
        self.state.set_tool_active(id, active).await
    }

    // =========================================================================
    // Query Methods
    // =========================================================================

    /// List all active tools
    pub async fn list_all(&self) -> Vec<UnifiedTool> {
        self.query.list_all().await
    }

    /// List builtin tools only
    pub async fn list_builtin_tools(&self) -> Vec<UnifiedTool> {
        self.query.list_builtin_tools().await
    }

    /// List preset tools for Settings UI (Flat Namespace Mode)
    pub async fn list_preset_tools(&self) -> Vec<UnifiedTool> {
        self.query.list_preset_tools().await
    }

    /// Generate routing rules from builtin tools
    pub async fn get_builtin_routing_rules(&self) -> Vec<RoutingRuleConfig> {
        self.query.get_builtin_routing_rules().await
    }

    /// List all tools for UI display (sorted by sort_order, then name)
    pub async fn list_all_for_ui(&self) -> Vec<UnifiedTool> {
        self.query.list_all_for_ui().await
    }

    /// List root-level commands for UI (Flat Namespace Mode)
    pub async fn list_root_commands(&self) -> Vec<UnifiedTool> {
        self.query.list_root_commands().await
    }

    /// List all tools including inactive ones
    pub async fn list_all_with_inactive(&self) -> Vec<UnifiedTool> {
        self.query.list_all_with_inactive().await
    }

    /// List tools by source type
    pub async fn list_by_source_type(&self, source_type: &str) -> Vec<UnifiedTool> {
        self.query.list_by_source_type(source_type).await
    }

    /// List tools by MCP server name
    pub async fn list_by_mcp_server(&self, server: &str) -> Vec<UnifiedTool> {
        self.query.list_by_mcp_server(server).await
    }

    /// Get tool by ID
    pub async fn get_by_id(&self, id: &str) -> Option<UnifiedTool> {
        self.query.get_by_id(id).await
    }

    /// Get tool by name
    pub async fn get_by_name(&self, name: &str) -> Option<UnifiedTool> {
        self.query.get_by_name(name).await
    }

    /// Fuzzy search tools by name or description
    pub async fn search(&self, query: &str) -> Vec<UnifiedTool> {
        self.query.search(query).await
    }

    /// Get total tool count
    pub async fn count(&self) -> usize {
        self.query.count().await
    }

    /// Get active tool count
    pub async fn active_count(&self) -> usize {
        self.query.active_count().await
    }

    // =========================================================================
    // Prompt Generation & Smart Discovery
    // =========================================================================

    /// Generate tool list for LLM prompt
    pub async fn to_prompt_block(&self) -> String {
        self.discovery.to_prompt_block().await
    }

    /// Generate lightweight tool index for smart discovery
    pub async fn generate_tool_index(&self, core_tools: &[&str]) -> ToolIndex {
        self.discovery.generate_tool_index(core_tools).await
    }

    /// Generate smart prompt with tool index + filtered full schemas
    pub async fn generate_smart_prompt(
        &self,
        core_tools: &[&str],
        filtered_tools: &[&str],
    ) -> (Vec<UnifiedTool>, String) {
        self.discovery
            .generate_smart_prompt(core_tools, filtered_tools)
            .await
    }

    /// Get full tool definition by name
    pub async fn get_tool_definition(&self, name: &str) -> Option<UnifiedTool> {
        self.discovery.get_tool_definition(name).await
    }

    /// List tools by category for the `list_tools` meta tool
    pub async fn list_tools_by_category(&self, category: Option<&str>) -> Vec<ToolIndexEntry> {
        self.discovery.list_tools_by_category(category).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::types::ToolPriority;
    use super::super::types::{ConflictInfo, ConflictResolution, ToolSource};

    #[tokio::test]
    async fn test_registry_new() {
        let registry = ToolRegistry::new();
        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn test_register_builtin_tools() {
        let registry = ToolRegistry::new();
        registry.register_builtin_tools().await;

        // Should register 4 builtin tools (2 generation + 2 skill reading)
        assert_eq!(registry.count().await, 4);

        // Builtins should include generation tools
        let builtins = registry.list_builtin_tools().await;
        assert_eq!(builtins.len(), 4);

        // Verify tool names
        let names: Vec<_> = builtins.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"generate_image"));
        assert!(names.contains(&"generate_speech"));
        assert!(names.contains(&"read_skill"));
        assert!(names.contains(&"list_skills"));
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
        // 4 builtin tools + 1 custom = 5
        assert_eq!(roots.len(), 5);

        // First should be builtins (sorted by priority)
        assert!(roots.iter().any(|t| t.name == "generate_image"));
        assert!(roots.iter().any(|t| t.name == "generate_speech"));
        assert!(roots.iter().any(|t| t.name == "en"));
    }

    #[tokio::test]
    async fn test_register_skills() {
        let registry = ToolRegistry::new();

        let skills = vec![
            SkillInfo {
                id: "refine-text".to_string(),
                name: "Refine Text".to_string(),
                description: "Improve and polish writing".to_string(),
                triggers: vec![],
                allowed_tools: vec![],
            },
            SkillInfo {
                id: "code-review".to_string(),
                name: "Code Review".to_string(),
                description: "Review code for issues".to_string(),
                triggers: vec![],
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
        registry.register_builtin_tools().await;

        let skills = vec![SkillInfo {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: "Test skill".to_string(),
            triggers: vec![],
            allowed_tools: vec![],
        }];
        registry.register_skills(&skills).await;

        let builtin = registry.list_by_source_type("Builtin").await;
        assert_eq!(builtin.len(), 4); // 4 builtin tools

        let skill = registry.list_by_source_type("Skill").await;
        assert_eq!(skill.len(), 1);

        // Native should be empty (reserved for future OS command tools)
        let native = registry.list_by_source_type("Native").await;
        assert_eq!(native.len(), 0);
    }

    #[tokio::test]
    async fn test_search() {
        let registry = ToolRegistry::new();

        // Register a custom command to test search
        let rules = vec![RoutingRuleConfig {
            regex: "^/search".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Search assistant".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let results = registry.search("search").await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "search");
    }

    #[tokio::test]
    async fn test_set_tool_active() {
        let registry = ToolRegistry::new();

        // Register a custom command to test
        let rules = vec![RoutingRuleConfig {
            regex: "^/test".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Test assistant".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        // Deactivate test command
        let updated = registry.set_tool_active("custom:test", false).await;
        assert!(updated);

        // Should not appear in active list
        let all = registry.list_all().await;
        assert!(!all.iter().any(|t| t.id == "custom:test"));

        // Should appear in full list
        let all_with_inactive = registry.list_all_with_inactive().await;
        assert!(all_with_inactive.iter().any(|t| t.id == "custom:test"));
    }

    #[tokio::test]
    async fn test_to_prompt_block() {
        let registry = ToolRegistry::new();

        // Register custom commands to test prompt block
        let rules = vec![
            RoutingRuleConfig {
                regex: "^/translate".to_string(),
                provider: Some("openai".to_string()),
                system_prompt: Some("Translate".to_string()),
                ..Default::default()
            },
            RoutingRuleConfig {
                regex: "^/code".to_string(),
                provider: Some("openai".to_string()),
                system_prompt: Some("Code assistant".to_string()),
                ..Default::default()
            },
        ];
        registry.register_custom_commands(&rules).await;

        let prompt = registry.to_prompt_block().await;
        assert!(prompt.contains("**translate**"));
        assert!(prompt.contains("**code**"));
    }

    // =========================================================================
    // Conflict Resolution Tests
    // =========================================================================

    #[tokio::test]
    async fn test_check_conflict_no_conflict() {
        let registry = ToolRegistry::new();

        // Register a custom command
        let rules = vec![RoutingRuleConfig {
            regex: "^/translate".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Translate".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        // No conflict for a new unique name
        let conflict = registry.check_conflict("git").await;
        assert!(conflict.is_none());
    }

    #[tokio::test]
    async fn test_check_conflict_exists() {
        let registry = ToolRegistry::new();

        // Register a custom command
        let rules = vec![RoutingRuleConfig {
            regex: "^/search".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Search".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        // Conflict with custom "search"
        let conflict = registry.check_conflict("search").await;
        assert!(conflict.is_some());

        let info = conflict.unwrap();
        assert_eq!(info.existing_name, "search");
        assert_eq!(info.existing_priority, ToolPriority::Custom);
    }

    #[tokio::test]
    async fn test_check_conflict_case_insensitive() {
        let registry = ToolRegistry::new();

        // Register a custom command
        let rules = vec![RoutingRuleConfig {
            regex: "^/search".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Search".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        // Should find conflict even with different case
        let conflict = registry.check_conflict("SEARCH").await;
        assert!(conflict.is_some());
        assert_eq!(conflict.unwrap().existing_name, "search");
    }

    #[test]
    fn test_resolve_conflict_new_wins() {
        let registry = ToolRegistry::new();

        // MCP tool exists, Builtin tries to register
        let conflict = ConflictInfo {
            existing_id: "mcp:server:search".to_string(),
            existing_name: "search".to_string(),
            existing_source: ToolSource::Mcp {
                server: "server".into(),
            },
            existing_priority: ToolPriority::Mcp,
        };

        let resolution = registry.resolve_conflict("search", &conflict, &ToolSource::Builtin);

        // Builtin has higher priority, should rename existing
        match resolution {
            ConflictResolution::RenameExisting {
                original_name,
                new_name,
            } => {
                assert_eq!(original_name, "search");
                assert_eq!(new_name, "search-mcp");
            }
            _ => panic!("Expected RenameExisting"),
        }
    }

    #[test]
    fn test_resolve_conflict_existing_wins() {
        let registry = ToolRegistry::new();

        // Builtin exists, MCP tries to register
        let conflict = ConflictInfo {
            existing_id: "builtin:search".to_string(),
            existing_name: "search".to_string(),
            existing_source: ToolSource::Builtin,
            existing_priority: ToolPriority::Builtin,
        };

        let resolution = registry.resolve_conflict(
            "search",
            &conflict,
            &ToolSource::Mcp {
                server: "server".into(),
            },
        );

        // Builtin has higher priority, should rename new
        match resolution {
            ConflictResolution::RenameNew {
                original_name,
                new_name,
            } => {
                assert_eq!(original_name, "search");
                assert_eq!(new_name, "search-mcp");
            }
            _ => panic!("Expected RenameNew"),
        }
    }

    #[test]
    fn test_resolve_conflict_same_priority() {
        let registry = ToolRegistry::new();

        // Two MCP tools with same priority
        let conflict = ConflictInfo {
            existing_id: "mcp:server1:status".to_string(),
            existing_name: "status".to_string(),
            existing_source: ToolSource::Mcp {
                server: "server1".into(),
            },
            existing_priority: ToolPriority::Mcp,
        };

        let resolution = registry.resolve_conflict(
            "status",
            &conflict,
            &ToolSource::Mcp {
                server: "server2".into(),
            },
        );

        // Same priority - new tool gets renamed (first registered wins)
        match resolution {
            ConflictResolution::RenameNew {
                original_name,
                new_name,
            } => {
                assert_eq!(original_name, "status");
                assert_eq!(new_name, "status-mcp");
            }
            _ => panic!("Expected RenameNew"),
        }
    }

    #[tokio::test]
    async fn test_register_with_conflict_resolution_no_conflict() {
        let registry = ToolRegistry::new();

        let tool = UnifiedTool::new(
            "mcp:server:git",
            "git",
            "Git operations",
            ToolSource::Mcp {
                server: "server".into(),
            },
        );

        let id = registry.register_with_conflict_resolution(tool).await;

        // No conflict, original ID used
        assert_eq!(id, "mcp:server:git");

        let registered = registry.get_by_id(&id).await;
        assert!(registered.is_some());
        assert_eq!(registered.unwrap().name, "git");
    }

    #[tokio::test]
    async fn test_register_with_conflict_resolution_new_renamed() {
        let registry = ToolRegistry::new();

        // Register Custom tool first (higher priority than MCP)
        let custom_tool = UnifiedTool::new(
            "custom:search",
            "search",
            "Custom Search",
            ToolSource::Custom { rule_index: 0 },
        );
        registry
            .register_with_conflict_resolution(custom_tool)
            .await;

        // Try to register MCP tool with same name as custom
        let mcp_tool = UnifiedTool::new(
            "mcp:server:search",
            "search",
            "MCP Search",
            ToolSource::Mcp {
                server: "server".into(),
            },
        );

        let id = registry.register_with_conflict_resolution(mcp_tool).await;

        // MCP tool should be renamed (Custom has higher priority)
        assert_eq!(id, "mcp:server:search-mcp");

        let registered = registry.get_by_id(&id).await.unwrap();
        assert_eq!(registered.name, "search-mcp");
        assert_eq!(registered.original_name, Some("search".to_string()));
        assert!(registered.was_renamed);

        // Custom should still have original name
        let custom = registry.get_by_id("custom:search").await.unwrap();
        assert_eq!(custom.name, "search");
        assert!(!custom.was_renamed);
    }

    #[tokio::test]
    async fn test_register_with_conflict_resolution_existing_renamed() {
        let registry = ToolRegistry::new();

        // Register MCP tool first
        let mcp_tool = UnifiedTool::new(
            "mcp:server:test",
            "test",
            "MCP Test",
            ToolSource::Mcp {
                server: "server".into(),
            },
        );
        registry.register_with_conflict_resolution(mcp_tool).await;

        // Register Custom tool with same name (higher priority)
        let custom_tool = UnifiedTool::new(
            "custom:test",
            "test",
            "Custom Test",
            ToolSource::Custom { rule_index: 0 },
        );
        let id = registry
            .register_with_conflict_resolution(custom_tool)
            .await;

        // Custom tool takes the name
        assert_eq!(id, "custom:test");
        let custom = registry.get_by_id(&id).await.unwrap();
        assert_eq!(custom.name, "test");
        assert!(!custom.was_renamed);

        // MCP tool should be renamed
        let mcp = registry.get_by_id("mcp:server:test-mcp").await;
        assert!(mcp.is_some());
        let mcp = mcp.unwrap();
        assert_eq!(mcp.name, "test-mcp");
        assert_eq!(mcp.original_name, Some("test".to_string()));
        assert!(mcp.was_renamed);
    }

    // =========================================================================
    // Atomic Refresh Tests (Phase 3.4)
    // =========================================================================

    #[tokio::test]
    async fn test_refresh_atomic_replaces_all_tools() {
        let registry = ToolRegistry::new();

        // Register some initial tools
        let rules = vec![RoutingRuleConfig {
            regex: "^/old".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Old command".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;
        let initial_count = registry.count().await;
        assert_eq!(initial_count, 1);

        // Create new tool list
        let new_tools = vec![
            UnifiedTool::new(
                "test:tool1",
                "tool1",
                "Test Tool 1",
                ToolSource::Custom { rule_index: 0 },
            ),
            UnifiedTool::new(
                "test:tool2",
                "tool2",
                "Test Tool 2",
                ToolSource::Custom { rule_index: 1 },
            ),
        ];

        // Atomic refresh should replace all tools
        registry.refresh_atomic(new_tools).await;

        // Should have exactly 2 tools now
        assert_eq!(registry.count().await, 2);

        // Old custom tools should be gone
        assert!(registry.get_by_id("custom:old").await.is_none());

        // New tools should exist
        assert!(registry.get_by_id("test:tool1").await.is_some());
        assert!(registry.get_by_id("test:tool2").await.is_some());
    }

    #[tokio::test]
    async fn test_refresh_atomic_empty_list() {
        let registry = ToolRegistry::new();

        // Register some tools first
        let rules = vec![RoutingRuleConfig {
            regex: "^/test".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Test".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;
        assert!(registry.count().await > 0);

        // Refresh with empty list
        registry.refresh_atomic(vec![]).await;

        // Should have 0 tools
        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn test_refresh_atomic_preserves_tool_properties() {
        let registry = ToolRegistry::new();

        // Create tool with all properties
        let tool = UnifiedTool::new(
            "custom:mytool",
            "mytool",
            "My Tool Description",
            ToolSource::Custom { rule_index: 0 },
        )
        .with_display_name("My Tool")
        .with_icon("star.fill")
        .with_usage("/mytool [args]")
        .with_requires_confirmation(true);

        registry.refresh_atomic(vec![tool]).await;

        let retrieved = registry.get_by_id("custom:mytool").await.unwrap();
        assert_eq!(retrieved.name, "mytool");
        assert_eq!(retrieved.display_name, "My Tool");
        assert_eq!(retrieved.description, "My Tool Description");
        assert_eq!(retrieved.icon, Some("star.fill".to_string()));
        assert_eq!(retrieved.usage, Some("/mytool [args]".to_string()));
        assert!(retrieved.requires_confirmation);
    }
}
