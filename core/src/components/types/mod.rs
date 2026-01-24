//! Shared types for component implementations.

// Submodules
mod status;
mod session;
mod context;
pub mod parts;
mod part_id;

#[cfg(test)]
mod tests;

// Re-export status types
pub use status::{SessionStatus, ReminderType, SystemReminderPart};

// Re-export session types
pub use session::{
    ExecutionSession, ToolCallRecord, Complexity, Decision, ComponentContext,
};

// Re-export context types
pub use context::{
    Knowledge, Entity, UserIntent, Goal, GoalStatus, DecisionRecord,
    ExecutionPhase, ContextVerbosity, ExecutionContext,
};

// Re-export all part types
pub use parts::{
    // Core part type enum
    SessionPart,
    // Individual part types
    UserInputPart,
    AiResponsePart,
    ToolCallPart,
    ToolCallStatus,
    ReasoningPart,
    PlanPart,
    SubAgentPart,
    SummaryPart,
    CompactionMarker,
    StepStartPart,
    StepFinishPart,
    StepFinishReason,
    StepTokenUsage,
    SnapshotPart,
    FileSnapshot,
    PatchPart,
    FileChange,
    FileChangeType,
    StreamingTextPart,
};

// Re-export part ID trait and update types
pub use part_id::{PartId, PartEventType, PartUpdateData};
