//! Source-specialised system prompts for the fact extractor.
//!
//! Each `RawMemorySource` variant routes to a prompt tuned to the
//! semantic of that capture point. Legacy variants fall back to the
//! generic prompt so existing behaviour is preserved.

use crate::memory::store::raw_memory::{RawMemorySource, SessionEndReason};

pub const PROMPT_RESCUE: &str = include_str!("source_prompts/snapshots/rescue.txt");
pub const PROMPT_LESSON: &str = include_str!("source_prompts/snapshots/lesson.txt");
pub const PROMPT_DIGEST: &str = include_str!("source_prompts/snapshots/digest.txt");
pub const PROMPT_RETRO: &str = include_str!("source_prompts/snapshots/retro.txt");

/// Choose the system prompt for a given raw-memory source.
/// Legacy variants return `None` so the caller falls back to the
/// existing generic prompt in `FactExtractor`.
pub fn prompt_for(source: &RawMemorySource) -> Option<&'static str> {
    match source {
        RawMemorySource::PreCompress => Some(PROMPT_RESCUE),
        RawMemorySource::Delegation { .. } => Some(PROMPT_LESSON),
        RawMemorySource::SessionEnd {
            reason: SessionEndReason::Disconnect,
        } => Some(PROMPT_DIGEST),
        RawMemorySource::SessionEnd {
            reason: SessionEndReason::TaskDone,
        } => Some(PROMPT_RETRO),
        RawMemorySource::SessionCompressed
        | RawMemorySource::Transcript
        | RawMemorySource::ToolOutput
        | RawMemorySource::Attachment => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_compress_selects_rescue() {
        assert_eq!(
            prompt_for(&RawMemorySource::PreCompress),
            Some(PROMPT_RESCUE)
        );
    }

    #[test]
    fn delegation_selects_lesson() {
        assert_eq!(
            prompt_for(&RawMemorySource::Delegation {
                child_agent_id: "c".into()
            }),
            Some(PROMPT_LESSON)
        );
    }

    #[test]
    fn session_end_disconnect_selects_digest() {
        assert_eq!(
            prompt_for(&RawMemorySource::SessionEnd {
                reason: SessionEndReason::Disconnect,
            }),
            Some(PROMPT_DIGEST)
        );
    }

    #[test]
    fn session_end_task_done_selects_retro() {
        assert_eq!(
            prompt_for(&RawMemorySource::SessionEnd {
                reason: SessionEndReason::TaskDone,
            }),
            Some(PROMPT_RETRO)
        );
    }

    #[test]
    fn legacy_variants_return_none() {
        assert!(prompt_for(&RawMemorySource::Transcript).is_none());
        assert!(prompt_for(&RawMemorySource::ToolOutput).is_none());
        assert!(prompt_for(&RawMemorySource::Attachment).is_none());
        assert!(prompt_for(&RawMemorySource::SessionCompressed).is_none());
    }

    #[test]
    fn prompts_have_nonempty_snapshots() {
        for prompt in [PROMPT_RESCUE, PROMPT_LESSON, PROMPT_DIGEST, PROMPT_RETRO] {
            assert!(prompt.len() > 100, "prompt snapshot too short");
            assert!(
                prompt.contains("JSON"),
                "prompt must instruct LLM to emit JSON"
            );
        }
    }
}
