//! Real-time streaming bridge: ProviderDelta → EventEmitter
//!
//! Created per-run by ExecutionEngine, injected into AgentLoop via with_delta_sink().
//! Uses semantic boundary detection for natural text chunking:
//! - Hard boundaries (\n\n, sentence endings, code fences) → flush immediately
//! - Soft boundary (single \n) → flush after 30ms
//! - No boundary → flush after 50ms

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::gateway::event_emitter::EventEmitter;
use crate::providers::adapter::StopReason;
use crate::providers::delta::{DeltaSink, ProviderDelta};
use crate::sync_primitives::Arc;

/// Hard flush timeout (ms) — force flush if no semantic boundary found
const FLUSH_TIMEOUT_MS: u64 = 50;

/// Soft boundary timeout (ms) — shorter timeout after single newline
const SOFT_BOUNDARY_TIMEOUT_MS: u64 = 30;

/// Check if buffer ends with a hard semantic boundary (immediate flush).
fn has_semantic_boundary(s: &str) -> bool {
    s.ends_with("\n\n")
        || s.ends_with(". ")
        || s.ends_with("。")
        || s.ends_with("？")
        || s.ends_with("！")
        || s.ends_with("! ")
        || s.ends_with("? ")
        || s.lines()
            .last()
            .is_some_and(|line| line.trim_start().starts_with("```"))
}

/// Check if buffer ends with a soft boundary (shorter timeout).
fn has_soft_boundary(s: &str) -> bool {
    s.ends_with('\n') && !s.ends_with("\n\n")
}

/// Bridges ProviderDelta events to the Gateway EventEmitter for real-time text streaming.
///
/// Created per-run by ExecutionEngine and injected into AgentLoop via with_delta_sink().
/// Uses semantic boundary detection to align chunks with natural text boundaries,
/// providing better rendering on the frontend.
///
/// On `Done`, flushes any remaining buffer:
/// - `Done(ToolUse)`: intermediate iteration — flushes with `is_intermediate = true`,
///   emits boundary marker (empty content, `is_intermediate = true`, `is_final = false`).
///   Resets `accumulated` for the next iteration.
/// - `Done(EndTurn | MaxTokens)`: final iteration — flushes normally and emits
///   final marker (empty content, `is_final = true`) to signal stream completion.
pub struct StreamingDeltaSink {
    run_id: String,
    emitter: Arc<dyn EventEmitter + Send + Sync>,
    chunk_index: AtomicU32,
    /// Shared with StreamCallback to coordinate text deduplication.
    /// Set to true when at least one TextDelta has been emitted.
    /// StreamCallback reads and resets via swap(false).
    has_emitted_text: Arc<AtomicBool>,
    buffer: Mutex<String>,
    last_emit: Mutex<Instant>,
    /// Accumulated full text within the current iteration.
    /// Reset on intermediate boundaries (Done(ToolUse)).
    accumulated: Mutex<String>,
}

impl StreamingDeltaSink {
    /// Create a new StreamingDeltaSink.
    ///
    /// # Arguments
    /// - `run_id` — the run identifier, forwarded to all emitted events
    /// - `emitter` — the EventEmitter to broadcast response chunks to
    /// - `has_emitted_text` — shared flag; set to `true` after any TextDelta is forwarded
    pub fn new(
        run_id: String,
        emitter: Arc<dyn EventEmitter + Send + Sync>,
        has_emitted_text: Arc<AtomicBool>,
    ) -> Self {
        Self {
            run_id,
            emitter,
            chunk_index: AtomicU32::new(0),
            has_emitted_text,
            buffer: Mutex::new(String::new()),
            last_emit: Mutex::new(Instant::now()),
            accumulated: Mutex::new(String::new()),
        }
    }
}

