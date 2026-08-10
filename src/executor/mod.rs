//! Executor Module
//!
//! Provides the builtin tool registry + the [`ToolRegistry`] trait used by
//! the gateway execution engine to dispatch tool calls.

mod builtin_registry;
mod tool_registry;

/// The registry-only tool descriptions — text that ships on every request
/// without appearing in `BUILTIN_TOOL_DEFINITIONS`. Test-only, and consumed by
/// the two guards that measure that surface: the byte ratchet in
/// `builtin_registry::definitions` and the duplicate-sentence scan in
/// `thinker::prompt_contract`.
#[cfg(test)]
pub(crate) use builtin_registry::REGISTRY_ONLY_DESCRIPTIONS;
pub use builtin_registry::{
    create_tool_boxed, get_builtin_tool_names, BuiltinToolConfig, BuiltinToolRegistry,
    BUILTIN_TOOL_DEFINITIONS, TOOL_CATEGORIES,
};
pub use tool_registry::ToolRegistry;
