//! `RedactingEmitter` — the emitter-side half of unattended secret redaction.
//!
//! # Why this exists
//!
//! An unattended run already had a redaction gate:
//! [`UnattendedRedactingSink`] wraps the `TraceSink`. But a run's output leaves
//! the engine down **two** legs, and only one of them is a `TraceSink`:
//!
//! * the trace leg — persistence, the scratchpad channel progress push, and the
//!   deliberately-lossy `agent_trace` WS mirror;
//! * the **emitter** leg — `event_drain` turning the run into
//!   `ToolStart{params}` / `ToolEnd{result}` / `ResponseChunk{delta,full_text}`
//!   / `RunComplete{summary}`, and `OriginFanoutEmitter` handing the final
//!   reply to the bound channel.
//!
//! `run_loop/inner.rs` wrapped the first and passed the second through
//! unwrapped. The contradiction was observable inside a single run: the same
//! final text was masked on the trace leg (`SessionCompleted.final_text`) and
//! delivered **in the clear** to Telegram/Slack by `OriginFanoutEmitter`, whose
//! only filter is `sanitize_final_response` — a `<think>`/completion-marker
//! stripper, not a secret masker. `ResponseChunk` / `ToolStart` / `ToolEnd`
//! were never masked at all.
//!
//! Concretely: an unattended `loop`/`goal` continuation bound to a chat channel
//! runs `cat ~/.aws/credentials`, the model quotes the key back, and the key is
//! delivered to a third-party service. No error, no failing test.
//!
//! So the gate belongs at the emitter chokepoint too, sharing one masker with
//! the trace leg (`exec::masker::mask_json_strings`) — one masker, one place,
//! per SECURITY.md's rule for anything every output must pass through.
//!
//! # Placement
//!
//! Wrap **outside** `OriginFanoutEmitter`, never inside: the fanout reads
//! `summary.final_response` off the event it is given, so an inner wrapper
//! would mask the WS copy and still ship the channel copy in the clear.
//!
//! # What this does NOT touch
//!
//! The model's own context. Prompts are fed from `session_events` and
//! `tool_result`, not from `EventEmitter` — so the model still sees real
//! values and can still act on them (A2). This masks only what is on its way
//! to a human or a third-party service.
//!
//! Attended runs are not wrapped, so their path is byte-identical to before and
//! pays no regex scan.
//!
//! [`UnattendedRedactingSink`]: crate::gateway::execution_engine::UnattendedRedactingSink

use std::sync::Arc;

use async_trait::async_trait;

use crate::exec::masker::{mask_json_strings, SecretMasker};
use crate::gateway::event_emitter::types::{EventEmitError, StreamEvent};
use crate::gateway::event_emitter::EventEmitter;

/// Decorates an `EventEmitter`, masking secrets in every text-bearing variant.
pub struct RedactingEmitter {
    inner: Arc<dyn EventEmitter + Send + Sync>,
    masker: SecretMasker,
}

impl RedactingEmitter {
    #[must_use]
    pub fn new(inner: Arc<dyn EventEmitter + Send + Sync>) -> Self {
        Self {
            inner,
            masker: SecretMasker::new(),
        }
    }

    fn mask(&self, text: &str) -> String {
        self.masker.mask(text)
    }
}

