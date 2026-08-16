//! Per-model robustness profile — tunes the tool-loop watchdog thresholds to
//! the active model family. Resolved per run at the orchestrator layer (where
//! the model is known) and threaded into `TurnVerifyContext`. The harness loop
//! only *reads* it, never decides with it (R10-safe).

use crate::verification::turn_verifier::TOOL_HISTORY_WINDOW;

/// Tunable thresholds for `ToolLoopVerifier`, keyed off the active model's
/// behavior family.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelRobustnessProfile {
    /// Tier-1 veto threshold: identical (name+args) consecutive calls.
    pub repeat_threshold: usize,
    /// Hard-halt threshold for the Tier-1 identical run (within the window).
    pub halt_threshold: usize,
    /// Max consecutive steers (vetoes) before the harness forces a wrap-up
    /// grace turn. Replaces the old global `MAX_VERIFIER_VETOS` const.
    pub steer_max: usize,
    /// Tier-2 fires only when window distinctness < this ratio (0.0..=1.0).
    /// Lower = more tolerant of fan-out before flagging a thrash.
    pub novelty_min: f32,
    /// Tier-2 requires the turn to carry no narration text.
    pub silence_required: bool,
}

impl ModelRobustnessProfile {
    /// Conservative default — byte-compatible with pre-change behavior.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            repeat_threshold: 5,
            halt_threshold: TOOL_HISTORY_WINDOW,
            steer_max: 10,
            novelty_min: 0.5,
            silence_required: true,
        }
    }

    /// Resolve a profile by model-behavior name. The behavior is determined
    /// by `orchestrator::harness_bridge::behavior_resolve::resolve_behavior`,
    /// which folds the prompt layer's `protocol_to_behavior` /
    /// `model_behavior_override` together with `provider.behavior_hint()`
    /// (vendor self-identification maps weak-vendor models to `"strict"`).
    #[must_use]
    pub fn for_behavior(name: Option<&str>) -> Self {
        match name {
            // Strong instruction-followers: loose — they rarely loop.
            Some("anthropic") => Self {
                repeat_threshold: 5,
                halt_threshold: TOOL_HISTORY_WINDOW,
                steer_max: 12,
                novelty_min: 0.35,
                silence_required: true,
            },
            // Weak / local models: tight — steer earlier, fewer chances.
            Some("ollama") => Self {
                repeat_threshold: 3,
                halt_threshold: TOOL_HISTORY_WINDOW,
                steer_max: 6,
                novelty_min: 0.6,
                silence_required: false,
            },
            // Open-weight / weaker instruction-followers (Kimi, Minimax,
            // DeepSeek, Qwen, GLM) self-identified by `vendor_identity`.
            // Tightest leash: steer early, few chances, tolerate little thrash.
            Some("strict") => Self {
                repeat_threshold: 3,
                halt_threshold: TOOL_HISTORY_WINDOW,
                steer_max: 5,
                novelty_min: 0.6,
                silence_required: false,
            },
            // openai / gemini / unknown: conservative default.
            _ => Self::conservative(),
        }
    }

    /// Clamp to the ring-buffer window invariants so a misconfigured profile
    /// can never silently disable detection.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.repeat_threshold = self.repeat_threshold.clamp(2, TOOL_HISTORY_WINDOW);
        self.halt_threshold = self
            .halt_threshold
            .clamp(self.repeat_threshold, TOOL_HISTORY_WINDOW);
        self.steer_max = self.steer_max.max(1);
        self.novelty_min = self.novelty_min.clamp(0.0, 1.0);
        self
    }
}

impl Default for ModelRobustnessProfile {
    fn default() -> Self {
        Self::conservative()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_matches_legacy_behavior() {
        let p = ModelRobustnessProfile::conservative();
        assert_eq!(p.repeat_threshold, 5);
        assert_eq!(p.halt_threshold, 8); // TOOL_HISTORY_WINDOW
        assert_eq!(p.steer_max, 10);
        assert!(p.silence_required);
    }

    #[test]
    fn for_behavior_anthropic_is_loose() {
        let p = ModelRobustnessProfile::for_behavior(Some("anthropic"));
        assert!(p.steer_max >= ModelRobustnessProfile::conservative().steer_max);
        assert!(p.novelty_min <= ModelRobustnessProfile::conservative().novelty_min);
    }

    #[test]
    fn for_behavior_ollama_is_tight() {
        let p = ModelRobustnessProfile::for_behavior(Some("ollama"));
        assert!(p.repeat_threshold < ModelRobustnessProfile::conservative().repeat_threshold);
        assert!(p.steer_max < ModelRobustnessProfile::conservative().steer_max);
    }

    #[test]
    fn for_behavior_unknown_is_conservative() {
        assert_eq!(
            ModelRobustnessProfile::for_behavior(None),
            ModelRobustnessProfile::conservative()
        );
        assert_eq!(
            ModelRobustnessProfile::for_behavior(Some("mystery-model")),
            ModelRobustnessProfile::conservative()
        );
    }

    #[test]
    fn clamped_enforces_window_invariants() {
        let bad = ModelRobustnessProfile {
            repeat_threshold: 99,
            halt_threshold: 1,
            steer_max: 0,
            novelty_min: 5.0,
            silence_required: true,
        }
        .clamped();
        assert!(bad.repeat_threshold >= 2 && bad.repeat_threshold <= 8);
        assert!(bad.halt_threshold >= bad.repeat_threshold && bad.halt_threshold <= 8);
        assert!(bad.steer_max >= 1);
        assert!(bad.novelty_min >= 0.0 && bad.novelty_min <= 1.0);
    }

    #[test]
    fn for_behavior_strict_is_tightest() {
        let strict = ModelRobustnessProfile::for_behavior(Some("strict"));
        let ollama = ModelRobustnessProfile::for_behavior(Some("ollama"));
        assert!(strict.repeat_threshold <= ollama.repeat_threshold);
        assert!(strict.steer_max <= ollama.steer_max);
        assert!(strict.novelty_min >= ollama.novelty_min);
        assert!(!strict.silence_required);
    }
}
