//! Multi-Agent Resilience Module
//!
//! Provides core types and subsystems for the Multi-Agent Resilience architecture:
//! - Agent task types with recovery support
//! - Execution trace types for Shadow Replay
//! - Tiered event types (Skeleton & Pulse)
//! - Subagent session types (Session-as-a-Service)
//! - Recovery: Shadow Replay, Graceful Shutdown, Recovery Manager
//! - Perception: Event classification, emission, and observation
//! - Collaboration: Session handles, swapping, and coordination
//! - Governance: Resource governor, quotas, and recursion limiting
//! - Database: SQLite state storage (StateDatabase) and CRUD operations

pub mod collaboration;
pub mod database;
pub mod governance;
pub mod perception;
pub mod recovery;
#[cfg(test)]
mod tests;
pub mod types;

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

pub use database::{MemoryStats, StateDatabase, DEFAULT_EMBEDDING_DIM};
