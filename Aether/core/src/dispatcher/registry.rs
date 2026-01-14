//! Tool Registry - Unified Tool Aggregation
//!
//! Aggregates tools from all sources (Native, MCP, Skills, Custom) into
//! a single queryable registry.

use crate::config::RoutingRuleConfig;
use crate::mcp::types::McpToolInfo;
use crate::skills::SkillInfo;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::types::{ConflictInfo, ConflictResolution, ToolSource, UnifiedTool};
#[cfg(test)]
use super::types::ToolPriority;

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

    /// Register builtin tools (deprecated - AI-first architecture)
    ///
    /// In AI-first architecture, there are no builtin commands.
    /// All tool selection is handled by the AI through MCP capability.
    /// This method is kept for API compatibility but does nothing.
    pub async fn register_builtin_tools(&self) {
        // No-op in AI-first architecture
        debug!("register_builtin_tools called (no-op in AI-first mode)");
    }

    /// Register MCP tools from tool info list (Flat Namespace Mode)
    ///
    /// In flat namespace mode, MCP tools are registered as root-level commands
    /// with automatic conflict resolution. Users can invoke them directly
    /// via `/{tool_name}` without the `/mcp` prefix.
    ///
    /// # Arguments
    ///
    /// * `mcp_tools` - List of MCP tool info from McpClient
    /// * `server_name` - Name of the MCP server (e.g., "fs", "git", "github")
    /// * `is_builtin` - Whether this is a builtin System Tool
    ///
    /// # Conflict Resolution
    ///
    /// If an MCP tool name conflicts with an existing tool:
    /// - Higher priority tools keep the original name
    /// - Lower priority tools are renamed with `-mcp` suffix
    ///
    /// Priority: Builtin > Native > Custom > MCP > Skill
    pub async fn register_mcp_tools(
        &self,
        mcp_tools: &[McpToolInfo],
        server_name: &str,
        is_builtin: bool,
    ) {
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
            .with_requires_confirmation(tool_info.requires_confirmation)
            .with_icon("bolt.fill") // Default MCP icon
            .with_usage(format!("/{} [args]", tool_info.name))
            // Generate routing regex for flat namespace
            .with_routing_regex(format!(r"^/{}\s*", regex::escape(&tool_info.name)))
            .with_routing_intent_type(format!("mcp:{}", tool_info.name))
            .with_routing_strip_prefix(true);

            // Mark builtin system tools for clarity
            let tool = if is_builtin {
                tool.with_display_name(format!("{} (System)", tool_info.name))
            } else {
                tool.with_display_name(&tool_info.name)
            };

            // Register with automatic conflict resolution
            self.register_with_conflict_resolution(tool).await;
        }

        debug!(
            "Registered {} MCP tools from server '{}' (flat namespace)",
            mcp_tools.len(),
            server_name
        );
    }

    /// Register native AgentTools (DEPRECATED)
    ///
    /// This method is deprecated. Native tools are now handled by rig-core's
    /// Tool trait and McpToolWrapper. Use register_mcp_tools() instead.
    #[deprecated(note = "Use rig-core tools and register_mcp_tools instead")]
    pub async fn register_agent_tools<T>(&self, _tools: &[Arc<T>], _service_name: &str) {
        // No-op - legacy method kept for API compatibility
        debug!("register_agent_tools called (deprecated, no-op)");
    }

    /// Register skills from SkillInfo list (Flat Namespace Mode)
    ///
    /// In flat namespace mode, skills are registered as root-level commands
    /// with automatic conflict resolution. Users can invoke them directly
    /// via `/{skill_id}` without the `/skill` prefix.
    ///
    /// # Arguments
    ///
    /// * `skills` - List of installed skill info
    ///
    /// # Conflict Resolution
    ///
    /// Skills have the lowest priority, so they will be renamed if they
    /// conflict with any other tool type.
    ///
    /// Priority: Builtin > Native > Custom > MCP > Skill
    pub async fn register_skills(&self, skills: &[SkillInfo]) {
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
            .with_display_name(&skill.name)
            .with_icon("lightbulb.fill") // Default Skill icon
            .with_usage(format!("/{} [input]", skill.id))
            // Generate routing regex for flat namespace
            .with_routing_regex(format!(r"^/{}\s*", regex::escape(&skill.id)))
            .with_routing_intent_type("skills")
            .with_routing_capabilities(vec!["skills".to_string(), "memory".to_string()])
            .with_routing_strip_prefix(true);

            // Register with automatic conflict resolution
            self.register_with_conflict_resolution(tool).await;
        }

        debug!("Registered {} skills (flat namespace)", skills.len());
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
            // Skip builtin rules - they are registered via register_builtin_tools()
            if rule.is_builtin {
                continue;
            }

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

    // =========================================================================
    // Conflict Resolution (Flat Namespace)
    // =========================================================================

    /// Check if a command name conflicts with an existing tool
    ///
    /// Returns conflict information if a tool with the same name already exists.
    /// The name comparison is case-insensitive.
    ///
    /// # Arguments
    ///
    /// * `name` - The command name to check
    ///
    /// # Returns
    ///
    /// `Some(ConflictInfo)` if a conflict exists, `None` otherwise
    pub async fn check_conflict(&self, name: &str) -> Option<ConflictInfo> {
        let tools = self.tools.read().await;
        let name_lower = name.to_lowercase();

        for tool in tools.values() {
            if tool.name.to_lowercase() == name_lower {
                return Some(ConflictInfo {
                    existing_id: tool.id.clone(),
                    existing_name: tool.name.clone(),
                    existing_source: tool.source.clone(),
                    existing_priority: tool.source.priority(),
                });
            }
        }
        None
    }

    /// Resolve a naming conflict between two tools
    ///
    /// Determines which tool wins (keeps original name) and which tool
    /// gets renamed with a suffix based on priority.
    ///
    /// Priority order (highest to lowest):
    /// 1. Builtin - System commands (/search, /youtube, /webfetch)
    /// 2. Native - System capabilities
    /// 3. Custom - User-defined rules
    /// 4. MCP - External MCP tools
    /// 5. Skill - Claude Agent skills
    ///
    /// # Arguments
    ///
    /// * `name` - The original command name
    /// * `conflict` - Information about the existing conflicting tool
    /// * `new_source` - The source of the new tool being registered
    ///
    /// # Returns
    ///
    /// `ConflictResolution` indicating which tool should be renamed
    pub fn resolve_conflict(
        &self,
        name: &str,
        conflict: &ConflictInfo,
        new_source: &ToolSource,
    ) -> ConflictResolution {
        let new_priority = new_source.priority();

        if new_priority > conflict.existing_priority {
            // New tool wins, rename existing
            ConflictResolution::RenameExisting {
                original_name: name.to_string(),
                new_name: format!("{}-{}", name, conflict.existing_source.suffix()),
            }
        } else if new_priority < conflict.existing_priority {
            // Existing wins, rename new
            ConflictResolution::RenameNew {
                original_name: name.to_string(),
                new_name: format!("{}-{}", name, new_source.suffix()),
            }
        } else {
            // Same priority - new tool gets renamed (first registered wins)
            ConflictResolution::RenameNew {
                original_name: name.to_string(),
                new_name: format!("{}-{}", name, new_source.suffix()),
            }
        }
    }

    /// Apply conflict resolution by renaming an existing tool
    ///
    /// This is called when a higher-priority tool needs to take over
    /// a name from an existing lower-priority tool.
    ///
    /// # Arguments
    ///
    /// * `existing_id` - The ID of the existing tool to rename
    /// * `new_name` - The new name for the existing tool
    ///
    /// # Returns
    ///
    /// `true` if the tool was found and renamed, `false` otherwise
    pub async fn rename_existing_tool(&self, existing_id: &str, new_name: &str) -> bool {
        let mut tools = self.tools.write().await;

        if let Some(mut tool) = tools.remove(existing_id) {
            let original_name = tool.name.clone();
            tool.original_name = Some(original_name.clone());
            tool.was_renamed = true;
            tool.name = new_name.to_string();
            tool.display_name = format!("{} (renamed)", new_name);

            // Update ID to reflect new name
            let new_id = match &tool.source {
                ToolSource::Native => format!("native:{}", new_name),
                ToolSource::Builtin => format!("builtin:{}", new_name),
                ToolSource::Mcp { server } => format!("mcp:{}:{}", server, new_name),
                ToolSource::Skill { id } => format!("skill:{}", id), // Keep skill ID
                ToolSource::Custom { rule_index } => format!("custom:{}:{}", rule_index, new_name),
            };

            debug!(
                "Tool conflict resolved: '{}' renamed to '{}' (priority system)",
                original_name, new_name
            );

            tool.id = new_id.clone();
            tools.insert(new_id, tool);
            true
        } else {
            false
        }
    }

    /// Register a tool with automatic conflict resolution
    ///
    /// This is the preferred way to register tools in flat namespace mode.
    /// It automatically handles name conflicts according to priority rules.
    ///
    /// # Arguments
    ///
    /// * `tool` - The tool to register
    ///
    /// # Returns
    ///
    /// The final tool ID after registration (may differ from input if renamed)
    pub async fn register_with_conflict_resolution(&self, mut tool: UnifiedTool) -> String {
        // Check for conflict
        if let Some(conflict) = self.check_conflict(&tool.name).await {
            let resolution = self.resolve_conflict(&tool.name, &conflict, &tool.source);

            match resolution {
                ConflictResolution::RenameExisting { original_name, new_name } => {
                    // Rename the existing tool
                    self.rename_existing_tool(&conflict.existing_id, &new_name).await;
                    info!(
                        "Conflict resolved: existing tool '{}' renamed to '{}', new tool '{}' takes priority",
                        original_name, new_name, tool.name
                    );
                }
                ConflictResolution::RenameNew { original_name, new_name } => {
                    // Rename the new tool
                    tool.original_name = Some(original_name.clone());
                    tool.was_renamed = true;
                    tool.name = new_name.clone();
                    tool.display_name = format!("{} ({})", new_name, tool.source.label());

                    // Update tool ID
                    tool.id = match &tool.source {
                        ToolSource::Native => format!("native:{}", new_name),
                        ToolSource::Builtin => format!("builtin:{}", new_name),
                        ToolSource::Mcp { server } => format!("mcp:{}:{}", server, new_name),
                        ToolSource::Skill { id } => format!("skill:{}", id),
                        ToolSource::Custom { rule_index } => format!("custom:{}:{}", rule_index, new_name),
                    };

                    debug!(
                        "Tool conflict resolved: '{}' renamed to '{}' (existing '{}' has priority)",
                        original_name, new_name, conflict.existing_name
                    );
                }
                ConflictResolution::NoConflict => {
                    // Should not happen if check_conflict returned Some
                }
            }
        }

        let id = tool.id.clone();
        let mut tools = self.tools.write().await;
        tools.insert(id.clone(), tool);
        id
    }

    /// Clear all registered tools
    pub async fn clear(&self) {
        let mut tools = self.tools.write().await;
        tools.clear();
        debug!("Cleared all tools from registry");
    }

    /// Atomic refresh - build new HashMap and replace in one operation
    ///
    /// This method prevents the race condition where `clear()` and `register()`
    /// have a brief window of empty tool list. Instead, we build a completely
    /// new HashMap with all tools, then atomically replace the old one.
    ///
    /// # Arguments
    ///
    /// * `new_tools` - Vector of tools to register (replaces all existing)
    ///
    /// # Thread Safety
    ///
    /// This uses a single write lock operation, so UI will never see
    /// an empty or partially populated tool list during refresh.
    pub async fn refresh_atomic(&self, new_tools: Vec<UnifiedTool>) {
        let new_map: HashMap<String, UnifiedTool> = new_tools
            .into_iter()
            .map(|t| (t.id.clone(), t))
            .collect();

        let count = new_map.len();

        // Single write lock operation - atomic replacement
        let mut tools = self.tools.write().await;
        *tools = new_map;
        // Lock released here - UI immediately sees new tools, no empty window

        info!("Tool registry atomically refreshed: {} tools", count);
    }

    // =========================================================================
    // Incremental Update Methods (Phase 2.3)
    // =========================================================================

    /// Remove all tools of a specific source type
    ///
    /// This enables incremental updates - only refresh the affected source
    /// instead of clearing and re-registering everything.
    ///
    /// # Arguments
    ///
    /// * `source_type` - The source type to remove (Skill, Mcp, Custom, etc.)
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_by_source_type(&self, source_type: super::types::ToolSourceType) -> usize {
        let mut tools = self.tools.write().await;
        let initial_count = tools.len();

        tools.retain(|_, tool| {
            super::types::ToolSourceType::from(&tool.source) != source_type
        });

        let removed = initial_count - tools.len();
        debug!(
            source_type = ?source_type,
            removed = removed,
            "Removed tools by source type"
        );
        removed
    }

    /// Remove tools from a specific MCP server
    ///
    /// Used when restarting or removing a single MCP server without
    /// affecting other servers or tool sources.
    ///
    /// # Arguments
    ///
    /// * `server_name` - The MCP server name to remove tools for
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_by_mcp_server(&self, server_name: &str) -> usize {
        let mut tools = self.tools.write().await;
        let initial_count = tools.len();

        tools.retain(|_, tool| {
            match &tool.source {
                super::types::ToolSource::Mcp { server } => server != server_name,
                _ => true,
            }
        });

        let removed = initial_count - tools.len();
        debug!(
            server = server_name,
            removed = removed,
            "Removed MCP server tools"
        );
        removed
    }

    /// Remove all skill tools
    ///
    /// Used when refreshing skills without affecting other tool sources.
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_skills(&self) -> usize {
        self.remove_by_source_type(super::types::ToolSourceType::Skill).await
    }

    /// Remove all custom commands
    ///
    /// Used when updating routing rules without affecting other tool sources.
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_custom_commands(&self) -> usize {
        self.remove_by_source_type(super::types::ToolSourceType::Custom).await
    }

    /// Remove all MCP tools (from all servers)
    ///
    /// Used when refreshing all MCP servers.
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_all_mcp_tools(&self) -> usize {
        self.remove_by_source_type(super::types::ToolSourceType::Mcp).await
    }

    /// Remove all native tools
    ///
    /// Used when refreshing native tool configuration.
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_native_tools(&self) -> usize {
        self.remove_by_source_type(super::types::ToolSourceType::Native).await
    }

    /// Refresh all tools from all sources
    ///
    /// This method aggregates tools from all sources into a unified registry.
    ///
    /// # Arguments
    ///
    /// * `mcp_tools` - External MCP server tools (server_name, tools)
    /// * `skills` - Installed Claude Agent skills
    /// * `rules` - User-defined routing rules
    ///
    /// # Registration Order
    ///
    /// 1. Builtin commands (if any)
    /// 2. External MCP tools
    /// 3. Skills
    /// 4. Custom commands from config
    pub async fn refresh_all(
        &self,
        mcp_tools: &[(String, Vec<McpToolInfo>)],
        skills: &[SkillInfo],
        rules: &[RoutingRuleConfig],
    ) {
        self.clear().await;

        // 1. Builtin commands first (currently no-op in AI-first mode)
        self.register_builtin_tools().await;

        // 2. External MCP tools
        for (server_name, tools) in mcp_tools {
            self.register_mcp_tools(tools, server_name, false).await;
        }

        // 3. Skills
        self.register_skills(skills).await;

        // 4. Custom commands from user config
        self.register_custom_commands(rules).await;

        let count = self.tools.read().await.len();
        info!(
            "Tool registry refreshed: {} total tools",
            count
        );
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
    /// Returns the 3 system builtin commands (/search, /youtube, /webfetch)
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

    /// List preset tools for Settings UI (Flat Namespace Mode)
    ///
    /// Returns all non-Custom tools: Builtin + MCP + Skill + Native
    /// These are the "preset" tools that users can't delete, sorted by priority.
    pub async fn list_preset_tools(&self) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut presets: Vec<_> = tools
            .values()
            .filter(|t| {
                t.is_active && !matches!(t.source, ToolSource::Custom { .. })
            })
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
        let mut result: Vec<_> = tools
            .values()
            .filter(|t| t.is_active)
            .cloned()
            .collect();

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
        // AI-first architecture: no builtin tools
        let registry = ToolRegistry::new();
        registry.register_builtin_tools().await;

        // Should register 0 tools (AI-first mode)
        assert_eq!(registry.count().await, 0);

        // No builtins should exist
        let builtins = registry.list_builtin_tools().await;
        assert_eq!(builtins.len(), 0);
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
        // AI-first: 0 builtins + 1 custom = 1
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].name, "en");
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
        // AI-first: no builtin tools
        let skills = vec![SkillInfo {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: "Test skill".to_string(),
            allowed_tools: vec![],
        }];
        registry.register_skills(&skills).await;

        let builtin = registry.list_by_source_type("Builtin").await;
        assert_eq!(builtin.len(), 0); // AI-first: no builtins

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
            existing_source: ToolSource::Mcp { server: "server".into() },
            existing_priority: ToolPriority::Mcp,
        };

        let resolution = registry.resolve_conflict(
            "search",
            &conflict,
            &ToolSource::Builtin,
        );

        // Builtin has higher priority, should rename existing
        match resolution {
            ConflictResolution::RenameExisting { original_name, new_name } => {
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
            &ToolSource::Mcp { server: "server".into() },
        );

        // Builtin has higher priority, should rename new
        match resolution {
            ConflictResolution::RenameNew { original_name, new_name } => {
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
            existing_source: ToolSource::Mcp { server: "server1".into() },
            existing_priority: ToolPriority::Mcp,
        };

        let resolution = registry.resolve_conflict(
            "status",
            &conflict,
            &ToolSource::Mcp { server: "server2".into() },
        );

        // Same priority - new tool gets renamed (first registered wins)
        match resolution {
            ConflictResolution::RenameNew { original_name, new_name } => {
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
            ToolSource::Mcp { server: "server".into() },
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
        registry.register_with_conflict_resolution(custom_tool).await;

        // Try to register MCP tool with same name as custom
        let mcp_tool = UnifiedTool::new(
            "mcp:server:search",
            "search",
            "MCP Search",
            ToolSource::Mcp { server: "server".into() },
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
            ToolSource::Mcp { server: "server".into() },
        );
        registry.register_with_conflict_resolution(mcp_tool).await;

        // Register Custom tool with same name (higher priority)
        let custom_tool = UnifiedTool::new(
            "custom:test",
            "test",
            "Custom Test",
            ToolSource::Custom { rule_index: 0 },
        );
        let id = registry.register_with_conflict_resolution(custom_tool).await;

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
        let rules = vec![
            RoutingRuleConfig {
                regex: "^/old".to_string(),
                provider: Some("openai".to_string()),
                system_prompt: Some("Old command".to_string()),
                ..Default::default()
            },
        ];
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

// integration_tests module removed - AgentTool system deprecated
