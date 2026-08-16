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
}
