//! Agent Module for rig-core AI Agent
//!
//! This module provides the `RigAgentManager` which implements AI agent functionality
//! using the rig-core library with tool calling support.
//!
//! # Architecture
//!
//! ```text
//! User Input
//!      ↓
//! ┌─────────────────────────────────────────────────────┐
//! │                  RigAgentManager                     │
//! │                                                      │
//! │  ┌─────────────────────────────────────────────────┐│
//! │  │ rig-core Agent with ToolServer                  ││
//! │  │ - SearchTool, WebFetchTool, YouTubeTool         ││
//! │  │ - McpToolWrapper (hot-reload MCP tools)         ││
//! │  └─────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────┘
//!      ↓
//! AgentResponse { content, tool_calls, ... }
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::agent::{RigAgentManager, RigAgentConfig};
//!
//! let config = RigAgentConfig::default();
//! let manager = RigAgentManager::new(config)?;
//!
//! let response = manager.process("Search for AI news").await?;
//! println!("Response: {}", response.content);
//! ```

pub mod config;
mod conversation;
pub mod manager;
mod types;

pub use config::RigAgentConfig;
pub use conversation::{ChatMessage, ConversationHistory, MessageRole};
pub use manager::{AgentResponse, RigAgentManager};
pub use types::{AgentConfig, AgentResult, ToolCallInfo, ToolCallResult};
