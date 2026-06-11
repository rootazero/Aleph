//! UnattendedRedactingSink — secret redaction for unattended autonomous runs.
//!
//! Round 2 made unattended runs fail closed on tool confirmation. This closes
//! the observability side: when no human is watching, model-authored trace text
//! (which could echo a secret the loop just read) is run through `SecretMasker`
//! before it reaches persistence, the channel progress push, or the WebSocket
//! stream. Attended runs are never wrapped, so their trace path is unchanged.
//!
//! Lives in `src/gateway/` (a TraceSink consumer), not `src/harness/` (R10).

use std::sync::Arc;

use crate::exec::masker::SecretMasker;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;

/// Wraps an inner `TraceSink`, redacting model-authored text on the two
/// text-bearing `LoopTraceEvent` variants. All other variants forward by
/// reference, unchanged (`#[non_exhaustive]`-safe wildcard).
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
            other => self.inner.on_trace(other),
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }

    fn on_init_seam(&self, stage: &'static str, seam: &'static str, configured: bool) {
        self.inner.on_init_seam(stage, seam, configured);
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
                assert!(!text.contains("sk-ant-api03-AAAABBBBCCCCDDDD"), "secret leaked: {text}");
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
                assert!(!final_text.as_ref().unwrap().contains("AKIAIOSFODNN7EXAMPLE"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
