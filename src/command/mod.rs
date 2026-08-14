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

/// Deferred handle to the process's one [`CommandParser`].
///
/// The parser cannot exist when the gateway is assembled — it is built from
/// the `ToolCatalog`, which is only populated once plugins, MCP servers and
/// skills have registered. Every consumer therefore holds this cell rather
/// than the parser, and reads it per request.
///
/// It is a *shared* cell, not one per consumer: `command.execute`, `chat.send`,
/// `agent.run` and the execution engine's own fallback all resolve slash input
/// through the same parser, which is what stops the four surfaces from growing
/// four slightly different ideas of what `/foo` means.
pub type CommandParserCell = crate::sync_primitives::Arc<
    tokio::sync::RwLock<Option<crate::sync_primitives::Arc<CommandParser>>>,
>;
