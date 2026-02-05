//! Multi-Agent Resilience Module
//!
//! Provides database operations for the Multi-Agent Resilience architecture:
//! - Agent task tracking with recovery support
//! - Execution traces for Shadow Replay
//! - Tiered event persistence (Skeleton & Pulse)
//! - Subagent session management (Session-as-a-Service)
//! - Recovery: Shadow Replay, Graceful Shutdown, Recovery Manager
//! - Perception: Event classification, emission, and observation
//! - Collaboration: Session handles, swapping, and coordination
//! - Governance: Resource governor, quotas, and recursion limiting

pub mod collaboration;
mod events;
pub mod governance;
pub mod perception;
pub mod recovery;
mod sessions;
mod tasks;
mod traces;
mod types;

pub use types::{
    AgentEvent, AgentTask, Lane, RiskLevel, SessionStatus, SubagentSession, TaskStatus, TaskTrace,
    TraceRole,
};

pub use recovery::{
    DivergenceStatus, GracefulShutdown, RecoveryDecision, RecoveryManager, RecoverySummary,
    ReplayResult, ShadowReplayEngine, ShutdownSignal, TaskRiskAdapter,
};

pub use perception::{
    EmitterConfig, EventClassifier, EventEmitter, EventTier, EventType, GapFillResult,
    PulseBuffer, TaskObserver,
};

pub use collaboration::{
    CoordinatorConfig, SessionCounts, SessionCoordinator, SessionHandle, SwapConfig, SwapManager,
    SwapResult, SwapStats, SwappedContext, TaskResult,
};

pub use governance::{
    GovernorConfig, GovernorStats, QuotaCheckResult, QuotaConfig, QuotaManager, QuotaUsage,
    QuotaViolation, RecursionLimitExceeded, RecursiveSentry, RemainingCapacity, ResourceGovernor,
    ResourcePermit,
};
