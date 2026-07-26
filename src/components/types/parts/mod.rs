//! Session part types - fine-grained execution records

use serde::{Deserialize, Serialize};

// Submodules
mod ai_response;
mod markers;
mod patch;
mod reasoning;
mod snapshot;
mod steps;
mod streaming;
mod sub_agent;
mod summary;
mod tool_call;
mod user_input;

// Re-export all part types
pub use ai_response::AiResponsePart;
pub use markers::CompactionMarker;
pub use patch::{FileChange, FileChangeType, PatchPart};
pub use reasoning::ReasoningPart;
pub use snapshot::{FileSnapshot, SnapshotPart};
pub use steps::{StepFinishPart, StepFinishReason, StepStartPart, StepTokenUsage};
pub use streaming::StreamingTextPart;
pub use sub_agent::SubAgentPart;
pub use summary::SummaryPart;
pub use tool_call::{ToolCallPart, ToolCallStatus};
pub use user_input::UserInputPart;

// Import SystemReminderPart from parent
use super::status::SystemReminderPart;

/// Session part - fine-grained execution records
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionPart {
    UserInput(UserInputPart),
    AiResponse(AiResponsePart),
    ToolCall(ToolCallPart),
    Reasoning(ReasoningPart),
    SubAgentCall(SubAgentPart),
    Summary(SummaryPart),
    /// Marker for compaction boundary - used by `filter_compacted()` to find
    /// the point where old context was summarized
    CompactionMarker(CompactionMarker),
    /// System reminder for context injection (aligns with `OpenCode`'s <system-reminder>)
    SystemReminder(SystemReminderPart),
    /// Step boundary - start
    StepStart(StepStartPart),
    /// Step boundary - finish
    StepFinish(StepFinishPart),
    /// Filesystem snapshot
    Snapshot(SnapshotPart),
    /// File change record
    Patch(PatchPart),
    /// Incremental streaming text
    StreamingText(StreamingTextPart),
}

impl SessionPart {
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::UserInput(_) => "user_input",
            Self::AiResponse(_) => "ai_response",
            Self::ToolCall(_) => "tool_call",
            Self::Reasoning(_) => "reasoning",
            Self::SubAgentCall(_) => "sub_agent_call",
            Self::Summary(_) => "summary",
            Self::CompactionMarker(_) => "compaction_marker",
            Self::SystemReminder(_) => "system_reminder",
            Self::StepStart(_) => "step_start",
            Self::StepFinish(_) => "step_finish",
            Self::Snapshot(_) => "snapshot",
            Self::Patch(_) => "patch",
            Self::StreamingText(_) => "streaming_text",
        }
    }
}
