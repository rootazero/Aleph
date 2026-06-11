//! Tool Registration Methods
//!
//! Methods for registering tools from various sources.

use tracing::{debug, info, warn};

use crate::config::RoutingRuleConfig;
use crate::skill::SkillInfo;

use super::super::types::{ToolSource, UnifiedTool};
use super::conflict::ConflictResolver;
use super::helpers::{extract_command_name, truncate_description};
use super::types::ToolStorage;

/// Registration functionality for ToolCatalog
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
        .with_usage("/generate image A beautiful sunset over mountains")
        .with_param_hint("<prompt>")
        .with_localization_key("tool.generate.image")
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
        .with_usage("/generate speech Hello, how are you?")
        .with_param_hint("<text>")
        .with_localization_key("tool.generate.speech")
        .with_sort_order(61);

        conflict_resolver
            .register_with_conflict_resolution(speech_generate)
            .await;

        // Skill reading tools (for Progressive Disclosure pattern)
        let read_skill = UnifiedTool::new(
            "builtin:skill_read",
            "skill_read",
            "Read the instructions of an installed skill. Use this to load skill-specific guidance before executing tasks that match a skill's purpose.",
            ToolSource::Builtin,
        )
        .with_icon("doc.text.magnifyingglass")
        .with_usage("/skill read refine-text")
        .with_param_hint("<skill-id>")
        .with_localization_key("tool.skill.read")
        .with_sort_order(70);

        conflict_resolver
            .register_with_conflict_resolution(read_skill)
            .await;

        let list_skills = UnifiedTool::new(
            "builtin:skill_list",
            "skill_list",
            "List all available skills installed on the system. Use this to discover what skills are available.",
            ToolSource::Builtin,
        )
        .with_icon("list.bullet.rectangle")
        .with_usage("/skill list")
        .with_localization_key("tool.skill.list")
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
        .with_usage("/snapshot capture")
        .with_localization_key("tool.snapshot.capture")
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
        .with_usage("/switch <agent>")
        .with_param_hint("<agent>")
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
        .with_param_hint("[personas]")
        .with_sort_order(81);

        conflict_resolver
            .register_with_conflict_resolution(groupchat_cmd)
            .await;

        // New session command (aligned with CLI: `aleph session new`).
        // `/new` is exposed as a first-class alias (the most common shortcut in
        // bots) instead of a separate phantom tool — both names resolve to this
        // single registration via the unified alias mechanism.
        let new_cmd = UnifiedTool::new(
            "builtin:session_new",
            "session_new",
            "Start a new conversation session",
            ToolSource::Builtin,
        )
        .with_aliases(["new"])
        .with_usage("/session new")
        .with_param_hint("[topic]")
        .with_sort_order(82);

        conflict_resolver
            .register_with_conflict_resolution(new_cmd)
            .await;

        // Cron management command
        let cron_cmd = UnifiedTool::new(
            "builtin:cron_manage",
            "cron_manage",
            "Manage scheduled tasks",
            ToolSource::Builtin,
        )
        .with_usage("/cron manage list | /cron manage create <task>")
        .with_sort_order(83);

        conflict_resolver
            .register_with_conflict_resolution(cron_cmd)
            .await;

        // Voice mode command (direct handler in router, like /new)
        let voice_cmd = UnifiedTool::new(
            "builtin:voice",
            "voice",
            "Toggle voice mode on/off for the current channel",
            ToolSource::Builtin,
        )
        .with_icon("speaker.wave.3")
        .with_usage("/voice on | /voice off | /voice status")
        .with_param_hint("[on|off|status]")
        .with_sort_order(84);

        conflict_resolver
            .register_with_conflict_resolution(voice_cmd)
            .await;

        info!("Registered builtin tools (generate_* + skill_* + snapshot_capture + switch + groupchat + session_new [alias: new] + cron_manage + voice)");
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
    pub async fn register_skills(
        &self,
        skills: &[SkillInfo],
        conflict_resolver: &ConflictResolver,
    ) {
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

            // Register with automatic conflict resolution. Channel visibility is
            // inferred centrally in `register_with_conflict_resolution`.
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
            let id = format!("plugin:{plugin_id}:{tool_name}");

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
            .with_usage(format!("/{tool_name} [input]"))
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

            let id = format!("custom:{command_name}");

            // Use system_prompt as description if available, otherwise generic
            let description = rule
                .system_prompt
                .as_ref()
                .map(|s| truncate_description(s, 100))
                .unwrap_or_else(|| format!("Custom command /{command_name}"));

            let mut tool = UnifiedTool::new(
                &id,
                &command_name,
                description,
                ToolSource::Custom { rule_index: index },
            )
            .with_display_name(format!("/{command_name}"))
            .with_routing_regex(rule.regex.clone());

            if let Some(ref prompt) = rule.system_prompt {
                tool = tool.with_routing_system_prompt(prompt.clone());
            }

            tools.insert(id, tool);
            count += 1;
        }

        debug!("Registered {} custom commands", count);
    }
}
