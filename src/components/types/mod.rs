//! Shared types for component implementations.

// Submodules
mod part_id;
pub mod parts;
mod session;
mod status;

#[cfg(test)]
mod tests;

// Re-export status types
pub use status::{ReminderType, SessionStatus, SystemReminderPart};

// Re-export session types
pub use session::{ExecutionSession, ToolCallRecord};

// Re-export all part types
pub use parts::{
    AiResponsePart,
    CompactionMarker,
    FileChange,
    FileChangeType,
    FileSnapshot,
    PatchPart,
    ReasoningPart,
    // Core part type enum
    SessionPart,
    SnapshotPart,
    StepFinishPart,
    StepFinishReason,
    StepStartPart,
    StepTokenUsage,
    StreamingTextPart,
    SubAgentPart,
    SummaryPart,
    ToolCallPart,
    ToolCallStatus,
    // Individual part types
    UserInputPart,
};

// Re-export part ID trait and update types
pub use part_id::{PartEventType, PartId, PartUpdateData};
