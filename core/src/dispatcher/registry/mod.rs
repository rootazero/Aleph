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
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::config::RoutingRuleConfig;
use crate::mcp::types::McpToolInfo;
use crate::skill::SkillInfo;

use super::types::{ChannelType, ToolIndex, ToolIndexEntry, ToolSourceType, UnifiedTool};
use conflict::ConflictResolver;
use discovery::ToolDiscovery;
use query::ToolQuery;
use registration::ToolRegistrar;
use state::ToolState;
use types::ToolStorage;
pub use types::ResolvedCommand;

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

    /// Register skills from SkillInfo list (Flat Namespace Mode)
    pub async fn register_skills(&self, skills: &[SkillInfo]) {
        self.registrar
            .register_skills(skills, &self.conflict_resolver)
            .await;
    }

    /// Register plugin tools from manifests (Flat Namespace Mode)
    pub async fn register_plugin_tools(&self, tools: &[(String, String, String)]) {
        self.registrar
            .register_plugin_tools(tools, &self.conflict_resolver)
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

    /// List active tools visible to a specific channel
    pub async fn list_for_channel(&self, channel: ChannelType) -> Vec<UnifiedTool> {
        self.query.list_for_channel(channel).await
    }

    /// Resolve a slash command input to a registered tool
    pub async fn resolve_command(&self, input: &str) -> Option<types::ResolvedCommand> {
        self.query.resolve_command(input).await
    }

    /// Check if a name is a namespace (has active tools with that prefix)
    pub async fn is_namespace(&self, name: &str) -> bool {
        self.query.is_namespace(name).await
    }

    /// List direct children of a namespace
    pub async fn list_namespace_children(&self, namespace: &str) -> Vec<UnifiedTool> {
        self.query.list_namespace_children(namespace).await
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

    /// Filter active tools by name prefix (case-insensitive)
    pub async fn filter_by_prefix(&self, prefix: &str) -> Vec<UnifiedTool> {
        self.query.filter_by_prefix(prefix).await
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
mod tests;
