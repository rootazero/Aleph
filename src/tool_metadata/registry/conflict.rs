//! Conflict Resolution for Flat Namespace
//!
//! Handles name conflicts when registering tools from different sources.

use tracing::{debug, info};

use super::super::types::{
    ChannelType, ConflictInfo, ConflictResolution, ToolSafetyLevel, ToolSource, UnifiedTool,
};
use super::types::ToolStorage;

/// Infer default `visible_channels` from a tool's safety profile.
///
/// Returns an empty vec ("visible to all channels") for ordinary tools, and a
/// restricted set for risky ones — e.g. dangerous operations stay on Panel/CLI,
/// and confirmation-requiring tools are excluded from channels without a
/// confirmation UI (iMessage). Applied uniformly to every tool registered
/// through [`ConflictResolver::register_with_conflict_resolution`].
fn infer_visible_channels(tool: &UnifiedTool) -> Vec<ChannelType> {
    match tool.safety_level {
        ToolSafetyLevel::IrreversibleHighRisk => {
            // Dangerous ops only via Panel and CLI
            vec![ChannelType::Panel, ChannelType::Cli]
        }
        _ if tool.requires_confirmation => {
            // Tools requiring confirmation excluded from iMessage (no confirmation UI)
            vec![
                ChannelType::Panel,
                ChannelType::Telegram,
                ChannelType::Discord,
                ChannelType::Cli,
            ]
        }
        _ => Vec::new(), // All channels
    }
}

/// Conflict resolver for handling tool name conflicts
pub struct ConflictResolver {
    tools: ToolStorage,
}

impl ConflictResolver {
    /// Create a new conflict resolver with the given storage
    pub const fn new(tools: ToolStorage) -> Self {
        Self { tools }
    }

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
            tool.display_name = format!("{new_name} (renamed)");

            // Update ID to reflect new name
            let new_id = match &tool.source {
                ToolSource::Native => format!("native:{new_name}"),
                ToolSource::Builtin => format!("builtin:{new_name}"),
                ToolSource::Mcp { server } => format!("mcp:{server}:{new_name}"),
                ToolSource::Skill { id } => format!("skill:{id}"), // Keep skill ID
                ToolSource::Custom { rule_index } => format!("custom:{rule_index}:{new_name}"),
                ToolSource::Plugin { plugin_id } => format!("plugin:{plugin_id}:{new_name}"),
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
    /// Uses a single write lock to prevent TOCTOU races between conflict
    /// check and tool insertion.
    ///
    /// Aliases participate in conflict detection: a later tool whose `name`
    /// or any `alias` collides with an existing canonical name (or alias)
    /// is treated as a conflict and resolved via the same priority rules.
    /// This stops a low-priority registrant from shadowing a high-priority
    /// builtin alias like `/model` or `/compact`.
    ///
    /// # Arguments
    ///
    /// * `tool` - The tool to register
    ///
    /// # Returns
    ///
    /// The final tool ID after registration (may differ from input if renamed)
    pub async fn register_with_conflict_resolution(&self, mut tool: UnifiedTool) -> String {
        // Centralized channel-visibility inference: tools that don't declare an
        // explicit `visible_channels` set inherit one derived from their safety
        // profile. Explicit declarations are preserved.
        if tool.visible_channels.is_empty() {
            tool.visible_channels = infer_visible_channels(&tool);
        }

        let mut tools = self.tools.write().await;

        // Inline conflict check under write lock (no TOCTOU race).
        // Match against the new tool's canonical name OR any of its aliases.
        let name_lower = tool.name.to_lowercase();
        let alias_lowers: Vec<String> = tool
            .aliases
            .iter()
            .map(|a| a.to_lowercase())
            .collect();
        let conflict = tools
            .values()
            .find(|t| {
                let existing_name_lower = t.name.to_lowercase();
                if existing_name_lower == name_lower {
                    return true;
                }
                // New's aliases vs existing canonical name.
                if alias_lowers.contains(&existing_name_lower) {
                    return true;
                }
                let existing_alias_lowers: Vec<String> = t
                    .aliases
                    .iter()
                    .map(|a| a.to_lowercase())
                    .collect();
                // Alias↔alias collisions: e.g. plugin `aliases = ["x"]` vs
                // builtin `aliases = ["x"]`. Two tools claiming the same
                // shortcut must resolve via the priority rules, not silently
                // shadow each other.
                existing_alias_lowers
                    .iter()
                    .any(|ea| alias_lowers.contains(ea))
            })
            .map(|t| ConflictInfo {
                existing_id: t.id.clone(),
                existing_name: t.name.clone(),
                existing_source: t.source.clone(),
                existing_priority: t.source.priority(),
            });

        if let Some(conflict) = conflict {
            let resolution = self.resolve_conflict(&tool.name, &conflict, &tool.source);

            match resolution {
                ConflictResolution::RenameExisting {
                    original_name,
                    new_name,
                } => {
                    // Rename the existing tool inline (under same write lock)
                    if let Some(mut existing) = tools.remove(&conflict.existing_id) {
                        let orig_name = existing.name.clone();
                        existing.original_name = Some(orig_name.clone());
                        existing.was_renamed = true;
                        existing.name = new_name.clone();
                        existing.display_name = format!("{new_name} (renamed)");

                        let new_id = existing.source.format_tool_id(&new_name);

                        debug!(
                            "Tool conflict resolved: '{}' renamed to '{}' (priority system)",
                            orig_name, new_name
                        );

                        existing.id = new_id.clone();
                        tools.insert(new_id, existing);
                    }

                    info!(
                        "Conflict resolved: existing tool '{}' renamed to '{}', new tool '{}' takes priority",
                        original_name, new_name, tool.name
                    );
                }
                ConflictResolution::RenameNew {
                    original_name,
                    new_name,
                } => {
                    // Rename the new tool
                    tool.original_name = Some(original_name.clone());
                    tool.was_renamed = true;
                    tool.name = new_name.clone();
                    tool.display_name = format!("{} ({})", new_name, tool.source.label());

                    // Update tool ID
                    tool.id = tool.source.format_tool_id(&new_name);

                    debug!(
                        "Tool conflict resolved: '{}' renamed to '{}' (existing '{}' has priority)",
                        original_name, new_name, conflict.existing_name
                    );
                }
                ConflictResolution::NoConflict => {
                    // Should not happen if conflict was detected
                }
            }
        }

        let id = tool.id.clone();
        tools.insert(id.clone(), tool);
        id
    }
}