#[async_trait]
impl DeltaSink for StreamingDeltaSink {
    async fn on_delta(&self, delta: &ProviderDelta) {
        match delta {
            ProviderDelta::TextDelta(text) => {
                let mut buf = self.buffer.lock().await;
                buf.push_str(text);

                let last = self.last_emit.lock().await;
                let elapsed = last.elapsed().as_millis() as u64;
                drop(last);

                // Determine flush threshold based on semantic boundaries
                let should_flush = if has_semantic_boundary(&buf) {
                    // Hard boundary: flush immediately
                    true
                } else if has_soft_boundary(&buf) {
                    // Soft boundary: flush after shorter timeout
                    elapsed >= SOFT_BOUNDARY_TIMEOUT_MS
                } else {
                    // No boundary: flush after standard timeout
                    elapsed >= FLUSH_TIMEOUT_MS
                };

                if should_flush {
                    let content = std::mem::take(&mut *buf);
                    drop(buf);

                    // Update accumulated text
                    let mut acc = self.accumulated.lock().await;
                    acc.push_str(&content);
                    let full_text = acc.clone();
                    drop(acc);

                    let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                    self.emitter
                        .emit_response_chunk(&self.run_id, &content, &full_text, idx, false, false)
                        .await;
                    self.has_emitted_text.store(true, Ordering::Release);
                    *self.last_emit.lock().await = Instant::now();
                }
            }
            ProviderDelta::Done(stop_reason) => {
                let is_intermediate = matches!(stop_reason, StopReason::ToolUse);

                // Flush any remaining buffered text
                let mut buf = self.buffer.lock().await;
                let remaining = std::mem::take(&mut *buf);
                drop(buf);

                if !remaining.is_empty() {
                    // Update accumulated with remaining buffer
                    let mut acc = self.accumulated.lock().await;
                    acc.push_str(&remaining);
                    let full_text = acc.clone();
                    drop(acc);

                    let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                    self.emitter
                        .emit_response_chunk(
                            &self.run_id,
                            &remaining,
                            &full_text,
                            idx,
                            false,
                            false,
                        )
                        .await;
                    self.has_emitted_text.store(true, Ordering::Release);
                }

                // Get accumulated full_text for the marker
                let full_text = self.accumulated.lock().await.clone();

                let idx = self.chunk_index.fetch_add(1, Ordering::Relaxed);
                if is_intermediate {
                    // Intermediate boundary: tell client to finalize accumulated
                    // text as a standalone intermediate message
                    self.emitter
                        .emit_response_chunk(&self.run_id, "", &full_text, idx, false, true)
                        .await;
                    // Reset accumulated for next iteration
                    self.accumulated.lock().await.clear();
                } else {
                    // Final marker: stream complete
                    self.emitter
                        .emit_response_chunk(&self.run_id, "", &full_text, idx, true, false)
                        .await;
                }
            }
            // ThinkingDelta, ToolCall*, Usage, Error — not handled in Phase 2
            _ => {}
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::{CollectingEventEmitter, StreamEvent};
    use crate::providers::adapter::StopReason;
    use std::sync::atomic::AtomicBool;

    /// Helper: create a sink backed by a CollectingEventEmitter
    fn make_sink(
        run_id: &str,
        has_emitted_text: Arc<AtomicBool>,
    ) -> (StreamingDeltaSink, Arc<CollectingEventEmitter>) {
        let emitter = Arc::new(CollectingEventEmitter::new());
        let sink = StreamingDeltaSink::new(
            run_id.to_string(),
            emitter.clone() as Arc<dyn EventEmitter + Send + Sync>,
            has_emitted_text,
        );
        (sink, emitter)
    }

    #[test]
    fn test_semantic_boundary_detection() {
        assert!(has_semantic_boundary("hello\n\n"));
        assert!(has_semantic_boundary("hello. "));
        assert!(has_semantic_boundary("你好。"));
        assert!(has_semantic_boundary("what？"));
        assert!(has_semantic_boundary("wow！"));
        assert!(has_semantic_boundary("hello! "));
        assert!(has_semantic_boundary("what? "));
        assert!(has_semantic_boundary("```rust\n"));
        assert!(has_semantic_boundary("some code\n```"));
        assert!(!has_semantic_boundary("hello\n"));
        assert!(!has_semantic_boundary("hello world"));
        assert!(!has_semantic_boundary("hello"));
    }

    #[test]
    fn test_soft_boundary_detection() {
        assert!(has_soft_boundary("hello\n"));
        assert!(!has_soft_boundary("hello world"));
        assert!(!has_soft_boundary("hello\n\n"));
    }

    /// Send TextDelta followed by Done; expect buffer flushed and final marker emitted.
    #[tokio::test]
    async fn test_done_flushes_buffer_and_emits_final() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, emitter) = make_sink("run-1", flag.clone());

        // TextDelta — will be buffered (elapsed < timeout)
        sink.on_delta(&ProviderDelta::TextDelta("hello world".to_string()))
            .await;

        // Done — should flush buffer + emit final marker
        sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn))
            .await;

        let events = emitter.events().await;

        // Must have at least two events: the flushed text chunk and the final marker
        assert!(
            events.len() >= 2,
            "Expected at least 2 events, got {}",
            events.len()
        );

        // The second-to-last event should contain the text content
        let text_event = events.iter().find(|e| {
            matches!(e, StreamEvent::ResponseChunk { content, is_final, .. }
                if !content.is_empty() && !*is_final)
        });
        assert!(
            text_event.is_some(),
            "Expected a non-final chunk with text content"
        );

        // The last event must be the final marker
        let last = events.last().unwrap();
        assert!(
            matches!(last, StreamEvent::ResponseChunk { content, is_final, .. }
                if *is_final && content.is_empty()),
            "Last event must be final empty marker, got: {:?}",
            last
        );
    }

    /// Verify has_emitted_text flag is set after a TextDelta emission.
    #[tokio::test]
    async fn test_has_emitted_text_flag_set() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, _emitter) = make_sink("run-2", flag.clone());

        // Not set before any deltas
        assert!(!flag.load(Ordering::Acquire));

        // Done with buffered text → flush triggers flag
        sink.on_delta(&ProviderDelta::TextDelta("some text".to_string()))
            .await;
        sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn))
            .await;

        assert!(
            flag.load(Ordering::Acquire),
            "has_emitted_text flag should be true after text emission"
        );
    }

    /// Non-text deltas (ThinkingDelta, ToolCallStart) should produce no events.
    #[tokio::test]
    async fn test_non_text_deltas_ignored() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, emitter) = make_sink("run-3", flag.clone());

        sink.on_delta(&ProviderDelta::ThinkingDelta("hmm".to_string()))
            .await;
        sink.on_delta(&ProviderDelta::ToolCallStart {
            id: "tc1".to_string(),
            name: "search".to_string(),
        })
        .await;

        let events = emitter.events().await;
        assert!(
            events.is_empty(),
            "Non-text deltas should not produce any events"
        );
        assert!(!flag.load(Ordering::Acquire), "Flag should remain unset");
    }

    /// Done(ToolUse) should flush text as non-intermediate, then emit boundary marker.
    #[tokio::test]
    async fn test_tool_use_done_emits_intermediate_boundary() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, emitter) = make_sink("run-int", flag.clone());

        sink.on_delta(&ProviderDelta::TextDelta("Setting up team...".to_string()))
            .await;
        sink.on_delta(&ProviderDelta::Done(StopReason::ToolUse))
            .await;

        let events = emitter.events().await;
        assert!(
            events.len() >= 2,
            "Expected at least 2 events, got {}",
            events.len()
        );

        // Flushed text should be non-intermediate (emitters buffer it, boundary flushes)
        let text_event = events.iter().find(|e| {
            matches!(e, StreamEvent::ResponseChunk { content, is_intermediate, .. }
                if !content.is_empty() && !*is_intermediate)
        });
        assert!(
            text_event.is_some(),
            "Flushed text should be non-intermediate"
        );

        // Last event: intermediate boundary marker (empty, is_intermediate=true, is_final=false)
        let last = events.last().unwrap();
        assert!(
            matches!(last, StreamEvent::ResponseChunk { content, is_final, is_intermediate, .. }
                if content.is_empty() && !*is_final && *is_intermediate),
            "Last event must be intermediate boundary, got: {:?}",
            last
        );
    }

    /// Multi-iteration: intermediate then final
    #[tokio::test]
    async fn test_intermediate_then_final_iterations() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, emitter) = make_sink("run-multi", flag.clone());

        // Iteration 1: intermediate
        sink.on_delta(&ProviderDelta::TextDelta("Thinking...".to_string()))
            .await;
        sink.on_delta(&ProviderDelta::Done(StopReason::ToolUse))
            .await;

        // Iteration 2: final
        sink.on_delta(&ProviderDelta::TextDelta("Done!".to_string()))
            .await;
        sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn))
            .await;

        let events = emitter.events().await;

        // Should have intermediate boundary (no is_final) and final marker (is_final=true)
        let has_intermediate_boundary = events.iter().any(|e| {
            matches!(e, StreamEvent::ResponseChunk { content, is_final, is_intermediate, .. }
                if content.is_empty() && !*is_final && *is_intermediate)
        });
        let has_final_marker = events.iter().any(|e| {
            matches!(e, StreamEvent::ResponseChunk { is_final, is_intermediate, .. }
                if *is_final && !*is_intermediate)
        });

        assert!(
            has_intermediate_boundary,
            "Must have intermediate boundary marker"
        );
        assert!(has_final_marker, "Must have final marker");
    }

    /// Done with no prior TextDelta should only emit the final marker (no text chunk).
    #[tokio::test]
    async fn test_empty_buffer_done_only_emits_final() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, emitter) = make_sink("run-4", flag.clone());

        sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn))
            .await;

        let events = emitter.events().await;
        assert_eq!(
            events.len(),
            1,
            "Should emit exactly one event (the final marker)"
        );

        let only = &events[0];
        assert!(
            matches!(only, StreamEvent::ResponseChunk { content, is_final, .. }
                if *is_final && content.is_empty()),
            "The single event should be the final empty marker, got: {:?}",
            only
        );

        // Flag should remain unset since no text was emitted
        assert!(
            !flag.load(Ordering::Acquire),
            "Flag should remain unset when no text was emitted"
        );
    }

    /// Verify accumulated text resets between iterations (on intermediate Done(ToolUse))
    #[tokio::test]
    async fn test_accumulated_resets_on_intermediate() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, emitter) = make_sink("run-acc", flag.clone());

        // Iteration 1
        sink.on_delta(&ProviderDelta::TextDelta("First part.".to_string()))
            .await;
        sink.on_delta(&ProviderDelta::Done(StopReason::ToolUse))
            .await;

        // Iteration 2
        sink.on_delta(&ProviderDelta::TextDelta("Second part.".to_string()))
            .await;
        sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn))
            .await;

        let events = emitter.events().await;

        // Find text chunks and verify full_text resets between iterations
        let text_chunks: Vec<_> = events
            .iter()
            .filter(
                |e| matches!(e, StreamEvent::ResponseChunk { content, .. } if !content.is_empty()),
            )
            .collect();

        assert!(
            text_chunks.len() >= 2,
            "Should have text chunks from both iterations"
        );

        if let StreamEvent::ResponseChunk { full_text, .. } = &text_chunks[0] {
            assert!(full_text.contains("First part."));
        }

        if let StreamEvent::ResponseChunk { full_text, .. } = text_chunks.last().unwrap() {
            assert!(
                !full_text.contains("First part."),
                "accumulated should reset between iterations"
            );
            assert!(full_text.contains("Second part."));
        }
    }

    /// Verify full_text accumulates within a single iteration
    #[tokio::test]
    async fn test_full_text_accumulates_within_iteration() {
        let flag = Arc::new(AtomicBool::new(false));
        let (sink, emitter) = make_sink("run-acc2", flag.clone());

        // Send text with a hard boundary to force immediate flush
        sink.on_delta(&ProviderDelta::TextDelta("Hello. ".to_string()))
            .await;
        // Another chunk with hard boundary
        sink.on_delta(&ProviderDelta::TextDelta("World. ".to_string()))
            .await;
        sink.on_delta(&ProviderDelta::Done(StopReason::EndTurn))
            .await;

        let events = emitter.events().await;

        // Find chunks with non-empty content
        let text_chunks: Vec<_> = events
            .iter()
            .filter(
                |e| matches!(e, StreamEvent::ResponseChunk { content, .. } if !content.is_empty()),
            )
            .collect();

        // The last text chunk's full_text should contain all accumulated text
        if let Some(StreamEvent::ResponseChunk { full_text, .. }) = text_chunks.last() {
            assert!(
                full_text.contains("Hello."),
                "full_text should contain earlier text"
            );
        }
    }
}
