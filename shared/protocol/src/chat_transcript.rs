//! Bounded presentation data for the negotiated V1 chat transcript.

use serde::{Deserialize, Serialize};

use crate::file_change::Presentation;

pub const CHAT_TOOL_TOPIC: &str = "stream.chat_tool";

/// A presentation-only change addressed by the tool call ID within a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatToolDelta {
    pub tool_call_id: String,
    pub change: ChatToolChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatToolChange {
    Started {
        display_name: String,
        args_text: String,
    },
    Working,
    Finished {
        success: bool,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        presentation: Option<Presentation>,
        presentation_omitted: bool,
    },
}
