//! Session part types - fine-grained execution records

use serde::{Deserialize, Serialize};

// Submodules
mod user_input;
mod ai_response;
mod tool_call;
mod reasoning;
mod plan;
mod sub_agent;
mod summary;
mod markers;
mod steps;
mod snapshot;
mod patch;
mod streaming;

// Re-export all part types
pub use user_input::UserInputPart;
pub use ai_response::AiResponsePart;
pub use tool_call::{ToolCallPart, ToolCallStatus};
pub use reasoning::ReasoningPart;
pub use plan::{PlanPart, PlanStep, StepStatus};
pub use sub_agent::SubAgentPart;
pub use summary::SummaryPart;
pub use markers::CompactionMarker;
pub use steps::{StepStartPart, StepFinishPart, StepFinishReason, StepTokenUsage};
pub use snapshot::{SnapshotPart, FileSnapshot};
pub use patch::{PatchPart, FileChange, FileChangeType};
pub use streaming::StreamingTextPart;

// Import SystemReminderPart from parent
use super::status::SystemReminderPart;

/// Session part - fine-grained execution records
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionPart {
    UserInput(UserInputPart),
    AiResponse(AiResponsePart),
    ToolCall(ToolCallPart),
    Reasoning(ReasoningPart),
    PlanCreated(PlanPart),
    SubAgentCall(SubAgentPart),
    Summary(SummaryPart),
    /// Marker for compaction boundary - used by filter_compacted() to find
    /// the point where old context was summarized
    CompactionMarker(CompactionMarker),
    /// System reminder for context injection (aligns with OpenCode's <system-reminder>)
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
    pub fn type_name(&self) -> &'static str {
        match self {
            SessionPart::UserInput(_) => "user_input",
            SessionPart::AiResponse(_) => "ai_response",
            SessionPart::ToolCall(_) => "tool_call",
            SessionPart::Reasoning(_) => "reasoning",
            SessionPart::PlanCreated(_) => "plan_created",
            SessionPart::SubAgentCall(_) => "sub_agent_call",
            SessionPart::Summary(_) => "summary",
            SessionPart::CompactionMarker(_) => "compaction_marker",
            SessionPart::SystemReminder(_) => "system_reminder",
            SessionPart::StepStart(_) => "step_start",
            SessionPart::StepFinish(_) => "step_finish",
            SessionPart::Snapshot(_) => "snapshot",
            SessionPart::Patch(_) => "patch",
            SessionPart::StreamingText(_) => "streaming_text",
        }
    }
}
