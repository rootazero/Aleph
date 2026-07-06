// Command Completion System
//
// This module provides a unified command registry for Aleph's command mode.
// It aggregates commands from multiple sources:
// - Builtin commands (from config.toml rules with ^/ prefix)
// - MCP tools (dynamic, from connected MCP servers)
// - User prompts (from config.toml rules)
// - Skills (from ~/.aleph/skills/)
//
// The command tree is exposed to UI clients as a hierarchical JSON tree over
// JSON-RPC (`commands.list`; see `gateway::handlers::commands`).

mod parser;

pub use parser::{CommandContext, CommandParser, ParsedCommand};
