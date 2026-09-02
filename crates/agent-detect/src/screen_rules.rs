// Ported from herdr 0.8.2 (https://github.com/herdrdev/herdr).
// Copyright the herdr authors. Licensed under the Apache License, Version 2.0.
// See ../NOTICE. This file never carried the Remote manifest source (that
// lived only in `manifest.rs`).
//
// Upstream file: `src/pane/agent_detection.rs`. Modifications to THIS file:
//   * `use crate::detect::{...}` -> `use crate::engine::{...}`, and
//     `crate::detect::X` -> `crate::engine::X` throughout.
//   * `pub(super)` -> `pub`: upstream this is a private child of `pane`; here
//     the consumer (Aleph's `gateway::runtime` sampler) is outside the crate.
//   * Everything except `detection_update_for_publish_with_osc` was CUT --
//     see below. Nothing was rewritten; each cut item is recoverable from
//     upstream by name.
//
// ---------------------------------------------------------------------------
// WHAT WAS CUT, AND WHY (Aleph task 5, 2026-09-02)
// ---------------------------------------------------------------------------
//
// Upstream drives detection from a poll loop (`AGENT_PENDING_IDLE_RECHECK`,
// 100 ms) that also maintains its own `detection_content_seq` counter. Aleph
// samples on the `pty.screen` DIFF FRAME: a frame exists exactly when the
// screen changed, and a quiet screen produces none. Every item below depended
// on one of those two clocks, or answered a publish question Aleph does not
// ask here. They were parked in task 2 with task 5 as the owner; task 5
// consumed one and cut the rest rather than leave the file at zero callers.
//
//   * `PendingIdleConfirmation` (+ `active`, `clear`,
//     `should_hold_working_to_idle`) and `AGENT_PENDING_IDLE_RECHECK` /
//     `_CONFIRMATIONS` / `_CAP`. The working -> plain-idle hold releases only
//     on a later recheck. Under the diff-frame cadence an agent that finishes
//     and goes quiet emits no further frame, so the hold would never be
//     confirmed and the entry would read Working forever -- a worse failure
//     than no hysteresis, and un-fixable without a second clock (判据 §12).
//   * `should_skip_idle_screen_scan`, `decide_detection_screen_read`,
//     `IdleScreenScanSkipInput`, `DetectionScreenReadInput`,
//     `DetectionScreenReadDecision`, `observe_detection_content_change`,
//     `mark_detection_content_changed`. All keyed on a `detection_content_seq`
//     that duplicates what the diff frame already signals. Two
//     content-changed signals are two orderings.
//   * `should_publish_detection_update`, `stable_visible_signal_refresh_due`,
//     `STABLE_VISIBLE_SIGNAL_REFRESH`, `decide_detection_transition`,
//     `decide_screen_detection_publish`, `DetectionPublishState`,
//     `DetectionTransitionInput`, `DetectionTransitionDecision`,
//     `ScreenDetectionPublishInput`, `DetectionPublishDecision`, and the
//     inlined identity `stabilize_agent_detection`. These decide whether to
//     PUBLISH an update, over four `visible_*` booleans that Aleph's
//     `RuntimeAgentEntry` does not carry. Aleph's `runtime.agents.changed`
//     (task 6) is a change of the stored entry -- a different question over a
//     different value.
//   * `AGENT_STARTUP_GRACE_WINDOW`: no reader, here or upstream in this file.
//   * `detection_update_for_publish`: an `#[allow(dead_code)]` shim that
//     existed only to call the function below with empty OSC strings. The
//     allow attribute was the tell (判据 §2).

use crate::engine::{Agent, AgentDetection, AgentState};

/// The pane-level entry point: run detection and say whether the result is
/// worth adopting.
///
/// Two arms, both reachable from Aleph's sampler
/// (`gateway::runtime::RuntimeAgents::sample`):
///
/// * `process_exited` -- a child that is gone is not working. Aleph passes
///   `PtySession::is_closed()`, which is true for a session killed but not
///   yet reaped by its reader thread.
/// * `skip_state_update` -- a manifest rule matched a region that means "the
///   screen is mid-repaint", so the detection says nothing about the agent.
///   `None` is "I have no statement", NOT "idle" (判据 §8); the caller keeps
///   whatever the previous frame established.
pub fn detection_update_for_publish_with_osc(
    agent: Option<Agent>,
    content: &str,
    osc_title: &str,
    osc_progress: &str,
    process_exited: bool,
) -> Option<AgentDetection> {
    if process_exited {
        return Some(AgentDetection {
            state: AgentState::Idle,
            skip_state_update: false,
            visible_idle: true,
            visible_blocker: false,
            visible_working: false,
        });
    }

    let detection = crate::engine::detect_agent_with_osc(agent, content, osc_title, osc_progress);
    (!detection.skip_state_update).then_some(detection)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exited arm must override the screen, and the live arm must not.
    /// Asserting only the exited arm would leave `process_exited` looking
    /// like a parameter that is always consulted and never decisive.
    #[test]
    fn an_exited_process_is_idle_regardless_of_what_the_screen_says() {
        let exited = detection_update_for_publish_with_osc(
            crate::identify_agent("claude"),
            "",
            "✳ Claude Code",
            "",
            true,
        )
        .expect("the exited arm always yields a detection");
        assert_eq!(exited.state, AgentState::Idle);
        assert!(exited.visible_idle);

        let live = detection_update_for_publish_with_osc(None, "", "", "", false)
            .expect("an unrecognised program still yields a detection");
        assert_eq!(live.state, AgentState::Unknown);
        assert!(!live.visible_idle);
    }
}
