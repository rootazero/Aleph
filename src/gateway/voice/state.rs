//! Per-channel voice mode state.
//!
//! Sister files (see `super::mod` for the canonical cross-reference table):
//! - `voice_mode.rs` — session-keyed `VoiceTurnState` (different concept, confusingly similar name).
//! - `voice_mode_set.rs` (in `builtin_tools/voice_tools/`) — the LLM tool that mutates THIS struct.
//! - `thinker::layers::voice_mode.rs` — the prompt-layer that reads the session registry, not this one.
//!
//! Tracks whether voice output is enabled for a channel, which provider/voice
//! to use, and consecutive failure count for auto-disable logic.

use serde::{Deserialize, Serialize};

/// Per-channel voice mode state.
///
/// Tracks whether voice output is enabled for a channel, which provider/voice
/// to use, and consecutive failure count for auto-disable logic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VoiceState {
    pub enabled: bool,
    pub provider: Option<String>,
    pub voice: Option<String>,
    pub consecutive_failures: u8,
}

impl VoiceState {
    /// Returns true when voice mode is explicitly enabled.
    /// Provider resolution happens at TTS generation time (auto-detect fallback).
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.enabled
    }

    /// Record a successful TTS synthesis — resets the failure counter.
    pub const fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Record a failed TTS synthesis.
    ///
    /// Returns `true` if the channel has been auto-disabled (3rd consecutive
    /// failure), `false` otherwise.
    pub const fn record_failure(&mut self) -> bool {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= 3 {
            self.enabled = false;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        let state = VoiceState::default();
        assert!(!state.enabled);
        assert!(!state.is_active());
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn record_success_resets_failures() {
        let mut state = VoiceState {
            enabled: true,
            provider: None,
            voice: None,
            consecutive_failures: 2,
        };
        state.record_success();
        assert_eq!(state.consecutive_failures, 0);
        assert!(state.enabled);
    }

    #[test]
    fn auto_disable_after_three_failures() {
        let mut state = VoiceState {
            enabled: true,
            provider: None,
            voice: None,
            consecutive_failures: 0,
        };

        let disabled_on_first = state.record_failure();
        assert!(!disabled_on_first);
        assert!(state.enabled);
        assert_eq!(state.consecutive_failures, 1);

        let disabled_on_second = state.record_failure();
        assert!(!disabled_on_second);
        assert!(state.enabled);
        assert_eq!(state.consecutive_failures, 2);

        let disabled_on_third = state.record_failure();
        assert!(disabled_on_third, "should auto-disable on 3rd failure");
        assert!(!state.enabled, "should be disabled after 3 failures");
        assert_eq!(state.consecutive_failures, 3);
    }

    // -----------------------------------------------------------------------
    // Failure-counter contract pin (2026-08-16 round)
    //
    // The caller in `reply_emitter/helpers::send_as_voice` calls `record_failure`
    // once per failed `generate_tts` call, not once per provider hop within the
    // call. A single `generate_tts` may try primary + 1 fallback; if both fail
    // the call returns `None` and the caller increments exactly once. These
    // tests pin both halves of that contract.
    // -----------------------------------------------------------------------

    #[test]
    fn three_failures_disables_once_no_per_hop_counting() {
        // Contract: 3 consecutive failures → auto-disable, regardless of how
        // many provider hops happened within the call. The counter is
        // incremented by the CALLER (`send_as_voice`), not by the TTS path.
        let mut state = VoiceState { enabled: true, ..Default::default() };
        assert!(!state.record_failure());
        assert!(state.enabled);
        assert!(!state.record_failure());
        assert!(state.enabled);
        let disabled = state.record_failure();
        assert!(disabled, "the 3rd failure must flip disabled");
        assert!(!state.enabled);
        // After disable, more failures keep the channel disabled (the
        // disabled flag is a one-way latch). The counter keeps growing via
        // saturating_add — neither a successful synthesis nor further
        // failures may re-enable the channel without an explicit user toggle.
        let _ = state.record_failure();
        assert!(!state.enabled, "channel must stay disabled after auto-disable");
        // record_success on a disabled channel resets the counter but does
        // not re-enable — the user must explicitly re-enable (the LLM tool
        // `voice_mode_set` is the only re-enable path).
        state.record_success();
        assert!(!state.enabled, "record_success must not re-enable a disabled channel");
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn success_resets_failure_counter() {
        let mut state = VoiceState {
            enabled: true,
            consecutive_failures: 2,
            ..Default::default()
        };
        state.record_success();
        assert_eq!(state.consecutive_failures, 0);
        assert!(state.enabled);
    }

    #[test]
    fn failure_counter_is_per_channel_state() {
        // Two channel states must be independent — a failure on channel A
        // must never leak into channel B's counter.
        let mut a = VoiceState { enabled: true, ..Default::default() };
        let mut b = VoiceState { enabled: true, ..Default::default() };
        a.record_failure();
        a.record_failure();
        assert_eq!(a.consecutive_failures, 2);
        assert_eq!(b.consecutive_failures, 0);
        b.record_success();
        assert_eq!(a.consecutive_failures, 2);
        assert_eq!(b.consecutive_failures, 0);
    }
}
