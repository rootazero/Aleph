//! Tool Registration Methods
//!
//! Methods for registering tools from various sources.

use tracing::{debug, info, warn};

use crate::config::RoutingRuleConfig;
use crate::mcp::types::McpToolInfo;
use crate::skills::SkillInfo;

use super::super::types::{ChannelType, ToolSafetyLevel, ToolSource, UnifiedTool};
use super::conflict::ConflictResolver;
use super::helpers::{extract_command_name, truncate_description};
use super::types::ToolStorage;

/// Infer default visible_channels based on safety level
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

/// Registration functionality for ToolRegistry
pub struct ToolRegistrar {
    tools: ToolStorage,
}

impl ToolRegistrar {
    /// Create a new registrar with the given storage
    pub fn new(tools: ToolStorage) -> Self {
        Self { tools }
    }

    /// Register builtin tools
    ///
    /// Registers system builtin tools including generation capabilities.
    /// These tools have the highest priority in conflict resolution.
    pub async fn register_builtin_tools(&self, conflict_resolver: &ConflictResolver) {
        debug!("Registering builtin tools");

        // Image generation tool
        let image_generate = UnifiedTool::new(
            "builtin:generate_image",
            "generate_image",
            "Generate images from text descriptions using AI models like DALL-E 3",
            ToolSource::Builtin,
        )
        .with_icon("photo.badge.plus")
        .with_usage("/generate_image A beautiful sunset over mountains")
        .with_localization_key("tool.generate_image")
        .with_sort_order(60);

        conflict_resolver
            .register_with_conflict_resolution(image_generate)
            .await;

        // Speech generation tool
        let speech_generate = UnifiedTool::new(
            "builtin:generate_speech",
            "generate_speech",
            "Convert text to speech using AI voices",
            ToolSource::Builtin,
        )
        .with_icon("speaker.wave.3")
        .with_usage("/generate_speech Hello, how are you?")
        .with_localization_key("tool.generate_speech")
        .with_sort_order(61);

        conflict_resolver
            .register_with_conflict_resolution(speech_generate)
            .await;

        // Skill reading tools (for Progressive Disclosure pattern)
        let read_skill = UnifiedTool::new(
            "builtin:read_skill",
            "read_skill",
            "Read the instructions of an installed skill. Use this to load skill-specific guidance before executing tasks that match a skill's purpose.",
            ToolSource::Builtin,
        )
        .with_icon("doc.text.magnifyingglass")
        .with_usage("/read_skill refine-text")
        .with_localization_key("tool.read_skill")
        .with_sort_order(70);

        conflict_resolver
            .register_with_conflict_resolution(read_skill)
            .await;

        let list_skills = UnifiedTool::new(
            "builtin:list_skills",
            "list_skills",
            "List all available skills installed on the system. Use this to discover what skills are available.",
            ToolSource::Builtin,
        )
        .with_icon("list.bullet.rectangle")
        .with_usage("/list_skills")
        .with_localization_key("tool.list_skills")
        .with_sort_order(71);

        conflict_resolver
            .register_with_conflict_resolution(list_skills)
            .await;

        let snapshot_capture = UnifiedTool::new(
            "builtin:snapshot_capture",
            "snapshot_capture",
            "Capture a system snapshot with AX tree and optional vision OCR",
            ToolSource::Builtin,
        )
        .with_icon("camera")
        .with_usage("/snapshot_capture {\"include_ax\": true, \"include_vision\": false}")
        .with_localization_key("tool.snapshot_capture")
        .with_sort_order(72);

        conflict_resolver
            .register_with_conflict_resolution(snapshot_capture)
            .await;

        // Agent switching command
        let switch_cmd = UnifiedTool::new(
            "builtin:switch",
            "switch",
            "Switch to a different AI agent",
            ToolSource::Builtin,
        )
        .with_usage("/switch <agent_id>")
        .with_sort_order(80);

        conflict_resolver
            .register_with_conflict_resolution(switch_cmd)
            .await;

        // Group chat command
        let groupchat_cmd = UnifiedTool::new(
            "builtin:groupchat",
            "groupchat",
            "Start, end, or manage a multi-persona group chat",
            ToolSource::Builtin,
        )
        .with_usage("/groupchat start <personas> [topic]")
        .with_sort_order(81);

        conflict_resolver
            .register_with_conflict_resolution(groupchat_cmd)
            .await;

        info!("Registered 7 builtin tools (2 generation + 2 skill reading + snapshot + switch + groupchat)");
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
    /// * `conflict_resolver` - Conflict resolver for handling name conflicts
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
        conflict_resolver: &ConflictResolver,
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

            // Infer channel visibility from safety level
            let visible = infer_visible_channels(&tool);
            let tool = if !visible.is_empty() {
                tool.with_visible_channels(visible)
            } else {
                tool
            };

            // Register with automatic conflict resolution
            conflict_resolver
                .register_with_conflict_resolution(tool)
                .await;
        }

        debug!(
            "Registered {} MCP tools from server '{}' (flat namespace)",
            mcp_tools.len(),
            server_name
        );
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
    /// * `conflict_resolver` - Conflict resolver for handling name conflicts
    ///
    /// # Conflict Resolution
    ///
    /// Skills have the lowest priority, so they will be renamed if they
    /// conflict with any other tool type.
    ///
    /// Priority: Builtin > Native > Custom > MCP > Skill
    pub async fn register_skills(&self, skills: &[SkillInfo], conflict_resolver: &ConflictResolver) {
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

            // Infer channel visibility from safety level
            let visible = infer_visible_channels(&tool);
            let tool = if !visible.is_empty() {
                tool.with_visible_channels(visible)
            } else {
                tool
            };

            // Register with automatic conflict resolution
            conflict_resolver
                .register_with_conflict_resolution(tool)
                .await;
        }

        debug!("Registered {} skills (flat namespace)", skills.len());
    }


    /// Register plugin tools from plugin manifests (Flat Namespace Mode)
    ///
    /// In flat namespace mode, plugin tools are registered as root-level commands
    /// with automatic conflict resolution. Users can invoke them directly
    /// via `/{tool_name}` without a prefix.
    ///
    /// # Arguments
    ///
    /// * `tools` - List of (plugin_id, tool_name, tool_description) tuples
    /// * `conflict_resolver` - Conflict resolver for handling name conflicts
    ///
    /// # Conflict Resolution
    ///
    /// Plugin tools have priority between Skill (lowest) and MCP.
    ///
    /// Priority: Builtin > Native > Custom > MCP > Plugin > Skill
    pub async fn register_plugin_tools(
        &self,
        tools: &[(String, String, String)],
        conflict_resolver: &ConflictResolver,
    ) {
        for (plugin_id, tool_name, tool_desc) in tools {
            let id = format!("plugin:{}:{}", plugin_id, tool_name);

            let tool = UnifiedTool::new(
                &id,
                tool_name,
                tool_desc,
                ToolSource::Plugin {
                    plugin_id: plugin_id.clone(),
                },
            )
            .with_display_name(tool_name)
            .with_icon("puzzlepiece.extension")
            .with_usage(format!("/{} [input]", tool_name))
            .with_routing_regex(format!(r"^/{}\s*", regex::escape(tool_name)))
            .with_routing_strip_prefix(true);

            conflict_resolver
                .register_with_conflict_resolution(tool)
                .await;
        }

        debug!("Registered {} plugin tools (flat namespace)", tools.len());
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
}
