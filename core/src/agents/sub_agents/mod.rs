//! Sub-Agent Delegation Framework
//!
//! This module provides specialized sub-agents that can be delegated to
//! by the main agent for specific types of tasks.
//!
//! # Architecture
//!
//! ```text
//! Main Agent
//!     │
//!     ▼ (delegate_tool)
//! ┌─────────────────────────────────────────────────────┐
//! │              Sub-Agent Dispatcher                    │
//! │                                                      │
//! │  ┌────────────┐  ┌────────────┐  ┌───────────────┐  │
//! │  │ McpAgent   │  │ SkillAgent │  │ CustomAgent   │  │
//! │  └─────┬──────┘  └─────┬──────┘  └───────┬───────┘  │
//! │        │               │                 │          │
//! │        ▼               ▼                 ▼          │
//! │    MCP Tools       Skills DAG        Custom Logic   │
//! └─────────────────────────────────────────────────────┘
//!                      │
//!                      ▼
//!              SubAgentResult
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::agents::sub_agents::{SubAgent, McpSubAgent, SubAgentRequest};
//!
//! // Create an MCP sub-agent
//! let mcp_agent = McpSubAgent::new(mcp_registry);
//!
//! // Execute a request
//! let request = SubAgentRequest::new("github", "List my open PRs");
//! let result = mcp_agent.execute(request).await?;
//! ```

mod traits;
mod mcp_agent;
mod skill_agent;
mod delegate_tool;
mod dispatcher;
mod result_merger;

pub use traits::{
    SubAgent, SubAgentCapability, SubAgentRequest, SubAgentResult,
    ExecutionContextInfo, StepContextInfo, ToolCallRecord, Artifact,
};
pub use mcp_agent::McpSubAgent;
pub use skill_agent::SkillSubAgent;
pub use delegate_tool::{DelegateTool, DelegateArgs, DelegateResult, ArtifactInfo, ToolCallInfo};
pub use dispatcher::{SubAgentDispatcher, SubAgentType};
pub use result_merger::{ResultMerger, MergedResult};
