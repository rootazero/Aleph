//! Tool Registration Methods
//!
//! Methods for registering tools from various sources.

use tracing::{debug, info, warn};

use crate::config::RoutingRuleConfig;
use crate::skill::SkillInfo;

use super::super::types::{ToolSource, UnifiedTool};
use super::conflict::ConflictResolver;
use super::helpers::{extract_command_name, truncate_description};

/// Registration functionality for `ToolCatalog`
#[derive(Default)]
pub struct ToolRegistrar;

impl ToolRegistrar {
    /// Create a new registrar
    pub const fn new() -> Self {
        Self
    }

    /// Register builtin tools
    ///
    /// Registers the curated multi-word slash commands (skills, groupchat,
    /// session_new, cron, voice, goal, help). These have the highest priority
    /// in conflict resolution. Single-word tool aliases (`/model`, `/image`, …)
    /// are seeded separately in `tool_catalog_init` from `SHORTHAND_ALIASES`.
    pub async fn register_builtin_tools(&self, conflict_resolver: &ConflictResolver) {
        debug!("Registering builtin tools");

        // NOTE: media generation slash commands (/image /video /audio /speech)
        // are NOT curated here. Their real executable tools are named
        // `<media>_generate` (image_generate is in BUILTIN_TOOL_DEFINITIONS;
        // video/audio/speech_generate are provider-gated runtime tools). The
        // catalog surfaces them via the alias-seeding in `tool_catalog_init`
        // (defs loop for image_generate + a runtime-only pass for the other
        // three), all driven by the single `SHORTHAND_ALIASES` source. Two
        // former curated entries here — `generate_image`/`generate_speech` —
        // used the reversed word order, matched no `execute_tool` arm, and so
        // dead-ended on "Unknown tool" while still being advertised in /help;
        // they were removed (see FEATURE_LOCATOR §3.5 round-3).

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
        // `/skills` is the cross-tool-standard top-level name (codex/openclaw/
        // hermes/kimi all use it). Discovery-only alias on the curated entry —
        // skill_list is also in BUILTIN_TOOL_DEFINITIONS but the curated entry
        // wins (first-registered), so the alias must live here, not in the
        // definitions loop where it would attach to the renamed loser.
        .with_aliases(["skills"])
        .with_usage("/skill list")
        .with_localization_key("tool.skill.list")
        .with_sort_order(71);

        conflict_resolver
            .register_with_conflict_resolution(list_skills)
            .await;

        // NOTE: no `snapshot_capture` curated entry — it had no `execute_tool`
        // arm, def, or any backing anywhere, so `/snapshot capture` only ever
        // errored. Removed (FEATURE_LOCATOR §3.5 round-3). Agent switching is
        // surfaced by `/agent`+`/agents` → agent_switch (SHORTHAND_ALIASES,
        // agent_switch is in defs); the former curated `switch` entry had no
        // `switch` arm and duplicated that working path, so it was removed too.

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
        // `/new` and `/clear` are first-class aliases (the most common shortcuts
        // in bots / codex-kimi muscle memory) instead of separate phantom tools
        // — all three names resolve to this single registration via the unified
        // alias mechanism. These are discovery-only aliases (NOT SHORTHAND):
        // `session_new` is `None` in `create_tool_boxed`, so the epoch bump runs
        // in the router's `handle_new_session` (which the resolved canonical name
        // `session_new` routes into), never as a raw fast-path tool.
        let new_cmd = UnifiedTool::new(
            "builtin:session_new",
            "session_new",
            "Start a new conversation session",
            ToolSource::Builtin,
        )
        .with_aliases(["new", "clear"])
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

        // Goal command — surface the autonomous-pursuit tool as a slash command.
        // `goal` is a runtime LoopTool (needs live per-session binding) and is
        // deliberately NOT in BUILTIN_TOOL_DEFINITIONS, so without this curated
        // entry it is LLM-callable but not slash-resolvable. It is
        // continuation-driven (`is_continuation_driven_slash`), so this entry
        // only adds discovery + resolution; execution falls through to the full
        // agent loop where the builder-wired live goal tool schedules the first
        // pursuit (the fast path would register the goal but never tick it).
        let goal_cmd = UnifiedTool::new(
            "builtin:goal",
            "goal",
            "Set an autonomous goal the agent pursues across turns until done",
            ToolSource::Builtin,
        )
        .with_icon("target")
        .with_usage("/goal <objective>")
        .with_param_hint("<objective>")
        .with_sort_order(85);

        conflict_resolver
            .register_with_conflict_resolution(goal_cmd)
            .await;

        // Help command — list available slash commands. The single universal
        // essential absent from Aleph (openclaw/hermes/kimi all ship `/help`).
        // Execution is intercepted in the inbound router (`handle_help`), which
        // formats the live command tree from the `ToolCatalog`; this curated
        // entry makes `/help` discoverable in completion menus and drives the
        // "did you mean?" suggester.
        let help_cmd = UnifiedTool::new(
            "builtin:help",
            "help",
            "List available slash commands and what they do",
            ToolSource::Builtin,
        )
        .with_icon("questionmark.circle")
        .with_usage("/help")
        .with_sort_order(1);

        conflict_resolver
            .register_with_conflict_resolution(help_cmd)
            .await;

        info!("Registered builtin tools (skill_* [alias: skills] + groupchat + session_new [aliases: new, clear] + cron_manage + voice + goal + help)");
    }

