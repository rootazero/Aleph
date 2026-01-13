//! Agent Module for Tool Calling Loop
//!
//! This module provides the `AgentLoop` which implements a custom agent loop
//! for executing tool calls with LLM function calling support.
//!
//! # Architecture
//!
//! ```text
//! User Input
//!      ↓
//! ┌─────────────────────────────────────────────────────┐
//! │                    AgentLoop                         │
//! │                                                      │
//! │  ┌─────────────────┐    ┌─────────────────────────┐ │
//! │  │ ConversationHist│    │ Message List:           │ │
//! │  │ - system        │ →  │ [system, user, assist,  │ │
//! │  │ - messages[]    │    │  tool_call, tool_result]│ │
//! │  └─────────────────┘    └───────────┬─────────────┘ │
//! │                                     ↓               │
//! │  ┌─────────────────────────────────────────────────┐│
//! │  │ AI Provider.chat_with_tools()                   ││
//! │  │ - Returns content + optional tool_calls         ││
//! │  └────────────────────────┬────────────────────────┘│
//! │                           ↓                         │
//! │  ┌─────────────────────────────────────────────────┐│
//! │  │ If tool_calls: Execute → Append result → Loop   ││
//! │  │ If no tool_calls: Return final response         ││
//! │  └─────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────┘
//!      ↓
//! AgentResult { response, history, tool_calls_made }
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::agent::{AgentLoop, AgentConfig};
//! use aethecore::tools::NativeToolRegistry;
//!
//! let agent = AgentLoop::new(provider, registry)
//!     .with_max_turns(10)
//!     .with_system_prompt("You are a helpful assistant.");
//!
//! let result = agent.run("Search for AI news and summarize").await?;
//!
//! println!("Response: {}", result.response);
//! println!("Tool calls made: {}", result.tool_calls_made);
//! ```

mod adapter;
pub mod config;
mod conversation;
mod executor;
pub mod manager;
mod types;

pub use adapter::{
    create_tool_adapter, AnthropicToolAdapter, AnthropicToolConfig, OpenAiToolAdapter,
    OpenAiToolConfig,
};
pub use config::RigAgentConfig;
pub use conversation::{ChatMessage, ConversationHistory, MessageRole};
pub use executor::{AgentLoop, ChatResponse, ToolCallingProvider};
pub use manager::RigAgentManager;
pub use types::{AgentConfig, AgentResult, ToolCallInfo, ToolCallResult};
