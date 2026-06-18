//! Core registry implementation for builtin tools.
//!
//! Split into cohesive siblings:
//! - [`struct_def`] — the `BuiltinToolRegistry` field declarations.
//! - [`inherent`] — inherent `impl` (constructors, deferred-injection setters,
//!   handle accessors, metadata lookups).
//! - [`tool_registry_impl`] — `impl ToolRegistry for BuiltinToolRegistry`
//!   (trait accessors + the indivisible `execute_tool` dispatch).
//! - [`free_fns`] — standalone helpers (`parse_caller_agent_id`,
//!   `resolve_plugin_handler_from_sources`).

mod free_fns;
mod inherent;
mod struct_def;
mod tool_registry_impl;

pub use struct_def::BuiltinToolRegistry;

// Re-exported so existing paths keep working:
// - `super::registry::resolve_plugin_handler_from_sources` (used by the
//   parent module's tests).
// - `registry::parse_caller_agent_id` (used by this module's tests).
pub(crate) use free_fns::resolve_plugin_handler_from_sources;
pub(super) use free_fns::parse_caller_agent_id;

#[cfg(test)]
mod tests;
