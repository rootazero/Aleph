//! Executor Module
//!
//! Provides the builtin tool registry + the [`ToolRegistry`] trait used by
//! the gateway execution engine to dispatch tool calls.

// `pub(crate)` so cross-crate test consumers (notably
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
/// The one list [`is_builtin_tool_name`] reads, so the Claude Code alias
/// table's guard and slash-row registration answer "is this a real tool" the
/// same way — `Task` maps to `subagent`, and a registry that did not know it
/// dropped it from every skill.
pub const TOOLS_OUTSIDE_DEFINITIONS: &[&str] = &[
    crate::agents::subagent_tool::SUBAGENT_TOOL_NAME,
    crate::tools::tool_search::ToolSearchTool::NAME,
];

/// `name` is a builtin tool: a [`BUILTIN_TOOL_DEFINITIONS`] row or one of
/// [`TOOLS_OUTSIDE_DEFINITIONS`].
#[must_use]
pub fn is_builtin_tool_name(name: &str) -> bool {
    BUILTIN_TOOL_DEFINITIONS.iter().any(|def| def.name == name)
        || TOOLS_OUTSIDE_DEFINITIONS.contains(&name)
}
/// The three non-catalog tool surfaces — text that ships to the model without
/// appearing in `BUILTIN_TOOL_DEFINITIONS`: registered by the core registry
/// constructor, pushed by the per-request tool service, or installed by the MCP
/// bridge. Test-only, and consumed by the guards that measure them: the byte
/// ratchets in `builtin_registry::definitions` and the duplicate-sentence scan
/// in `thinker::prompt_contract`.
///
/// They are re-exported rather than restated because a second list is the exact
/// failure these tables exist to prevent, one layer up.
#[cfg(test)]
pub(crate) use builtin_registry::{
    BRIDGE_TOOL_DESCRIPTIONS, INJECTED_TOOL_DESCRIPTIONS, REGISTRY_ONLY_DESCRIPTIONS,
};
pub use tool_registry::ToolRegistry;