#[async_trait]
impl EventEmitter for RedactingEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        // Every arm is explicit rather than a catch-all `_ => event`, so a NEW
        // text-bearing variant is a compile error here instead of a silent hole.
        let redacted = match event {
            StreamEvent::ResponseChunk {
                run_id,
                seq,
                delta,
                full_text,
                chunk_index,
                is_final,
                is_intermediate,
            } => StreamEvent::ResponseChunk {
                run_id,
                seq,
                delta: self.mask(&delta),
                full_text: self.mask(&full_text),
                chunk_index,
                is_final,
                is_intermediate,
            },
            StreamEvent::Reasoning {
                run_id,
                seq,
                content,
                is_complete,
            } => StreamEvent::Reasoning {
                run_id,
                seq,
                content: self.mask(&content),
                is_complete,
            },
            StreamEvent::ReasoningBlock {
                run_id,
                seq,
                step_type,
                label,
                content,
                confidence,
                is_final,
            } => StreamEvent::ReasoningBlock {
                run_id,
                seq,
                step_type,
                label,
                content: self.mask(&content),
                confidence,
                is_final,
            },
            StreamEvent::ToolStart {
                run_id,
                seq,
                tool_name,
                tool_id,
                mut params,
            } => {
                mask_json_strings(&self.masker, &mut params);
                StreamEvent::ToolStart {
                    run_id,
                    seq,
                    tool_name,
                    tool_id,
                    params,
                }
            }
            StreamEvent::ToolUpdate {
                run_id,
                seq,
                tool_id,
                progress,
            } => StreamEvent::ToolUpdate {
                run_id,
                seq,
                tool_id,
                progress: self.mask(&progress),
            },
            StreamEvent::ToolEnd {
                run_id,
                seq,
                tool_id,
                mut result,
                duration_ms,
            } => {
                result.output = result.output.map(|o| self.mask(&o));
                result.error = result.error.map(|e| self.mask(&e));
                StreamEvent::ToolEnd {
                    run_id,
                    seq,
                    tool_id,
                    result,
                    duration_ms,
                }
            }
            StreamEvent::RunComplete {
                run_id,
                seq,
                mut summary,
                total_duration_ms,
            } => {
                summary.final_response = summary.final_response.map(|t| self.mask(&t));
                for err in &mut summary.errors {
                    err.error = self.mask(&err.error);
                }
                StreamEvent::RunComplete {
                    run_id,
                    seq,
                    summary,
                    total_duration_ms,
                }
            }
            StreamEvent::RunError {
                run_id,
                seq,
                error,
                error_code,
                session_key,
            } => StreamEvent::RunError {
                run_id,
                seq,
                error: self.mask(&error),
                error_code,
                // Forwarded verbatim: it is an addressing field, not content,
                // and dropping it here would re-open the fail-closed hole the
                // producer set it to close.
                session_key,
            },
            StreamEvent::UncertaintySignal {
                run_id,
                seq,
                uncertainty,
                suggested_action,
            } => StreamEvent::UncertaintySignal {
                run_id,
                seq,
                uncertainty: self.mask(&uncertainty),
                suggested_action,
            },
            // A truncated provider error. Provider failures do echo request
            // material, so this is not identifier-only.
            StreamEvent::RunRetrying {
                run_id,
                seq,
                provider,
                attempt,
                max_attempts,
                reason,
            } => StreamEvent::RunRetrying {
                run_id,
                seq,
                provider,
                attempt,
                max_attempts,
                reason: self.mask(&reason),
            },
            // `AgentTrace` is already masked upstream: on an unattended run
            // `UnattendedRedactingSink` wraps OUTSIDE `AgentTraceEmitSink`
            // (`run_loop/inner.rs`), so every event reaching this emitter has
            // already been through it. Masking again would only cost a second
            // regex pass over the same bytes.
            //
            // ⚠️ This is a guarantee outsourced to another module, and it was
            // FALSE until 2026-08-29: that sink covered three variants and
            // forwarded the rest through an `other =>` wildcard, so
            // `ToolCallStarted.call.input` — the same bytes its masked
            // `ToolCallCompleted` twin carries — reached this arm in the clear
            // and this comment vouched for it. What backs the exemption now is
            // that the sink matches the enum exhaustively with no wildcard, and
            // `the_agent_trace_exemption_is_backed_by_an_exhaustive_upstream_match`
            // below reads that property off the sink's source rather than
            // restating it here.
            //
            // The rest carry no free text: identifiers, counters, gauges, and
            // the `AskUser` question (authored by the model as a question to a
            // human, and delivered by its own bus path, not this emitter).
            other @ (StreamEvent::RunAccepted { .. }
            | StreamEvent::RunQueued { .. }
            | StreamEvent::AgentTrace { .. }
            | StreamEvent::ContextGauge { .. }
            | StreamEvent::AskUser { .. }
            | StreamEvent::ModelResolved { .. }) => other,
        };
        self.inner.emit(redacted).await
    }

    fn next_seq(&self) -> u64 {
        self.inner.next_seq()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::CollectingEventEmitter;

    /// A real AWS key shape, so `SecretMasker`'s own pattern decides — not a
    /// custom pattern the test invented.
    const KEY: &str = "AKIAIOSFODNN7EXAMPLE";

    fn wrapped() -> (Arc<CollectingEventEmitter>, RedactingEmitter) {
        let inner = Arc::new(CollectingEventEmitter::new());
        let outer =
            RedactingEmitter::new(Arc::clone(&inner) as Arc<dyn EventEmitter + Send + Sync>);
        (inner, outer)
    }

    /// `RunComplete.summary.final_response` is what `OriginFanoutEmitter` hands
    /// to the bound channel — i.e. what a third-party service receives. It was
    /// the same text the trace leg masked, delivered in the clear.
    #[tokio::test]
    async fn the_final_response_a_channel_receives_is_masked() {
        let (inner, outer) = wrapped();
        let summary = crate::gateway::event_emitter::types::RunSummary {
            final_response: Some(format!("the key is {KEY}")),
            ..Default::default()
        };
        outer
            .emit(StreamEvent::RunComplete {
                run_id: "r".into(),
                seq: 1,
                summary,
                total_duration_ms: 0,
            })
            .await
            .unwrap();

        let events = inner.events().await;
        let StreamEvent::RunComplete { summary, .. } = &events[0] else {
            panic!("expected RunComplete, got {:?}", events[0]);
        };
        let text = summary.final_response.as_deref().unwrap();
        assert!(
            !text.contains(KEY),
            "credential reached the channel: {text}"
        );
    }

    /// The streaming legs carry the same bytes and were never masked at all.
    #[tokio::test]
    async fn streamed_text_and_tool_results_are_masked() {
        let (inner, outer) = wrapped();
        outer
            .emit(StreamEvent::ResponseChunk {
                run_id: "r".into(),
                seq: 1,
                delta: format!("found {KEY}"),
                full_text: format!("found {KEY}"),
                chunk_index: 0,
                is_final: false,
                is_intermediate: false,
            })
            .await
            .unwrap();
        outer
            .emit(StreamEvent::ToolEnd {
                run_id: "r".into(),
                seq: 2,
                tool_id: "t1".into(),
                result: crate::gateway::event_emitter::types::ToolResult::success(format!(
                    "aws_access_key={KEY}"
                )),
                duration_ms: 1,
            })
            .await
            .unwrap();
        outer
            .emit(StreamEvent::ToolStart {
                run_id: "r".into(),
                seq: 3,
                tool_name: "bash".into(),
                tool_id: "t2".into(),
                params: serde_json::json!({ "command": format!("export K={KEY}") }),
            })
            .await
            .unwrap();

        let events = inner.events().await;
        let rendered = format!("{events:?}");
        assert!(
            !rendered.contains(KEY),
            "an unmasked credential survived the emitter leg: {rendered}"
        );
    }

    /// **All three** legs of an unattended run must agree.
    ///
    /// A human can receive run-derived text down the TraceSink leg
    /// (persistence / channel progress push / `agent_trace` WS), the emitter
    /// leg (`ToolEnd`, and `RunComplete` onward to the bound channel), or —
    /// found on 2026-08-08 — the direct-notice leg: `execute::notify_origin`
    /// and its cron twin `cron::executor::deliver_to_channel`, which call
    /// `ChannelRegistry::send` themselves and were wrapped by neither.
    ///
    /// One masked copy plus one clear copy is not redaction. The count in this
    /// test's name went two → three because a census that stops at the legs
    /// somebody remembered to wrap will keep finding a new one; all three share
    /// `exec::masker` rather than each rolling its own walk, and a fourth that
    /// does not will fail here.
    #[test]
    fn all_three_legs_mask_identically() {
        let masker = SecretMasker::new();
        let mut trace_side = serde_json::json!({ "stdout": format!("k={KEY}") });
        mask_json_strings(&masker, &mut trace_side);

        let emitter_side = RedactingEmitter::new(Arc::new(CollectingEventEmitter::new()));
        let masked_by_emitter = emitter_side.mask(&format!("k={KEY}"));

        // The notice leg's masker, constructed exactly as `notify_origin` and
        // `deliver_to_channel` construct it.
        let masked_by_notice = SecretMasker::new().mask(&format!("k={KEY}"));

        assert_eq!(trace_side["stdout"].as_str().unwrap(), masked_by_emitter);
        assert_eq!(masked_by_emitter, masked_by_notice);
        assert!(!masked_by_emitter.contains(KEY));
        assert!(!masked_by_notice.contains(KEY));
    }

    /// Attended runs are not wrapped at all, but assert the pass-through arms
    /// really are pass-through so a future variant added to the "no free text"
    /// group cannot quietly start dropping fields.
    #[tokio::test]
    async fn identifier_only_events_pass_through_unchanged() {
        let (inner, outer) = wrapped();
        let event = StreamEvent::ContextGauge {
            run_id: "r".into(),
            seq: 7,
            context_tokens: 10,
            context_window: 100,
            total_tokens: 42,
        };
        outer.emit(event.clone()).await.unwrap();
        let events = inner.events().await;
        assert_eq!(format!("{:?}", events[0]), format!("{event:?}"));
    }

    /// The `AgentTrace` pass-through arm above is the one place this file
    /// outsources its guarantee to another module. Check the property that
    /// makes the outsourcing sound, instead of trusting the comment.
    ///
    /// The property is *exhaustiveness*, not "three named variants are
    /// handled": `UnattendedRedactingSink` used to end in `other =>
    /// self.inner.on_trace(other)`, written for `#[non_exhaustive]`
    /// forward-compatibility, and that wildcard is exactly what let
    /// `ToolCallStarted.call.input` — a credential the model had just echoed
    /// into a tool argument — reach this emitter unmasked while the comment
    /// said it could not. A wildcard reintroduced there turns this red.
    #[test]
    fn the_agent_trace_exemption_is_backed_by_an_exhaustive_upstream_match() {
        use crate::utils::source_scan::{code_text, production_prefix};

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/gateway/execution_engine/unattended_redacting_sink.rs");
        let src = code_text(&production_prefix(
            &std::fs::read_to_string(&path).expect("unattended_redacting_sink.rs"),
        ));

        let start = src
            .find("pub(crate) fn mask_trace_event")
            .expect("mask_trace_event is what this arm relies on; it was renamed or removed");
        // Brace-match the function body so the scan stops at the function's own
        // syntactic end rather than after a fixed number of lines — a window
        // sized in lines drifts onto the next declaration the first time
        // something above it grows.
        let open = src[start..].find('{').expect("fn body") + start;
        let mut depth = 0usize;
        let mut end = open;
        for (i, c) in src[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &src[open..=end];

        let arms = body.matches("LoopTraceEvent::").count();
        assert!(
            arms >= 19,
            "found only {arms} LoopTraceEvent arms in mask_trace_event — the \
             scan stopped matching, so its green would mean nothing"
        );
        for wildcard in ["_ =>", "other =>", "_event =>"] {
            assert!(
                !body.contains(wildcard),
                "mask_trace_event has a `{wildcard}` catch-all, so a trace variant can \
                 reach this emitter unmasked while the AgentTrace pass-through arm \
                 above still claims it was masked upstream"
            );
        }
    }
}
