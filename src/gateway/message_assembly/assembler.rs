//! The single owner of a run's assembled agent message.
//!
//! Ports the kosong `merge_in_place` / pi `partial` pattern: one reducer per
//! run/consumer that accumulates the deliverable answer and the reasoning,
//! stripping reasoning/completion/memory framing from the *live* visible
//! stream across delta boundaries (G4). `snapshot()` (the live `full_text`)
//! and `finalize().answer` (the terminal `final_response`) derive from the
//! same accumulator and can never disagree — the anti-drift invariant.

use crate::memory::streaming_scrubber::{StreamingContextScrubber, DISCARD_TAG_PAIRS};

/// The terminal, deliverable form of a run's assembled message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssembledMessage {
    /// Sanitized visible answer; `None` when nothing deliverable survives.
    pub answer: Option<String>,
    /// Accumulated reasoning; `None` when empty.
    pub reasoning: Option<String>,
}

/// Reducer over a run's streamed text + reasoning.
#[derive(Debug)]
pub struct MessageAssembler {
    visible: String,
    reasoning: String,
    chunk_index: u32,
    scrubber: StreamingContextScrubber,
}

impl Default for MessageAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageAssembler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            visible: String::new(),
            reasoning: String::new(),
            chunk_index: 0,
            scrubber: StreamingContextScrubber::with_tag_set(DISCARD_TAG_PAIRS),
        }
    }

    /// Feed a raw text delta. Returns the cleaned, user-visible slice to stream
    /// now (empty if fully absorbed or held back as a partial-tag tail).
    pub fn push_text_delta(&mut self, raw: &str) -> String {
        let visible = self.scrubber.feed(raw);
        if !visible.is_empty() {
            self.visible.push_str(&visible);
        }
        visible
    }

    /// Feed a reasoning delta (from `FlowStreamEvent::Reasoning`).
    pub fn push_reasoning_delta(&mut self, raw: &str) {
        self.reasoning.push_str(raw);
    }

    /// Drain any held-back tag tail at a tool-call / run boundary.
    pub fn flush_boundary(&mut self) -> String {
        let tail = self.scrubber.flush();
        if !tail.is_empty() {
            self.visible.push_str(&tail);
        }
        tail
    }

    /// The live full visible snapshot — populates `ResponseChunk.full_text`.
    #[must_use]
    pub fn snapshot(&self) -> &str {
        &self.visible
    }

    /// Next monotonic chunk index.
    pub fn next_chunk_index(&mut self) -> u32 {
        let idx = self.chunk_index;
        self.chunk_index += 1;
        idx
    }

    /// Terminal answer + reasoning. Flushes any held tail first, then applies
    /// the idempotent final sanitizer (catches self-closing `<task-complete/>`
    /// and trailing incomplete directives the streaming scrubber leaves).
    pub fn finalize(&mut self) -> AssembledMessage {
        let _ = self.flush_boundary();
        let answer = finalize_sanitize(&self.visible);
        let reasoning = if self.reasoning.trim().is_empty() {
            None
        } else {
            Some(self.reasoning.clone())
        };
        AssembledMessage { answer, reasoning }
    }
}

/// Final sanitize pass. Inlined in Task 2; Task 4 replaces this body with a
/// delegation to `crate::gateway::reply_emitter::sanitize_final_text`.
fn finalize_sanitize(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
