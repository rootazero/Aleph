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
pub use tool_registry::ToolRegistry;
