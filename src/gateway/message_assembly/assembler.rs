//! The single owner of a run's live visible message stream.
//!
//! Ports the kosong `merge_in_place` / pi `partial` pattern to Aleph's Rust
//! core: one reducer that accumulates the deliverable answer as it streams,
//! stripping reasoning/completion/memory framing from the *live* visible
//! stream across delta boundaries (G4). `snapshot()` is the running visible
//! text that populates `ResponseChunk.full_text`.
//!
//! # Consumers
//!
//! The event drain (`execution_engine::event_drain`) is the sole consumer: it
//! feeds raw deltas through `push_text_delta`, streams the cleaned slice, reads
//! `snapshot` / `next_chunk_index` for each `ResponseChunk`, and calls
//! `flush_boundary` / `reset_iteration` at tool-call boundaries. The terminal
//! answer there comes from `FlowOutcome`, not this reducer — so this type owns
//! only the *streaming* surface. The shared final-answer sanitize atom lives in
//! `reply_emitter::sanitize_final_text` (used by every delivery path); it is
//! deliberately not duplicated here.

use crate::memory::streaming_scrubber::{StreamingContextScrubber, DISCARD_TAG_PAIRS};

/// Reducer over a run's streamed visible text.
#[derive(Debug)]
pub struct MessageAssembler {
    visible: String,
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

    /// Clear the visible accumulator at a think-iteration boundary while
    /// preserving the monotonic `chunk_index`. The drain calls this at each
    /// tool-call boundary so every iteration's `full_text` starts fresh
    /// (matching the prior per-iteration semantics), while the chunk counter
    /// stays monotonic across the whole run. Precondition: the scrubber was
    /// just drained via `flush_boundary`, so no partial-tag tail is stranded.
    pub fn reset_iteration(&mut self) {
        self.visible.clear();
    }
}
