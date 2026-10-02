//! Aleph as an MCP **server** (Streamable HTTP, `/mcp`) — spec §3.7.
//!
//! An interface face (R4): it translates `tools/list` / `tools/call` into the
//! same scoped tool dispatch a chat turn uses and nothing else. Wire payloads
//! come from `crate::mcp::protocol`; the JSON-RPC envelope is the gateway's
//! own `crate::gateway::protocol`. Nothing here is a second MCP implementation
//! (CLAUDE.md 禁用清单).

pub mod config;
