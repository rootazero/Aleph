//! Bounded presentation data for the negotiated V1 chat transcript.

use serde::{Deserialize, Serialize};

use crate::file_change::Presentation;

pub const CHAT_TOOL_TOPIC: &str = "stream.chat_tool";
pub const CHAT_TRANSCRIPT_DETAILS_METHOD: &str = "chat.transcript_details";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTranscriptDetailsRequest {
    pub session_key: String,
    pub run_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTranscriptDetails {
    pub runs: Vec<ChatRunDetail>,
    pub limited: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRunDetail {
    pub run_id: String,
    pub tools: Vec<ChatToolDelta>,
    pub limited: bool,
}

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
