//! Hybrid Compression Trigger
//!
//! Combines signal-based smart triggering with token threshold safety net.

use super::signal_detector::CompressionSignal;
use serde::{Deserialize, Serialize};

/// Configuration for compression triggering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// Maximum token window size
    pub max_token_window: usize,
    /// Trigger threshold as fraction (0.9 = 90%)
    pub trigger_threshold: f32,
    /// Target after compression as fraction (0.5 = 50%)
    pub target_after_compression: f32,
    /// Whether signal detection is enabled
    pub signal_detection_enabled: bool,
    /// Recent turns to keep during aggressive compression
    pub keep_recent_turns: usize,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            max_token_window: 128_000,
            trigger_threshold: 0.9,
            target_after_compression: 0.5,
            signal_detection_enabled: true,
            keep_recent_turns: 5,
        }
    }
}

/// Reason for triggering compression
#[derive(Debug, Clone)]
pub enum TriggerReason {
    /// Triggered by signal detection
    Signal(CompressionSignal),
    /// Triggered by token threshold
    TokenThreshold {
        current: usize,
        max: usize,
    },
    /// Both signal and threshold
    Both {
        signal: CompressionSignal,
        tokens: usize,
    },
}

impl TriggerReason {
    /// Check if this was a safety net trigger (not signal-driven)
    pub fn is_safety_net(&self) -> bool {
        matches!(self, TriggerReason::TokenThreshold { .. })
    }

    /// Get compression aggressiveness
    pub fn aggressiveness(&self) -> CompressionAggressiveness {
        match self {
            TriggerReason::Signal(CompressionSignal::Milestone { .. }) => {
                CompressionAggressiveness::Full
            }
            TriggerReason::Signal(CompressionSignal::ContextSwitch { .. }) => {
                CompressionAggressiveness::TopicOnly
            }
            TriggerReason::TokenThreshold { .. } => {
                CompressionAggressiveness::Aggressive
            }
            TriggerReason::Both { .. } => {
                CompressionAggressiveness::Full
            }
            _ => CompressionAggressiveness::Normal,
        }
    }
}

/// How aggressively to compress
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionAggressiveness {
    /// Normal compression with semantic boundaries
    Normal,
    /// Full compression + archive scratchpad
    Full,
    /// Only compress old topic
    TopicOnly,
    /// Emergency: keep only recent N turns
    Aggressive,
}

/// Hybrid compression trigger
pub struct HybridTrigger {
    config: TriggerConfig,
}

impl HybridTrigger {
    /// Create a new trigger with configuration
    pub fn new(config: TriggerConfig) -> Self {
        Self { config }
    }

    /// Check if compression should be triggered
    pub fn check(
        &self,
        signal: Option<CompressionSignal>,
        current_tokens: usize,
    ) -> Option<TriggerReason> {
        self.check_signal(signal, current_tokens, self.config.max_token_window)
    }

    /// Check with explicit max tokens
    pub fn check_signal(
        &self,
        signal: Option<CompressionSignal>,
        current_tokens: usize,
        max_tokens: usize,
    ) -> Option<TriggerReason> {
        let threshold = (max_tokens as f32 * self.config.trigger_threshold) as usize;
        let over_threshold = current_tokens > threshold;

        match (signal, over_threshold) {
            (Some(s), true) => Some(TriggerReason::Both {
                signal: s,
                tokens: current_tokens,
            }),
            (Some(s), false) if self.config.signal_detection_enabled => {
                Some(TriggerReason::Signal(s))
            }
            (None, true) => Some(TriggerReason::TokenThreshold {
                current: current_tokens,
                max: threshold,
            }),
            _ => None,
        }
    }

    /// Check tokens only (bypass signal detection)
    pub fn check_tokens(&self, current_tokens: usize, max_tokens: usize) -> Option<TriggerReason> {
        let threshold = (max_tokens as f32 * self.config.trigger_threshold) as usize;

        if current_tokens > threshold {
            Some(TriggerReason::TokenThreshold {
                current: current_tokens,
                max: threshold,
            })
        } else {
            None
        }
    }

    /// Get configuration
    pub fn config(&self) -> &TriggerConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_threshold_trigger() {
        let config = TriggerConfig::default();
        let trigger = HybridTrigger::new(config.clone());

        let result = trigger.check_tokens(100_000, 128_000);
        assert!(result.is_none());

        let result = trigger.check_tokens(120_000, 128_000);
        assert!(matches!(result, Some(TriggerReason::TokenThreshold { .. })));
    }

    #[test]
    fn test_signal_trigger() {
        let config = TriggerConfig::default();
        let trigger = HybridTrigger::new(config);

        let signal = CompressionSignal::Milestone {
            task_description: "Build auth".to_string(),
            completion_indicator: "done".to_string(),
        };

        let result = trigger.check_signal(Some(signal.clone()), 50_000, 128_000);
        assert!(matches!(result, Some(TriggerReason::Signal(_))));
    }

    #[test]
    fn test_both_trigger() {
        let config = TriggerConfig::default();
        let trigger = HybridTrigger::new(config);

        let signal = CompressionSignal::Milestone {
            task_description: "Build auth".to_string(),
            completion_indicator: "done".to_string(),
        };

        let result = trigger.check_signal(Some(signal), 120_000, 128_000);
        assert!(matches!(result, Some(TriggerReason::Both { .. })));
    }

    #[test]
    fn test_aggressiveness() {
        let milestone = TriggerReason::Signal(CompressionSignal::Milestone {
            task_description: "test".to_string(),
            completion_indicator: "done".to_string(),
        });
        assert_eq!(milestone.aggressiveness(), CompressionAggressiveness::Full);

        let threshold = TriggerReason::TokenThreshold { current: 100, max: 90 };
        assert_eq!(threshold.aggressiveness(), CompressionAggressiveness::Aggressive);
    }

    #[test]
    fn test_is_safety_net() {
        let threshold = TriggerReason::TokenThreshold { current: 100, max: 90 };
        assert!(threshold.is_safety_net());

        let signal = TriggerReason::Signal(CompressionSignal::Milestone {
            task_description: "test".to_string(),
            completion_indicator: "done".to_string(),
        });
        assert!(!signal.is_safety_net());
    }
}
