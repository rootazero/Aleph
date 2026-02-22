/// SQLite database wrapper for resilience state management
///
/// This module provides the SQLite-based storage for resilience operations
/// (agent events, tasks, traces, sessions). Memory operations have been
/// migrated to LanceDB via the `memory::store` module.
///
/// # Module Organization
///
/// - `core`: Database connection, initialization, schema, and utility functions
/// - `migration`: Schema migration utilities
/// - `resilience_*`: Resilience CRUD operations (events, tasks, traces, sessions)

mod core;
pub mod migration;
mod resilience_events;
mod resilience_sessions;
mod resilience_tasks;
mod resilience_traces;

// Re-export main types
pub use core::{MemoryStats, StateDatabase, DEFAULT_EMBEDDING_DIM};

// Re-export resilience types for backward compatibility
// (types now live in crate::resilience, CRUD impl blocks are in resilience_*.rs above)
pub use crate::resilience::{
    AgentEvent, AgentTask, CoordinatorConfig, DivergenceStatus, EmitterConfig, EventClassifier,
    EventEmitter, EventTier, EventType, GapFillResult, GovernorConfig, GovernorStats,
    GracefulShutdown, Lane, PulseBuffer, QuotaCheckResult, QuotaConfig, QuotaManager, QuotaUsage,
    QuotaViolation, RecoveryDecision, RecoveryManager, RecoverySummary, RecursionLimitExceeded,
    RecursiveSentry, RemainingCapacity, ReplayResult, ResourceGovernor, ResourcePermit, RiskLevel,
    SessionCounts, SessionCoordinator, SessionHandle, SessionStatus, ShadowReplayEngine,
    ShutdownSignal, SubagentSession, SwapConfig, SwapManager, SwapResult, SwapStats,
    SwappedContext, TaskObserver, TaskResult, TaskRiskAdapter, TaskStatus, TaskTrace, TraceRole,
};
