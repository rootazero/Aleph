//! Rig tool implementations
//!
//! All tools implement rig's Tool trait for AI-callable functions.

pub mod error;
pub mod search;
pub mod web_fetch;

pub use error::ToolError;
pub use search::SearchTool;
pub use web_fetch::WebFetchTool;
