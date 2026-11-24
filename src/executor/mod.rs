//! Executor Module
//!
//! Provides the builtin tool registry + the [`ToolRegistry`] trait used by
//! the gateway execution engine to dispatch tool calls.

// `pub(crate)` so this crate's test consumers (notably
// `security::dangerous_tools::tests::every_entry_names_a_real_tool`, which
// pins every denylist entry against a real tool) can reach the test-only
// `dispatchable` census through `crate::executor::builtin_registry
// ::dispatchable::advertised_tools` without rebuilding the registry. The
// runtime surface still flows through the `pub use` below; nothing
// here is a runtime API.
pub(crate) mod builtin_registry;
mod tool_registry;

pub use builtin_registry::{
    create_tool_boxed, BuiltinToolConfig, BuiltinToolRegistry, BUILTIN_TOOL_DEFINITIONS,
    TOOL_CATEGORIES,
};

/// Real tools the model is offered by the run loop although they are not in
/// [`BUILTIN_TOOL_DEFINITIONS`] and no slash-catalog row names them when
/// skill and command rows register: `subagent` (attached to every run's tool
/// service) and `tool_search` (the deferred tier's meta-tool). Each is spelled
/// by its own tool's name constant.
///
/// One of the lists [`is_builtin_tool_name`] reads, so the Claude Code alias
/// table's guard and slash-row registration answer "is this a real tool" the
/// same way — `Task` maps to `subagent`, and a registry that did not know it
/// dropped it from every skill. The other is [`REGISTRY_ONLY_TOOL_NAMES`].
pub const TOOLS_OUTSIDE_DEFINITIONS: &[&str] = &[
    crate::agents::subagent_tool::SUBAGENT_TOOL_NAME,
    crate::tools::tool_search::ToolSearchTool::NAME,
];

/// The tool names the registry registers that [`BUILTIN_TOOL_DEFINITIONS`]
/// does not list.
///
/// The run loop offers the model the catalog and then completes the list from
/// the registry's own map ("anything the registry can execute, the model can
/// see"), so a name here IS a tool the model is offered even though no
/// catalog row names it. `is_builtin_tool_name` used to answer "no" for every
/// one of them, so a skill row could claim a name the run loop already owns —
/// the predicate and the offered list disagreed about the same tool.
///
/// Kept honest from both sides by
/// `every_registry_only_tool_is_named_by_the_predicate_list` in
/// `builtin_registry::dispatchable`: that census scans every registration
/// shape in source, so a registration this list does not name fails by name,
/// and an entry here that no registration backs fails the other way.
pub const REGISTRY_ONLY_TOOL_NAMES: &[&str] = &[
    "acp_session_control",
    "audio_generate",
    "automation",
    "capability_projection_diagnostics",
    "channel_directory",
    "channel_message",
    "channel_outbox",
    "goal",
    "local_voice",
    "loop",
    "loop_graph",
    "media",
    "permission",
    "pim",
    "scratchpad",
    "speech_generate",
    "strategy",
    "system",
    "task_exit_journal",
    "team_task_control",
    "video_generate",
    "voice_mode_set",
];

/// `name` is a builtin tool: a [`BUILTIN_TOOL_DEFINITIONS`] row, a name the
/// registry registers outside the catalog ([`REGISTRY_ONLY_TOOL_NAMES`]), or
/// one of [`TOOLS_OUTSIDE_DEFINITIONS`].
///
/// The three lists together are the set of tools the run loop can offer the
/// model, so the answer no longer depends on which registration shape a name
/// happens to come from.
#[must_use]
pub fn is_builtin_tool_name(name: &str) -> bool {
    BUILTIN_TOOL_DEFINITIONS.iter().any(|def| def.name == name)
        || REGISTRY_ONLY_TOOL_NAMES.contains(&name)
        || TOOLS_OUTSIDE_DEFINITIONS.contains(&name)
}

/// The description the model is sent for a builtin tool name.
///
/// The catalog is not the only place a tool's description lives: the run
/// loop offers the model every tool the registry can execute, and the
/// registry-only ones carry theirs in [`REGISTRY_ONLY_DESCRIPTIONS`]. A
/// lookup that read the catalog alone answered "" for every registered
/// tool the catalog does not list, so the Panel showed those rows with a
/// blank description.
///
/// One lookup, one table: the catalog first, then the registry-only table.
/// Nothing here keeps a list of its own.
#[must_use]
pub(crate) fn builtin_tool_description(name: &str) -> &'static str {
    if let Some(def) = BUILTIN_TOOL_DEFINITIONS.iter().find(|def| def.name == name) {
        return def.description;
    }
    REGISTRY_ONLY_DESCRIPTIONS
        .iter()
        .find(|(tool_name, _)| *tool_name == name)
        .map_or("", |(_, description)| *description)
}
/// The descriptions of the tools the registry registers outside the catalog.
///
/// This one is NOT test-gated: it is the runtime source of truth for those
/// descriptions (`builtin_tool_description` reads it), and a description the
/// model is sent has to be reachable while the model is being served. The
/// other two non-catalog surfaces are still test-only.
pub(crate) use builtin_registry::REGISTRY_ONLY_DESCRIPTIONS;
/// The two remaining non-catalog tool surfaces — text that ships to the model
/// without appearing in `BUILTIN_TOOL_DEFINITIONS`: pushed by the per-request
/// tool service, or installed by the MCP bridge. Test-only, and consumed by
/// the guards that measure them: the byte ratchets in
/// `builtin_registry::definitions` and the duplicate-sentence scan in
/// `thinker::prompt_contract`.
///
/// They are re-exported rather than restated because a second list is the exact
/// failure these tables exist to prevent, one layer up.
#[cfg(test)]
pub(crate) use builtin_registry::{BRIDGE_TOOL_DESCRIPTIONS, INJECTED_TOOL_DESCRIPTIONS};
pub use tool_registry::ToolRegistry;

#[cfg(test)]
mod tests {
    use super::{builtin_tool_description, TOOL_CATEGORIES};

    /// A tool the registry registers outside the catalog still has to resolve
    /// a description: the Panel lists those tools by category, and a lookup
    /// that read the catalog alone gave every one of them a blank row.
    #[test]
    fn registry_only_tools_resolve_a_description() {
        for name in [
            "pim",
            "system",
            "acp_session_control",
            "task_exit_journal",
            "team_task_control",
            "capability_projection_diagnostics",
        ] {
            assert!(
                !builtin_tool_description(name).is_empty(),
                "{name} is registered outside the catalog and must still resolve a description"
            );
        }
    }

    /// The category list and the description lookup have to agree: a tool the
    /// Panel lists is a tool the model is offered, and every listed row
    /// carries the description that tool really has.
    #[test]
    fn every_category_tool_resolves_a_description() {
        for category in TOOL_CATEGORIES {
            for tool_name in category.tools {
                assert!(
                    !builtin_tool_description(tool_name).is_empty(),
                    "{} is listed in category {} but resolves no description",
                    tool_name,
                    category.id
                );
            }
        }
    }
}
