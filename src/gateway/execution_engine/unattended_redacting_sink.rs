//! `UnattendedRedactingSink` — secret redaction for unattended autonomous runs.
//!
//! Round 2 made unattended runs fail closed on tool confirmation. This closes
//! the observability side: when no human is watching, model-authored trace text
//! (which could echo a secret the loop just read) is run through `SecretMasker`
//! before it reaches persistence, the channel progress push, or the WebSocket
//! stream. Attended runs are never wrapped, so their trace path is unchanged.
//!
//! Lives in `src/gateway/` (a `TraceSink` consumer), not `src/harness/` (R10).

use std::sync::Arc;

use crate::exec::masker::{mask_json_strings, SecretMasker};
use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;

/// Wraps an inner `TraceSink`, redacting the text-bearing `LoopTraceEvent`
/// variants: `TextEmitted`, `SessionCompleted.final_text`, and
/// `ToolCallCompleted` (tool input + result — the payload the scratchpad
/// channel push and the WS `agent_trace` stream actually deliver; leaving it
/// unmasked contradicted this module's own coverage claim). All other variants
/// forward by reference, unchanged (`#[non_exhaustive]`-safe wildcard).
pub struct UnattendedRedactingSink {
    inner: Arc<dyn TraceSink>,
    masker: SecretMasker,
}

impl UnattendedRedactingSink {
    #[must_use]
    pub fn new(inner: Arc<dyn TraceSink>) -> Self {
        Self {
            inner,
            masker: SecretMasker::new(),
        }
    }
}

impl TraceSink for UnattendedRedactingSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        match event {
            LoopTraceEvent::TextEmitted {
                iteration,
                stream,
                text,
            } => {
                let redacted = self.masker.mask(text);
                if redacted == *text {
                    self.inner.on_trace(event);
                } else {
                    self.inner.on_trace(&LoopTraceEvent::TextEmitted {
                        iteration: *iteration,
                        stream: *stream,
                        text: redacted,
                    });
                }
            }
            LoopTraceEvent::SessionCompleted {
                final_text: Some(t),
                ..
            } => {
                let redacted = self.masker.mask(t);
                if redacted == *t {
                    self.inner.on_trace(event);
                } else {
                    // Clone the whole event and overwrite only final_text;
                    // the other fields (outcome, tokens, timeline…) are
                    // preserved verbatim.
                    let mut ev = event.clone();
                    if let LoopTraceEvent::SessionCompleted { final_text, .. } = &mut ev {
                        *final_text = Some(redacted);
                    }
                    self.inner.on_trace(&ev);
                }
            }
            LoopTraceEvent::ToolCallCompleted { .. } => {
                // Tool results are the highest-bandwidth secret channel in an
                // unattended run: a tool that read a credential echoes it in
                // `result` (which the scratchpad progress push sends to the
                // bound channel and `AgentTraceEmitSink` puts on the WS), and
                // the model can echo one into `call.input` (a scratchpad
                // objective/plan). Mask every string leaf of both; forward the
                // original event untouched when nothing matched.
                let mut ev = event.clone();
                let LoopTraceEvent::ToolCallCompleted { call, result, .. } = &mut ev else {
                    unreachable!("matched ToolCallCompleted above");
                };
                let mut changed = mask_json_strings(&self.masker, &mut call.input);
                match result {
                    crate::tools::runtime::ToolResult::Success { output } => {
                        changed |= mask_json_strings(&self.masker, output);
                    }
                    crate::tools::runtime::ToolResult::Error { error, .. } => {
                        let masked = self.masker.mask(error);
                        if masked != *error {
                            *error = masked;
                            changed = true;
                        }
                    }
                }
                if changed {
                    self.inner.on_trace(&ev);
                } else {
                    self.inner.on_trace(event);
                }
            }
            other => self.inner.on_trace(other),
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::trace::{LoopTraceSessionOutcome, LoopTraceTextKind};
    use std::sync::Mutex;

    #[derive(Default)]
    struct CaptureSink {
        events: Mutex<Vec<LoopTraceEvent>>,
    }
    impl TraceSink for CaptureSink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
        fn flush(&self) {}
    }

    #[test]
    fn redacts_secret_in_text_emitted() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: LoopTraceTextKind::Final,
            text: "the key is sk-ant-api03-AAAABBBBCCCCDDDD".into(),
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::TextEmitted { text, .. } => {
                assert!(
                    !text.contains("sk-ant-api03-AAAABBBBCCCCDDDD"),
                    "secret leaked: {text}"
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn passes_clean_text_through_unchanged() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: LoopTraceTextKind::Final,
            text: "just a normal sentence".into(),
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::TextEmitted { text, .. } => assert_eq!(text, "just a normal sentence"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn redacts_secret_in_tool_call_completed_result_and_input() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: crate::harness::trace::ToolCallEndEvent {
                tool_id: "t1".into(),
                tool_name: "scratchpad".into(),
                input: serde_json::json!({
                    "action": "set_objective",
                    "objective": "rotate key sk-ant-api03-AAAABBBBCCCCDDDD"
                }),
                duration_ms: 5,
            },
            result: crate::tools::runtime::ToolResult::Success {
                output: serde_json::json!({
                    "content": "- [ ] use AKIAIOSFODNN7EXAMPLE to sign"
                }),
            },
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::ToolCallCompleted { call, result, .. } => {
                let input = call.input.to_string();
                assert!(
                    !input.contains("sk-ant-api03-AAAABBBBCCCCDDDD"),
                    "input leaked"
                );
                let crate::tools::runtime::ToolResult::Success { output } = result else {
                    panic!("expected success result");
                };
                assert!(
                    !output.to_string().contains("AKIAIOSFODNN7EXAMPLE"),
                    "result leaked: {output}"
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn clean_tool_call_completed_forwards_unchanged() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::ToolCallCompleted {
            iteration: 2,
            call: crate::harness::trace::ToolCallEndEvent {
                tool_id: "t2".into(),
                tool_name: "read_file".into(),
                input: serde_json::json!({"path": "README.md"}),
                duration_ms: 3,
            },
            result: crate::tools::runtime::ToolResult::Success {
                output: serde_json::json!({"content": "plain text"}),
            },
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::ToolCallCompleted { call, .. } => {
                assert_eq!(call.input["path"], "README.md");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn redacts_secret_in_session_completed_final_text() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::SessionCompleted {
            outcome: LoopTraceSessionOutcome::Completed,
            iterations: 1,
            tool_calls_made: 0,
            total_tokens: 0,
            hit_limit: false,
            final_text: Some("done, token AKIAIOSFODNN7EXAMPLE used".into()),
            terminate_reason: None,
            duration_ms: None,
            token_breakdown: None,
            tool_timeline: Vec::new(),
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::SessionCompleted { final_text, .. } => {
                assert!(!final_text
                    .as_ref()
                    .unwrap()
                    .contains("AKIAIOSFODNN7EXAMPLE"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
