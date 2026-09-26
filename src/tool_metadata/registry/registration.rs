//! Tool Registration Methods
//!
//! Methods for registering tools from various sources.

use tracing::{debug, info, warn};

use crate::config::RoutingRuleConfig;
use crate::skill::SkillInfo;

use super::super::types::{ToolSource, UnifiedTool};
use super::conflict::ConflictResolver;
use super::helpers::{extract_command_name, truncate_description};

/// What a slash row's `allowed-tools:` does to a `/name` turn. One
/// frontmatter key carries two meanings (Claude Code's reading), so the
/// registering caller names which one its rows have; registration never
/// infers it from the row's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowedToolsMeaning {
    /// A plugin COMMAND's list narrows the run's tool surface: an unknown
    /// name refuses the row (a restriction naming nothing is a silent
    /// deny-all).
    Restricts,
    /// A SKILL's list pre-grants: Claude Code names are mapped, what cannot be
    /// granted is dropped with a warn, and the skill always registers.
    PreGrants,
}

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

    /// Register skills and plugin commands from `SkillInfo` rows (Flat
    /// Namespace Mode)
    ///
    /// In flat namespace mode, rows are registered as root-level commands
    /// with automatic conflict resolution. Users can invoke them directly
    /// via `/{skill_id}` without the `/skill` prefix.
    ///
    /// # Arguments
    ///
    /// * `skills` - the rows, all of one kind
    /// * `conflict_resolver` - Conflict resolver for handling name conflicts
    /// * `admit_unregistered` - the caller's third source of known names — see
    ///   [`Self::is_known_tool_name`]
    /// * `meaning` - what the rows' `allowed-tools:` does to a `/name` turn,
    ///   which decides how it is validated here ([`AllowedToolsMeaning`])
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
    /// The ids of rows that were **not** registered because their declared
    /// `allowed-tools:` named tools that do not exist — plugin commands only
    /// ([`AllowedToolsMeaning::Restricts`]); a skill is never refused.
    /// Returned rather than only logged so the caller can say it out loud: a
    /// command silently missing from the slash catalog reads exactly like a
    /// command that was never installed.
    pub async fn register_skills(
        &self,
        skills: &[SkillInfo],
        conflict_resolver: &ConflictResolver,
        admit_unregistered: &(dyn Fn(&str) -> bool + Send + Sync),
        meaning: AllowedToolsMeaning,
    ) -> Vec<String> {
        let mut rejected: Vec<String> = Vec::new();
        for skill in skills {
            let id = format!("skill:{}", skill.id);

            // Resolve the declared tool scope BEFORE building the tool. For a
            // plugin command an unresolvable declaration means this row does
            // not get a slash command at all. That matches what the skill
            // system already does with a file it cannot parse
            // (`skill::scan_directory` skips it with a warn); the alternative —
            // register it with the declaration dropped — hands the model a
            // plugin `/command` that runs with the full toolbelt the author
            // explicitly tried to shrink. A SKILL's declaration pre-grants
            // rather than restricts
            // (`gateway::execution_engine::slash_skill_pregrant`), so what
            // cannot be granted is dropped and the skill still registers.
            let routing_capabilities = match meaning {
                AllowedToolsMeaning::PreGrants => {
                    Self::resolve_skill_pregrant_scope(
                        &skill.id,
                        skill.allowed_tools.as_deref(),
                        conflict_resolver,
                        admit_unregistered,
                    )
                    .await
                }
                AllowedToolsMeaning::Restricts => match Self::resolve_command_tool_scope(
                    skill.allowed_tools.as_deref(),
                    conflict_resolver,
                    admit_unregistered,
                )
                .await
                {
                    Ok(caps) => caps,
                    Err(unknown) => {
                        warn!(
                            command = %skill.id,
                            unknown_tools = ?unknown,
                            "plugin command declares `allowed-tools:` naming tools that do not \
                             exist; its slash command is NOT registered"
                        );
                        rejected.push(skill.id.clone());
                        continue;
                    }
                },
            };

            let tool = UnifiedTool::new(
                &id,
                &skill.id, // Use skill ID as command name
                &skill.description,
                ToolSource::Skill {
                    id: skill.id.clone(),
                    plugin_id: skill.plugin_id.clone(),
                },
            )
            .with_display_name(&skill.name)
            .with_icon("lightbulb.fill") // Default Skill icon
            .with_usage(match skill.argument_hint.as_deref() {
                Some(hint) => format!("/{} {hint}", skill.id),
                None => format!("/{} [input]", skill.id),
            })
            // Generate routing regex for flat namespace
            .with_routing_regex(format!(r"^/{}\s*", regex::escape(&skill.id)))
            .with_routing_intent_type("skills")
            // No `routing_system_prompt` here: it used to carry the skill's
            // description onto `CommandContext::Skill.instructions` and out
            // through the slash-command envelope, where nothing ever read it
            // back. The description already reaches the model through the
            // `<available_skills>` block (`thinker::layers::skill_instructions`)
            // and the body only through the `skill_read` tool, so the copy was
            // a second, reader-less expression of the same string.
            // The validated `allowed-tools:` scope. `None` here is "the skill
            // declared nothing" and must stay distinguishable from
            // `Some(vec![])`, "the skill declared nothing is allowed" — see
            // `UnifiedTool::routing_capabilities`.
            .with_routing_capabilities(routing_capabilities)
            .with_routing_strip_prefix(true);
            // The hint's second face: `usage` above reaches `/help`, this one
            // reaches `commands.list` and so the completion menus.
            let tool = match skill.argument_hint.as_deref() {
                Some(hint) => tool.with_param_hint(hint),
                None => tool,
            };

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

    /// A plugin COMMAND's declared `allowed-tools:`, validated against the
    /// tool names that actually exist.
    ///
    /// `Ok(None)` — declared nothing. `Ok(Some(names))` — every declared name
    /// resolves ([`Self::is_known_tool_name`]). `Err(unknown)` — at least one
    /// name names nothing, and the caller must refuse the row. The validated
    /// list restricts the run: an empty `Some` is a legitimate, explicit
    /// deny-all; `None` keeps the full surface.
    ///
    /// Known boundary: plugin tools and MCP tools register into the catalog
    /// *after* the boot pass does (MCP joins per request at run time), so a
    /// command naming a server its plugin does not declare is refused even
    /// though the run loop could have honoured it. That is a loud false
    /// negative — the command is named in a warn — chosen over the silent
    /// false positive [`Self::is_known_tool_name`] describes. This function
    /// translates nothing: matching upstream's `Read`/`Bash`/`Grep` literally
    /// would retain zero tools while reporting success, so an upstream name is
    /// refused here by name. A plugin command's declaration arrives already
    /// translated (`manifest/parsers.rs` maps it through
    /// `extension::hooks::normalize_cc_tool_entry` at parse time).
    async fn resolve_command_tool_scope(
        declared: Option<&[String]>,
        conflict_resolver: &ConflictResolver,
        admit_unregistered: &(dyn Fn(&str) -> bool + Send + Sync),
    ) -> Result<Option<Vec<String>>, Vec<String>> {
        let Some(names) = declared else {
            return Ok(None);
        };

        let mut unknown: Vec<String> = Vec::new();
        for name in names {
            if !Self::is_known_tool_name(name, conflict_resolver, admit_unregistered).await {
                unknown.push(name.clone());
            }
        }

        if unknown.is_empty() {
            Ok(Some(names.to_vec()))
        } else {
            Err(unknown)
        }
    }

    /// A SKILL's declared `allowed-tools:`, as the Aleph tools a typed
    /// `/<skill>` may pre-grant
    /// (`gateway::execution_engine::slash_skill_pregrant`: only these names,
    /// never what the file added since — until the next boot, when skill rows
    /// are registered again from the file as it then stands).
    ///
    /// Claude Code skills write Claude Code names (`Read, Grep, Bash(git *)`),
    /// so each entry is mapped by
    /// [`crate::skill::frontmatter::pregrant_tool_name`] — the same mapping
    /// the turn applies to the loaded file before intersecting it with this
    /// list. An entry that maps to nothing (a CC tool with no counterpart, a
    /// scoped `Bash(...)`, a glob, a bare `mcp__<server>`) or to a name
    /// [`Self::is_known_tool_name`] does not know is dropped, with one warn
    /// naming the skill and the entry. The skill still registers: a skill's
    /// list only grants, so a dropped entry costs the author that grant and
    /// nothing else. `None` — declared nothing; `Some(vec![])` — declared, and
    /// nothing survived: nothing is pre-granted. Only a plugin command's list
    /// narrows the surface — with one exception on the turn side: a skill row
    /// whose manifest is gone by the time `/<skill>` is typed (removed or
    /// renamed since boot) is not recognised as a skill, and its list
    /// restricts that turn like a command's (`slash_skill_pregrant::split`),
    /// so an all-dropped list there leaves no tools.
    async fn resolve_skill_pregrant_scope(
        skill_id: &str,
        declared: Option<&[String]>,
        conflict_resolver: &ConflictResolver,
        admit_unregistered: &(dyn Fn(&str) -> bool + Send + Sync),
    ) -> Option<Vec<String>> {
        let declared = declared?;
        let mut granted: Vec<String> = Vec::new();
        for entry in declared {
            let Some(tool) = crate::skill::frontmatter::pregrant_tool_name(entry) else {
                warn!(
                    skill = %skill_id,
                    entry = %entry,
                    "skill `allowed-tools:` entry can pre-grant no Aleph tool (a Claude Code \
                     tool with no Aleph counterpart, a scoped `Bash(...)`, a glob or a bare \
                     `mcp__<server>`); dropped — the skill still registers"
                );
                continue;
            };
            if !Self::is_known_tool_name(&tool, conflict_resolver, admit_unregistered).await {
                warn!(
                    skill = %skill_id,
                    entry = %entry,
                    tool = %tool,
                    "skill `allowed-tools:` entry names no tool Aleph knows when skills \
                     register (plugin and MCP tools register later); dropped — the skill \
                     still registers"
                );
                continue;
            }
            if !granted.contains(&tool) {
                granted.push(tool);
            }
        }
        Some(granted)
    }

    /// Whether `name` is a tool the run loop can offer.
    ///
    /// Three sources are unioned:
    /// * [`crate::executor::is_builtin_tool_name`] — the executor's own static
    ///   list (the same one the `ExecutionEngine` seeds its tool list from)
    ///   plus [`crate::executor::TOOLS_OUTSIDE_DEFINITIONS`], the real tools
    ///   no catalog row names (`subagent`, `tool_search`). It is the predicate
    ///   the Claude Code alias table's guard reads, so every alias target is a
    ///   name kept here. Consulting it means this answer does not depend on how
    ///   much of the catalog happens to be populated when rows register. A
    ///   guard whose known-set is "whatever registered first" starts rejecting
    ///   valid rows the day boot order changes, and a guard that rejects
    ///   everything looks exactly like a guard that works.
    /// * the catalog as it stands, **restricted to sources that are also LLM
    ///   tool names**. This is not fussiness: the catalog is a *slash-command*
    ///   index, and `Skill` / `Custom` entries live only there. Admitting them
    ///   would let a declaration pass validation and then match nothing in the
    ///   run loop, whose candidate list is built from the executor's tools —
    ///   for a command a silent deny-all, for a skill a grant of nothing
    ///   reported as a grant.
    /// * `admit_unregistered` — names the caller vouches for although no
    ///   catalog row exists *yet*. A plugin command passes the tool keys of
    ///   the MCP servers its own plugin declares
    ///   (`extension::slash_effect::register_slash_commands_effect`): those
    ///   tools reach the catalog only when the server starts, which nothing
    ///   orders before the command's registration, and the run loop narrows
    ///   by name and joins MCP per request, so the admitted name is honoured
    ///   once the server is up. Every other caller admits nothing here.
    ///
    /// Known boundary: plugin tools and MCP tools register into the catalog
    /// *after* skills do, so a skill naming one has that entry dropped (and
    /// warned) although the run loop could have honoured it.
    async fn is_known_tool_name(
        name: &str,
        conflict_resolver: &ConflictResolver,
        admit_unregistered: &(dyn Fn(&str) -> bool + Send + Sync),
    ) -> bool {
        if crate::executor::is_builtin_tool_name(name) {
            return true;
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
            return true;
        }
        admit_unregistered(name)
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
    /// Only rules with `^/` prefix patterns are registered as tools; a rule
    /// without the prefix is a retired keyword rule and reaches nothing here
    /// (see `RoutingRuleConfig`'s module doc).
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

            // Only slash commands become tools. The predicate lives on the
            // config type so that this skip, the load-time retirement warning
            // and the `routing_rules.*` refusal all describe the same set —
            // a second spelling here is how the warning would end up naming a
            // different set of rules than the one actually skipped.
            if !rule.is_registered_command() {
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
