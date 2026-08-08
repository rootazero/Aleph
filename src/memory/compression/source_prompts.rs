//! Source-specialised guidance blocks for the compound-ingest LLM call.
//!
//! Each `RawMemorySource` variant routes to a block tuned to the semantic
//! of that capture point. These are SUFFIXES, not standalone prompts:
//! `notes::ingest::prompts::build_compound_system_prompt` appends the
//! selected one to `PROMPT_COMPOUND_PLAN`, which alone owns the output
//! contract. A snapshot must therefore never restate an output shape — two
//! contracts in one system prompt means the model obeys the wrong one and
//! the plan parser rejects the result. Legacy variants select no block and
//! run on the base prompt alone.

use crate::memory::store::raw_memory::{RawMemorySource, SessionEndReason};

pub const PROMPT_RESCUE: &str = include_str!("source_prompts/snapshots/rescue.txt");
pub const PROMPT_LESSON: &str = include_str!("source_prompts/snapshots/lesson.txt");
pub const PROMPT_DIGEST: &str = include_str!("source_prompts/snapshots/digest.txt");
pub const PROMPT_RETRO: &str = include_str!("source_prompts/snapshots/retro.txt");

/// Choose the source-specific guidance block for a given raw-memory source.
/// Legacy variants return `None` so the caller emits the base
/// `PROMPT_COMPOUND_PLAN` unsuffixed.
#[must_use]
pub const fn prompt_for(source: &RawMemorySource) -> Option<&'static str> {
    match source {
        RawMemorySource::PreCompress => Some(PROMPT_RESCUE),
        RawMemorySource::Delegation { .. } => Some(PROMPT_LESSON),
        RawMemorySource::SessionEnd {
            reason: SessionEndReason::Disconnect,
        } => Some(PROMPT_DIGEST),
        RawMemorySource::SessionEnd {
            reason: SessionEndReason::TaskDone,
        } => Some(PROMPT_RETRO),
        // Reflection rows already carry first-person lessons; the lesson-tuned
        // prompt keeps the ingestor classifying into `lesson`/`tool` rather
        // than generic notes. It does NOT produce `feedback/` notes — those are
        // written only by the `FeedbackDistill` dream stage from `Correction`
        // rows, and only they get the always-on feedback floor.
        RawMemorySource::Reflection => Some(PROMPT_LESSON),
        RawMemorySource::SessionCompressed
        | RawMemorySource::Transcript
        | RawMemorySource::ToolOutput
        | RawMemorySource::Attachment => None,
        // Correction signals are consumed by FeedbackDistill via path-prefix,
        // not by CompressionService. If one ever reaches this path defensively
        // fall back to the base prompt rather than synthesizing a bogus one.
        RawMemorySource::Correction { .. } => None,
        // Tool-invocation signals are pure telemetry, read only by the
        // `insights.tools` aggregator (`crate::memory::insights`) — NOT by any
        // dream stage. `CompressionService` partitions them out of the ingest
        // batch, so the planner never sees them either.
        RawMemorySource::ToolInvocation { .. } => None,
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
    fn reflection_selects_lesson() {
        assert_eq!(
            prompt_for(&RawMemorySource::Reflection),
            Some(PROMPT_LESSON)
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
        // No output-format assertion here on purpose: the JSON contract is
        // stated once, by `PROMPT_COMPOUND_PLAN`. A snapshot that also
        // described a shape is the bug `prompts_carry_distillation_quality_rules`
        // guards against.
        for prompt in [PROMPT_RESCUE, PROMPT_LESSON, PROMPT_DIGEST, PROMPT_RETRO] {
            assert!(prompt.len() > 100, "prompt snapshot too short");
        }
    }

    #[test]
    fn prompts_carry_distillation_quality_rules() {
        // All four source-specialised prompts share the distillation quality
        // bar: verbatim greppable handles, absolute dates, the
        // empty-output-preferred gate, and the anti-rot denylist (store the
        // remedy, not the failure narrative).
        for prompt in [PROMPT_RESCUE, PROMPT_LESSON, PROMPT_DIGEST, PROMPT_RETRO] {
            assert!(
                prompt.contains("RULES:"),
                "guidance block must keep its rules section"
            );
            assert!(
                prompt.contains("never paraphrase identifiers"),
                "must preserve greppable handles"
            );
            assert!(
                prompt.contains("absolute dates"),
                "must convert relative time to absolute dates"
            );
            assert!(
                prompt.contains("emit an empty plan"),
                "empty output must be an allowed outcome"
            );
            assert!(
                prompt.contains("remedy, not the failure narrative"),
                "anti-rot denylist must be present"
            );
            // These blocks are appended AFTER the base plan prompt, so whatever
            // they say last is what the model reads last. They once ended with
            // a `{"updates": [...]}` block belonging to the retired
            // FactExtractor — an instruction to emit a shape the plan parser
            // cannot read, placed where it overrides the real contract.
            // Guidance only: the rules must be the final word.
            assert!(
                !prompt.contains("updates"),
                "snapshot must not resurrect the retired `updates` output shape"
            );
            assert!(
                prompt.trim_end().ends_with("emit an empty plan."),
                "snapshot must end on its rules, with nothing appended after"
            );
        }
        // Per-source rules the shared loop above cannot see, so the deletion
        // cannot silently take a whole rules block with it.
        for prompt in [PROMPT_LESSON, PROMPT_RETRO] {
            assert!(
                prompt.contains("evidence → implication"),
                "lesson-shaped prompts must keep the evidence→implication phrasing"
            );
        }
        for prompt in [PROMPT_LESSON, PROMPT_DIGEST, PROMPT_RETRO] {
            assert!(
                prompt.contains("will a future agent plausibly act better"),
                "must keep the act-better gate"
            );
        }
        assert!(
            PROMPT_RESCUE.contains("Err on over-extraction"),
            "rescue trades precision for recall — that rule is its whole point"
        );
    }
}
