//! Slash command parsing for Aleph.
//!
//! Resolves `/foo` inputs against the unified [`ToolCatalog`]
//! (in `tool_metadata`). Holds **no registry of its own** — every
//! source-specific field the parser produces (`Builtin` / `Native` /
//! `Plugin` collapsing into `CommandContext::Builtin`, `Mcp`, `Skill`,
//! `Custom`) is derived from a single `UnifiedTool` lookup, so the
//! six [`ToolSource`] variants stay in lockstep with the catalog.
//!
//! Discovery (the hierarchical tree surfaced via `commands.list`)
//! lives in `gateway::handlers::commands`.

mod parser;

pub use parser::{CommandContext, CommandParser, ParsedCommand};