    /// Register skills from `SkillInfo` list (Flat Namespace Mode)
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
    /// Priority: Builtin > Native > Custom > MCP > Plugin > Skill
    ///
    /// # Returns
    ///
    /// The ids of skills that were **not** registered because their declared
    /// `allowed-tools:` named tools that do not exist. Returned rather than
    /// only logged so the boot path can say it out loud: a skill silently
    /// missing from the slash catalog reads exactly like a skill that was
    /// never installed.
    pub async fn register_skills(
        &self,
        skills: &[SkillInfo],
        conflict_resolver: &ConflictResolver,
    ) -> Vec<String> {
        let mut rejected: Vec<String> = Vec::new();
        for skill in skills {
            let id = format!("skill:{}", skill.id);

            // Resolve the skill's declared tool scope BEFORE building the
            // tool: an unresolvable declaration means this skill does not get
            // a slash command at all. That matches what the skill system
            // already does with a skill it cannot parse (`skill::scan_directory`
            // skips it with a warn); the alternative — register it with the
            // declaration dropped — hands the model a `/skill` that runs with
            // the full toolbelt the author explicitly tried to shrink.
            let routing_capabilities = match Self::resolve_skill_tool_scope(
                skill.allowed_tools.as_deref(),
                conflict_resolver,
            )
            .await
            {
                Ok(caps) => caps,
                Err(unknown) => {
                    warn!(
                        skill = %skill.id,
                        unknown_tools = ?unknown,
                        "skill declares `allowed-tools:` naming tools that do not exist; \
                         its slash command is NOT registered. Use Aleph tool names \
                         (`file_read`, `bash`, `grep`), not upstream Claude Code names \
                         (`Read`, `Bash`, `Grep`)"
                    );
                    rejected.push(skill.id.clone());
                    continue;
                }
            };

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
            // `routing_system_prompt` carries the skill's description onto
            // `CommandContext::Skill.instructions`. Nothing injects that
            // string into a prompt — the description already reaches the model
            // through the `<available_skills>` block
            // (`thinker::layers::skill_instructions`) and the body only through
            // the `skill_read` tool. It is kept because the field is part of
            // the slash-command envelope's shape, not because a consumer reads
            // it back.
            .with_routing_system_prompt(&skill.description)
            // The validated `allowed-tools:` scope. `None` here is "the skill
            // declared nothing" and must stay distinguishable from
            // `Some(vec![])`, "the skill declared nothing is allowed" — see
            // `UnifiedTool::routing_capabilities`.
            .with_routing_capabilities(routing_capabilities)
            .with_routing_strip_prefix(true);

            // Register with automatic conflict resolution. Channel visibility is
            // inferred centrally in `register_with_conflict_resolution`.
            conflict_resolver
                .register_with_conflict_resolution(tool)
                .await;
        }

        debug!(
            "Registered {} skills (flat namespace), {} rejected",
            skills.len() - rejected.len(),
            rejected.len()
        );
        rejected
    }

