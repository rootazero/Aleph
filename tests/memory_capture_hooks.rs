//! Integration test: Spec 1 memory capture hooks end-to-end.
//!
//! The legacy `CompressionService` no longer calls the LLM directly; it routes
//! per-source batches through the compound ingestor, which builds a
//! source-aware system prompt via `build_compound_system_prompt`. These probes
//! verify that each raw-memory source maps to the expected specialised prompt
//! block (RESCUE / LESSON / DIGEST / RETRO).

use alephcore::memory::notes::ingest::prompts::build_compound_system_prompt;
use alephcore::memory::store::raw_memory::{RawMemorySource, SessionEndReason};

#[test]
fn pre_compress_source_uses_rescue_prompt() {
    let prompt = build_compound_system_prompt(&RawMemorySource::PreCompress);
    assert!(
        prompt.contains("memory rescue assistant"),
        "expected RESCUE prompt, got: {prompt}"
    );
}

#[test]
fn delegation_source_uses_lesson_prompt() {
    let prompt = build_compound_system_prompt(&RawMemorySource::Delegation {
        child_agent_id: "child-1".into(),
    });
    assert!(
        prompt.contains("delegating work to a sub-agent"),
        "expected LESSON prompt, got: {prompt}"
    );
}

#[test]
fn session_end_disconnect_uses_digest_prompt() {
    let prompt = build_compound_system_prompt(&RawMemorySource::SessionEnd {
        reason: SessionEndReason::Disconnect,
    });
    assert!(
        prompt.contains("end-of-session digest"),
        "expected DIGEST prompt, got: {prompt}"
    );
}

#[test]
fn session_end_task_done_uses_retro_prompt() {
    let prompt = build_compound_system_prompt(&RawMemorySource::SessionEnd {
        reason: SessionEndReason::TaskDone,
    });
    assert!(
        prompt.contains("memory retrospective assistant"),
        "expected RETRO prompt, got: {prompt}"
    );
}
