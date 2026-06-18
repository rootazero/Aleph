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

// Re-exported so existing paths keep working. Both consumers are test-only
// (the parent module's tests reach `super::registry::resolve_plugin_handler_from_sources`,
// and this module's tests use `parse_caller_agent_id`), so gate to test builds.
#[cfg(test)]
pub(crate) use free_fns::resolve_plugin_handler_from_sources;
#[cfg(test)]
pub(super) use free_fns::parse_caller_agent_id;

#[cfg(test)]
mod tests;