    /// Validate a skill's declared `allowed-tools:` against the tool names
    /// that actually exist.
    ///
    /// `Ok(None)` — the skill declared nothing; the run keeps the agent's full
    /// tool surface. `Ok(Some(names))` — every declared name resolves (an
    /// empty `Some` is a legitimate, explicit deny-all). `Err(unknown)` — at
    /// least one name names nothing, and the caller must refuse the skill.
    ///
    /// Two sources are unioned to answer "does this name exist":
    /// * [`crate::executor::BUILTIN_TOOL_DEFINITIONS`] — the executor's own
    ///   static list, and the same one the `ExecutionEngine` seeds its tool
    ///   list from. Consulting it means this answer does not depend on how
    ///   much of the catalog happens to be populated when skills register. A
    ///   guard whose known-set is "whatever registered first" starts rejecting
    ///   valid skills the day boot order changes, and a guard that rejects
    ///   everything looks exactly like a guard that works.
    /// * the catalog as it stands, **restricted to sources that are also LLM
    ///   tool names**. This is not fussiness: the catalog is a *slash-command*
    ///   index, and `Skill` / `Custom` entries live only there. Admitting them
    ///   would let a declaration pass validation and then match nothing in the
    ///   run loop, whose candidate list is built from the executor's tools —
    ///   a silent deny-all, which is the exact failure this change exists to
    ///   remove.
    ///
    /// Known boundary: plugin tools and MCP tools register into the catalog
    /// *after* skills do (MCP joins per request at run time), so a skill that
    /// names one is refused even though the run loop could have honoured it.
    /// That is a loud false negative — the author is named in a warn and in
    /// the boot output — chosen over the silent false positive above. There is
    /// no Claude-Code-name translation table on purpose: a table only covers
    /// the names that existed the day it was written, and matching upstream's
    /// `Read`/`Bash`/`Grep` literally would retain zero tools while reporting
    /// success.
    async fn resolve_skill_tool_scope(
        declared: Option<&[String]>,
        conflict_resolver: &ConflictResolver,
    ) -> Result<Option<Vec<String>>, Vec<String>> {
        let Some(names) = declared else {
            return Ok(None);
        };

        let mut unknown: Vec<String> = Vec::new();
        for name in names {
            if crate::executor::BUILTIN_TOOL_DEFINITIONS
                .iter()
                .any(|def| def.name == name.as_str())
            {
                continue;
            }
            if conflict_resolver
                .check_conflict(name)
                .await
                .is_some_and(|c| {
                    matches!(
                        c.existing_source,
                        ToolSource::Builtin
                            | ToolSource::Native
                            | ToolSource::Mcp { .. }
                            | ToolSource::Plugin { .. }
                    )
                })
            {
                continue;
            }
            unknown.push(name.clone());
        }

        if unknown.is_empty() {
            Ok(Some(names.to_vec()))
        } else {
            Err(unknown)
        }
    }

    /// Register plugin tools from plugin manifests (Flat Namespace Mode)
    ///
    /// In flat namespace mode, plugin tools are registered as root-level commands
    /// with automatic conflict resolution. Users can invoke them directly
    /// via `/{tool_name}` without a prefix.
    ///
    /// # Arguments
    ///
    /// * `tools` - List of (`plugin_id`, `tool_name`, `tool_description`) tuples
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
    /// Routed through `ConflictResolver` so a custom rule whose canonical
    /// name (or any future alias) collides with a higher-priority builtin /
    /// MCP / plugin tool gets renamed instead of silently coexisting in the
    /// flat namespace. Multiple custom rules resolving to the same name are
    /// still distinguished by `rule_index` in the id.
    ///
    /// # Arguments
    ///
    /// * `rules` - Routing rules from config.toml
    /// * `conflict_resolver` - The catalog's conflict resolver
    pub async fn register_custom_commands(
        &self,
        rules: &[RoutingRuleConfig],
        conflict_resolver: &ConflictResolver,
    ) {
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

            // Include rule_index in the id (custom:{index}:{name}) like
            // ToolSource::format_tool_id everywhere else — two rules resolving
            // to the same command name would otherwise collide on `custom:{name}`
            // and the second silently overwrite the first.
            let id = ToolSource::Custom { rule_index: index }.format_tool_id(&command_name);

            // Use system_prompt as description if available, otherwise generic
            let description = rule.system_prompt.as_ref().map_or_else(
                || format!("Custom command /{command_name}"),
                |s| truncate_description(s, 100),
            );

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

            conflict_resolver
                .register_with_conflict_resolution(tool)
                .await;
            count += 1;
        }

        debug!("Registered {} custom commands", count);
    }
}
