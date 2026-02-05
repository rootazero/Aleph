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
//!                      │
//!     ┌────────────────┼────────────────┐
//!     ▼                ▼                ▼
//! ExecutionCoordinator ResultCollector  (sync wait)
//! ```
//!
//! # Synchronous Execution
//!
//! The module now supports synchronous wait for sub-agent results:
//!
//! ```rust,ignore
//! use alephcore::agents::sub_agents::{
//!     SubAgentDispatcher, SubAgentRequest, ExecutionCoordinator, CoordinatorConfig
//! };
//!
//! // Create coordinator for synchronous wait
//! let coordinator = ExecutionCoordinator::new(CoordinatorConfig::default());
//!
//! // Dispatch and wait for result
//! let result = dispatcher.dispatch_sync(request, Duration::from_secs(60)).await?;
//!
//! // Or dispatch multiple in parallel and wait for all
//! let results = dispatcher.dispatch_parallel_sync(requests, Duration::from_secs(120)).await;
//! ```
//!
//! # Result Collection
//!
//! Tool calls and artifacts are automatically collected during execution:
//!
//! ```rust,ignore
//! use alephcore::agents::sub_agents::ResultCollector;
//!
//! let collector = ResultCollector::new();
//! collector.init_request("req-1").await;
//!
//! // Tool calls are recorded automatically via event handlers
//! // Get OpenCode-compatible summary
//! let summary = collector.get_summary("req-1").await;
//! ```
//!
//! # Legacy Usage
//!
//! ```rust,ignore
//! use alephcore::agents::sub_agents::{SubAgent, McpSubAgent, SubAgentRequest};
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
mod coordinator;
mod result_collector;
mod run;
mod registry;
mod persistence;

pub use traits::{
    SubAgent, SubAgentCapability, SubAgentRequest, SubAgentResult,
    ExecutionContextInfo, StepContextInfo, ToolCallRecord, Artifact,
};
pub use mcp_agent::McpSubAgent;
pub use skill_agent::SkillSubAgent;
pub use delegate_tool::{DelegateTool, DelegateArgs, DelegateResult, ArtifactInfo, ToolCallInfo};
pub use dispatcher::{SubAgentDispatcher, SubAgentType, DispatcherInfo, AgentInfo};
pub use result_merger::{ResultMerger, MergedResult};

// New synchronous execution components
pub use coordinator::{
    ExecutionCoordinator, CoordinatorConfig, ExecutionError, ExecutionHandle,
    ExecutionSlot, CoordinatorStats, ToolCallSummary, ToolCallState,
    ToolCallProgress, ToolCallStatus,
};
pub use result_collector::{
    ResultCollector, CollectedToolCall, CollectedToolStatus, CollectorStats,
    truncate_for_preview,
};

// Multi-Agent 2.0 run tracking
pub use run::{SubAgentRun, RunStatus, RunOutcome, Lane, CleanupPolicy};
pub use registry::{LifecycleEvent, RegistryStats, SubAgentRegistry};
pub use persistence::SubAgentRunFact;
