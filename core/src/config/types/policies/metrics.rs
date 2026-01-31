//! Performance metrics policies
//!
//! Configurable performance targets for monitoring and alerting
//! across different pipeline stages.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Policy for performance monitoring and alerting
///
/// Defines target latencies for each pipeline stage and the threshold
/// multiplier for triggering warnings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MetricsPolicy {
    /// Target latency: hotkey press to clipboard read (ms)
    /// Default: 50
    #[serde(default = "default_hotkey_to_clipboard_ms")]
    pub target_hotkey_to_clipboard_ms: u64,

    /// Target latency: clipboard copy to memory store (ms)
    /// Default: 100
    #[serde(default = "default_clipboard_to_memory_ms")]
    pub target_clipboard_to_memory_ms: u64,

    /// Target latency: memory retrieval to AI request (ms)
    /// Default: 500
    #[serde(default = "default_memory_to_ai_ms")]
    pub target_memory_to_ai_ms: u64,

    /// Target latency: AI response to paste insertion (ms)
    /// Default: 50
    #[serde(default = "default_ai_to_paste_ms")]
    pub target_ai_to_paste_ms: u64,

    /// Target latency: paste to final completion (ms)
    /// Default: 100
    #[serde(default = "default_paste_to_complete_ms")]
    pub target_paste_to_complete_ms: u64,

    /// Warning threshold multiplier
    /// Operations exceeding target * multiplier trigger warnings
    /// Default: 2.0
    #[serde(default = "default_warning_multiplier")]
    pub warning_multiplier: f64,

    /// Enable performance logging
    /// Default: true
    #[serde(default = "default_enable_logging")]
    pub enable_logging: bool,

    /// Enable performance warnings
    /// Default: true
    #[serde(default = "default_enable_warnings")]
    pub enable_warnings: bool,
}

impl Default for MetricsPolicy {
    fn default() -> Self {
        Self {
            target_hotkey_to_clipboard_ms: default_hotkey_to_clipboard_ms(),
            target_clipboard_to_memory_ms: default_clipboard_to_memory_ms(),
            target_memory_to_ai_ms: default_memory_to_ai_ms(),
            target_ai_to_paste_ms: default_ai_to_paste_ms(),
            target_paste_to_complete_ms: default_paste_to_complete_ms(),
            warning_multiplier: default_warning_multiplier(),
            enable_logging: default_enable_logging(),
            enable_warnings: default_enable_warnings(),
        }
    }
}

fn default_hotkey_to_clipboard_ms() -> u64 {
    50
}

fn default_clipboard_to_memory_ms() -> u64 {
    100
}

fn default_memory_to_ai_ms() -> u64 {
    500
}

fn default_ai_to_paste_ms() -> u64 {
    50
}

fn default_paste_to_complete_ms() -> u64 {
    100
}

fn default_warning_multiplier() -> f64 {
    2.0
}

fn default_enable_logging() -> bool {
    true
}

fn default_enable_warnings() -> bool {
    true
}

impl MetricsPolicy {
    /// Get the warning threshold for a given target in milliseconds
    pub fn warning_threshold_ms(&self, target_ms: u64) -> u64 {
        (target_ms as f64 * self.warning_multiplier) as u64
    }

    /// Check if a duration exceeds the warning threshold for hotkey->clipboard
    pub fn is_hotkey_to_clipboard_slow(&self, duration_ms: u64) -> bool {
        duration_ms > self.warning_threshold_ms(self.target_hotkey_to_clipboard_ms)
    }

    /// Check if a duration exceeds the warning threshold for clipboard->memory
    pub fn is_clipboard_to_memory_slow(&self, duration_ms: u64) -> bool {
        duration_ms > self.warning_threshold_ms(self.target_clipboard_to_memory_ms)
    }

    /// Check if a duration exceeds the warning threshold for memory->AI
    pub fn is_memory_to_ai_slow(&self, duration_ms: u64) -> bool {
        duration_ms > self.warning_threshold_ms(self.target_memory_to_ai_ms)
    }

    /// Check if a duration exceeds the warning threshold for AI->paste
    pub fn is_ai_to_paste_slow(&self, duration_ms: u64) -> bool {
        duration_ms > self.warning_threshold_ms(self.target_ai_to_paste_ms)
    }

    /// Check if a duration exceeds the warning threshold for paste->complete
    pub fn is_paste_to_complete_slow(&self, duration_ms: u64) -> bool {
        duration_ms > self.warning_threshold_ms(self.target_paste_to_complete_ms)
    }

    /// Get total target latency for the full pipeline
    pub fn total_target_ms(&self) -> u64 {
        self.target_hotkey_to_clipboard_ms
            + self.target_clipboard_to_memory_ms
            + self.target_memory_to_ai_ms
            + self.target_ai_to_paste_ms
            + self.target_paste_to_complete_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let policy = MetricsPolicy::default();
        assert_eq!(policy.target_hotkey_to_clipboard_ms, 50);
        assert_eq!(policy.target_clipboard_to_memory_ms, 100);
        assert_eq!(policy.target_memory_to_ai_ms, 500);
        assert_eq!(policy.target_ai_to_paste_ms, 50);
        assert_eq!(policy.target_paste_to_complete_ms, 100);
        assert_eq!(policy.warning_multiplier, 2.0);
    }

    #[test]
    fn test_warning_threshold() {
        let policy = MetricsPolicy::default();
        // With 2.0 multiplier, 50ms target has 100ms threshold
        assert_eq!(policy.warning_threshold_ms(50), 100);
        assert_eq!(policy.warning_threshold_ms(100), 200);
    }

    #[test]
    fn test_slowness_detection() {
        let policy = MetricsPolicy::default();
        // Hotkey->clipboard: 50ms target, 100ms threshold
        assert!(!policy.is_hotkey_to_clipboard_slow(50)); // At target
        assert!(!policy.is_hotkey_to_clipboard_slow(100)); // At threshold
        assert!(policy.is_hotkey_to_clipboard_slow(101)); // Over threshold
    }

    #[test]
    fn test_total_target() {
        let policy = MetricsPolicy::default();
        // 50 + 100 + 500 + 50 + 100 = 800ms
        assert_eq!(policy.total_target_ms(), 800);
    }

    #[test]
    fn test_partial_deserialization() {
        let toml = r#"
            target_hotkey_to_clipboard_ms = 30
            warning_multiplier = 1.5
        "#;
        let policy: MetricsPolicy = toml::from_str(toml).unwrap();
        assert_eq!(policy.target_hotkey_to_clipboard_ms, 30);
        assert_eq!(policy.warning_multiplier, 1.5);
        // Defaults for unspecified
        assert_eq!(policy.target_memory_to_ai_ms, 500);
        assert!(policy.enable_logging);
    }
}
