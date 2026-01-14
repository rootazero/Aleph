//! Rig tool implementations
//!
//! All tools implement rig's Tool trait for AI-callable functions.
//!
//! # Built-in Tools
//!
//! - [`SearchTool`] - Web search via SearXNG
//! - [`WebFetchTool`] - Web page fetching
//! - [`YouTubeTool`] - YouTube video transcript extraction
//!
//! # Tool Wrappers (for hot-reload)
//!
//! - [`McpToolWrapper`] - Wraps MCP server tools as rig-compatible tools

pub mod error;
pub mod mcp_wrapper;
pub mod search;
pub mod web_fetch;
pub mod youtube;

pub use error::ToolError;
pub use mcp_wrapper::McpToolWrapper;
pub use search::SearchTool;
pub use web_fetch::WebFetchTool;
pub use youtube::YouTubeTool;
