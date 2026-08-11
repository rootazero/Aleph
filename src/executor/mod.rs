//! Executor Module
//!
//! Provides the builtin tool registry + the [`ToolRegistry`] trait used by
//! the gateway execution engine to dispatch tool calls.

mod builtin_registry;
mod tool_registry;

pub use builtin_registry::{
    create_tool_boxed, get_builtin_tool_names, BuiltinToolConfig, BuiltinToolRegistry,
    BUILTIN_TOOL_DEFINITIONS, TOOL_CATEGORIES,
};
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
