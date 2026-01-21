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
//! - [`CodeExecTool`] - Code execution (Python, JavaScript, Shell)
//! - [`PdfGenerateTool`] - PDF generation from text/Markdown
//! - [`ImageGenerateTool`] - Image generation from text prompts
//! - [`SpeechGenerateTool`] - Text-to-speech generation
//!
//! # Tool Wrappers (for hot-reload)
//!
//! - [`McpToolWrapper`] - Wraps MCP server tools as rig-compatible tools

pub mod code_exec;
pub mod error;
pub mod file_ops;
pub mod generation;
pub mod mcp_wrapper;
pub mod pdf_generate;
pub mod search;
pub mod web_fetch;
pub mod youtube;

pub use code_exec::{CodeExecArgs, CodeExecTool};
pub use error::ToolError;
pub use file_ops::{FileOpsArgs, FileOpsTool};
pub use generation::{ImageGenerateArgs, ImageGenerateTool, SpeechGenerateArgs, SpeechGenerateTool};
pub use mcp_wrapper::McpToolWrapper;
pub use pdf_generate::{PdfGenerateArgs, PdfGenerateTool};
pub use search::{SearchArgs, SearchTool};
pub use web_fetch::{WebFetchArgs, WebFetchTool};
pub use youtube::{YouTubeArgs, YouTubeTool};
