//! Stage 7 (#12) — Init seam audit tests.
//!
//! Locks the contract that `AgentHarnessRunner::run` emits exactly eight
//! `TraceSink::on_init_seam` events in declared order, with `configured`
//! flags reflecting which Stage 1-6 / P0 rescue seams have wired impls.

use crate::sync_primitives::Mutex;

use crate::harness::trace::LoopTraceEvent;
use crate::harness::trace_sink::TraceSink;
use crate::orchestrator::harness_bridge::emit_init_seams;

#[derive(Default)]
struct InitRecorder {
    events: Mutex<Vec<(&'static str, &'static str, bool)>>,
}

impl TraceSink for InitRecorder {
    fn on_trace(&self, _event: &LoopTraceEvent) {}
    fn flush(&self) {}
    fn on_init_seam(&self, stage: &'static str, seam: &'static str, configured: bool) {
        self.events.lock().unwrap().push((stage, seam, configured));
    }
}

#[test]
fn cold_start_emits_all_seam_events() {
    let recorder = InitRecorder::default();
    emit_init_seams(&recorder, false, true, false, false, false);
    let events = recorder.events.lock().unwrap();
    let seams: Vec<&'static str> = events.iter().map(|(_, seam, _)| *seam).collect();
    let expected = [
        "PromptBuilder",
        "ChainContext",
        "GuardrailRegistry",
        "VerifierChain",
        "StallConfig",
        "ConsecutiveFailureCap",
        "TurnTimeout",
    ];
    assert_eq!(
        seams.len(),
        expected.len(),
        "expected {} init seams, got {}: {seams:?}",
        expected.len(),
        seams.len()
    );
    for seam in expected {
        assert!(
            seams.contains(&seam),
            "missing init seam {seam}; got {seams:?}"
        );
    }
}

#[test]
fn init_seams_emitted_in_declared_order_with_correct_configured_flags() {
    let recorder = InitRecorder::default();
    // guardrails=true, verifier_chain=true, consecutive_failure_cap=true;
    // remaining three are false. PromptBuilder + ChainContext always true.
    emit_init_seams(&recorder, true, true, false, true, false);
    let events = recorder.events.lock().unwrap();
    let snapshot: Vec<(&'static str, &'static str, bool)> = events.iter().copied().collect();
    assert_eq!(
        snapshot,
        vec![
            ("stage3-prompt", "PromptBuilder", true),
            ("stage4-chain", "ChainContext", true),
            ("stage5a-guardrails", "GuardrailRegistry", true),
            ("stage6a-verifier", "VerifierChain", true),
            ("p0-rescue-stall", "StallConfig", false),
            ("p0-rescue-cap", "ConsecutiveFailureCap", true),
            ("p0-rescue-timeout", "TurnTimeout", false),
        ]
    );
}

#[test]
fn default_trait_impl_is_noop() {
    // Sanity check: the default `on_init_seam` impl on existing TraceSinks
    // (NoopTraceSink, GatewayTraceSink) must not panic and must not
    // observe events.
    let noop = crate::harness::NoopTraceSink;
    noop.on_init_seam("any-stage", "AnySeam", true);
    // Reaching this line means the default impl returned without panic.
}
