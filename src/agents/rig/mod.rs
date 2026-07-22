//! Agent Configuration Module for AlephTool System
//!
//! This module provides configuration and tool server functionality for AI agents
//! using the self-implemented AlephTool system with tool calling support.
//!
//! # Architecture
//!
//! ```text
//! User Input
//!      ↓
//! ┌─────────────────────────────────────────────────────┐
//! │              Agent Loop (self-implemented)          │
//! │                                                      │
//! │  ┌─────────────────────────────────────────────────┐│
//! │  │ AlephTool ToolServer for hot-reload support    ││
//! │  │ - SearchTool, WebFetchTool                       ││
//! │  │ - McpToolWrapper (hot-reload MCP tools)         ││
//! │  └─────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────┘
//!      ↓
//! Response { content, tool_calls, ... }
//! ```

mod message_history;
mod types;

pub use message_history::{ChatMessage, ConversationHistory, MessageRole};
pub use types::{AgentResult, ToolCallInfo, ToolCallResult};
