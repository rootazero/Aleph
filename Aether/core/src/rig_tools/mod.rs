//! Rig tool implementations
//!
//! All tools implement rig's Tool trait for AI-callable functions.
//!
//! # Built-in Tools
//!
//! - [`SearchTool`] - Web search via SearXNG
//! - [`WebFetchTool`] - Web page fetching
//! - [`YouTubeTool`] - YouTube video transcript extraction
//! - [`FileOpsTool`] - File system operations (list, read, write, move, copy, delete, mkdir, search)
//!
//! # Tool Wrappers (for hot-reload)
//!
//! - [`McpToolWrapper`] - Wraps MCP server tools as rig-compatible tools

pub mod error;
pub mod file_ops;
pub mod mcp_wrapper;
pub mod search;
pub mod web_fetch;
pub mod youtube;

pub use error::ToolError;
pub use file_ops::FileOpsTool;
pub use mcp_wrapper::McpToolWrapper;
pub use search::SearchTool;
pub use web_fetch::WebFetchTool;
pub use youtube::YouTubeTool;
