//! Tool Registry - Unified Tool Aggregation
//!
//! Aggregates tools from all sources (Native, MCP, Skills, Custom) into
//! a single queryable registry.

mod conflict;
pub mod health;
mod helpers;
mod query;
mod registration;
mod state;
mod types;

use crate::sync_primitives::{Arc, AsyncRwLock};
use std::collections::HashMap;

use crate::config::RoutingRuleConfig;
use crate::skill::SkillInfo;

use super::types::{ChannelType, UnifiedTool};
use conflict::ConflictResolver;
// Re-exports for external (integration test, gateway) consumers. The
// in-crate paths use these via fully-qualified names, hence the lint
// suppression.
#[allow(unused_imports)]
pub use health::{HealthReason, HealthSnapshot, ProbeResult, ToolHealthCache, ToolHealthProbe};
use query::ToolQuery;
use registration::ToolRegistrar;
use state::ToolState;
pub use types::ResolvedCommand;
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
/// let registry = ToolCatalog::new();
///
/// // Register tools from various sources
/// registry.register_builtin_tools().await;
/// registry.register_skills(&skills).await;
/// registry.register_custom_commands(&rules).await;
///
/// // Query tools
/// let all = registry.list_all().await;
/// let tool = registry.get_by_name("search").await;
/// ```
pub struct ToolCatalog {
    /// Registrar for tool registration
    registrar: ToolRegistrar,
    /// Conflict resolver for handling name conflicts
    conflict_resolver: ConflictResolver,
    /// Query handler for tool queries
    query: ToolQuery,
    /// State manager for tool state operations
    state: ToolState,
    /// Runtime health probe cache. Tools opt in via [`register_health_probe`].
    /// Consulted alongside `is_active` when emitting native tool schemas so
    /// the LLM never sees a tool whose dependencies are dead.
    health: Arc<ToolHealthCache>,
}

impl Default for ToolCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolCatalog {
    /// Create a new empty registry
    #[must_use]
    pub fn new() -> Self {
        Self::with_health(Arc::new(ToolHealthCache::new()))
    }

    /// Create a registry that shares an existing health cache.
    ///
    /// Boot needs this because the catalog is built *after* the
    /// `ExecutionEngine` has already been wrapped in an `Arc`, so the engine
    /// cannot be handed the catalog's own cache afterwards. Creating the cache
    /// first and giving the same handle to both is what lets the per-request
    /// tool service consult the probes this catalog registers — without it the
    /// gate is attached to a cache nobody writes to.
    #[must_use]
    pub fn with_health(health: Arc<ToolHealthCache>) -> Self {
        let tools: ToolStorage = Arc::new(AsyncRwLock::new(HashMap::new()));
        Self {
            registrar: ToolRegistrar::new(Arc::clone(&tools)),
            conflict_resolver: ConflictResolver::new(Arc::clone(&tools)),
            query: ToolQuery::new(Arc::clone(&tools)),
            state: ToolState::new(tools),
            health,
        }
    }

    /// Shared handle to the runtime health cache.
    ///
    /// Used by callers that need to register a [`ToolHealthProbe`]
    /// (typically at boot, alongside the tool that owns the probe) or
    /// inspect cached probe results.
    #[must_use]
    pub fn health(&self) -> Arc<ToolHealthCache> {
        Arc::clone(&self.health)
    }

    /// Register a runtime health probe for the named tool.
    ///
    /// Tools opt in via this method; absent any registered probe a tool
    /// is treated as always healthy and continues to surface in the
    /// model's native tool list as before.
    pub fn register_health_probe(&self, name: impl Into<String>, probe: Arc<dyn ToolHealthProbe>) {
        self.health.register_probe(name, probe);
    }

    // =========================================================================
    // Registration Methods
    // =========================================================================

    /// Register builtin tools
    pub async fn register_builtin_tools(&self) {
        self.registrar
            .register_builtin_tools(&self.conflict_resolver)
            .await;
        self.health.invalidate_all();
    }

    /// Register skills from `SkillInfo` list (Flat Namespace Mode)
    pub async fn register_skills(&self, skills: &[SkillInfo]) {
        self.registrar
            .register_skills(skills, &self.conflict_resolver)
            .await;
        self.health.invalidate_all();
    }

    /// Register plugin tools from manifests (Flat Namespace Mode)
    pub async fn register_plugin_tools(&self, tools: &[(String, String, String)]) {
        self.registrar
            .register_plugin_tools(tools, &self.conflict_resolver)
            .await;
        self.health.invalidate_all();
    }

    /// Register custom commands from config rules
    pub async fn register_custom_commands(&self, rules: &[RoutingRuleConfig]) {
        self.registrar
            .register_custom_commands(rules, &self.conflict_resolver)
            .await;
        self.health.invalidate_all();
    }

    // =========================================================================
    // Conflict Resolution (Flat Namespace)
    // =========================================================================

    /// Check if a command name conflicts with an existing tool
    pub async fn check_conflict(&self, name: &str) -> Option<super::types::ConflictInfo> {
        self.conflict_resolver.check_conflict(name).await
    }

    /// Register a tool with automatic conflict resolution
    pub async fn register_with_conflict_resolution(&self, tool: UnifiedTool) -> String {
        let id = self
            .conflict_resolver
            .register_with_conflict_resolution(tool)
            .await;
        self.health.invalidate_all();
        id
    }

    // =========================================================================
    // State Management
    // =========================================================================

    /// Clear all registered tools
    pub async fn clear(&self) {
        self.state.clear().await;
        // Tool membership changed — invalidate the health cache so the
        // next `generate_smart_prompt` re-evaluates from a clean slate.
        self.health.invalidate_all();
    }

    /// Atomic refresh - build new `HashMap` and replace in one operation
    pub async fn refresh_atomic(&self, new_tools: Vec<UnifiedTool>) {
        self.state.refresh_atomic(new_tools).await;
        self.health.invalidate_all();
    }

    /// Remove tools from a specific MCP server
    pub async fn remove_by_mcp_server(&self, server_name: &str) -> usize {
        let n = self.state.remove_by_mcp_server(server_name).await;
        if n > 0 {
            self.health.invalidate_all();
        }
        n
    }

    /// Set tool active state
    pub async fn set_tool_active(&self, id: &str, active: bool) -> bool {
        let changed = self.state.set_tool_active(id, active).await;
        if changed {
            self.health.invalidate_all();
        }
        changed
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

    /// List all tools for UI display (sorted by `sort_order`, then name)
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

    /// Suggest up to `max` command names closest to an unknown `needle` for
    /// "did you mean?" replies (scores canonical names + aliases).
    pub async fn suggest_commands(&self, needle: &str, max: usize) -> Vec<String> {
        self.query.suggest_commands(needle, max).await
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
}

#[cfg(test)]
mod tests;
