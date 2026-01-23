//! Agent Module for rig-core AI Agent
//!
//! This module provides configuration and tool server functionality for AI agents
//! using the rig-core library with tool calling support.
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
//! │  │ rig-core ToolServer for hot-reload support      ││
//! │  │ - SearchTool, WebFetchTool, YouTubeTool         ││
//! │  │ - McpToolWrapper (hot-reload MCP tools)         ││
//! │  └─────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────┘
//!      ↓
//! Response { content, tool_calls, ... }
//! ```

pub mod config;
mod message_history;
pub mod tools;
mod types;

pub use config::RigAgentConfig;
pub use message_history::{ChatMessage, ConversationHistory, MessageRole};
pub use tools::{create_builtin_tool_server, create_builtin_tools_list, BuiltinToolConfig};
pub use types::{AgentConfig, AgentResult, ToolCallInfo, ToolCallResult};
