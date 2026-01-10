//! Dispatcher configuration types
//!
//! Contains Dispatcher Layer (Aether Cortex) configuration:
//! - DispatcherConfigToml: Multi-layer routing and confirmation settings

use serde::{Deserialize, Serialize};
use tracing::warn;

// =============================================================================
// DispatcherConfigToml
// =============================================================================

/// Configuration for the Dispatcher Layer (Aether Cortex)
///
/// The Dispatcher Layer provides intelligent tool routing through three layers:
/// - L1: Regex-based pattern matching (highest confidence)
/// - L2: Semantic keyword matching (medium confidence)
/// - L3: AI-powered inference (variable confidence)
///
/// When a tool match has low confidence, the system can show a confirmation
/// dialog to the user before execution.
///
/// # Example TOML
///
/// ```toml
/// [dispatcher]
/// enabled = true
/// l3_enabled = true
/// l3_timeout_ms = 5000
/// confirmation_threshold = 0.7
/// confirmation_timeout_ms = 30000
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatcherConfigToml {
    /// Whether the dispatcher is enabled (default: true)
    #[serde(default = "default_dispatcher_enabled")]
    pub enabled: bool,

    /// Whether L3 AI inference is enabled (default: true)
    #[serde(default = "default_dispatcher_l3_enabled")]
    pub l3_enabled: bool,

    /// L3 routing timeout in milliseconds (default: 5000)
    #[serde(default = "default_dispatcher_l3_timeout")]
    pub l3_timeout_ms: u64,

    /// Confidence threshold below which confirmation is required (0.0-1.0, default: 0.7)
    /// - Values >= 1.0 disable confirmation entirely
    /// - Values <= 0.0 always require confirmation
    #[serde(default = "default_dispatcher_confirmation_threshold")]
    pub confirmation_threshold: f32,

    /// Confirmation dialog timeout in milliseconds (default: 30000)
    #[serde(default = "default_dispatcher_confirmation_timeout")]
    pub confirmation_timeout_ms: u64,

    /// Whether confirmation dialogs are enabled (default: true)
    #[serde(default = "default_dispatcher_confirmation_enabled")]
    pub confirmation_enabled: bool,
}

pub fn default_dispatcher_enabled() -> bool {
    true
}

pub fn default_dispatcher_l3_enabled() -> bool {
    true
}

pub fn default_dispatcher_l3_timeout() -> u64 {
    5000 // 5 seconds
}

pub fn default_dispatcher_confirmation_threshold() -> f32 {
    0.7 // Require confirmation if confidence < 70%
}

pub fn default_dispatcher_confirmation_timeout() -> u64 {
    30000 // 30 seconds
}

pub fn default_dispatcher_confirmation_enabled() -> bool {
    true
}

impl Default for DispatcherConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_dispatcher_enabled(),
            l3_enabled: default_dispatcher_l3_enabled(),
            l3_timeout_ms: default_dispatcher_l3_timeout(),
            confirmation_threshold: default_dispatcher_confirmation_threshold(),
            confirmation_timeout_ms: default_dispatcher_confirmation_timeout(),
            confirmation_enabled: default_dispatcher_confirmation_enabled(),
        }
    }
}

impl DispatcherConfigToml {
    /// Validate the configuration values
    ///
    /// # Returns
    /// * `Ok(())` - Configuration is valid
    /// * `Err(String)` - Validation error message
    pub fn validate(&self) -> std::result::Result<(), String> {
        // Validate confirmation threshold range
        if self.confirmation_threshold < 0.0 {
            return Err(format!(
                "confirmation_threshold must be >= 0.0, got {}",
                self.confirmation_threshold
            ));
        }
        if self.confirmation_threshold > 1.0 {
            warn!(
                threshold = self.confirmation_threshold,
                "confirmation_threshold > 1.0 will disable confirmation entirely"
            );
        }

        // Validate L3 timeout
        if self.l3_timeout_ms == 0 {
            return Err("l3_timeout_ms must be > 0".to_string());
        }
        if self.l3_timeout_ms > 60000 {
            warn!(
                timeout = self.l3_timeout_ms,
                "l3_timeout_ms > 60000ms may cause poor user experience"
            );
        }

        // Validate confirmation timeout
        if self.confirmation_timeout_ms == 0 {
            return Err("confirmation_timeout_ms must be > 0".to_string());
        }

        Ok(())
    }

    /// Convert to internal DispatcherConfig
    pub fn to_dispatcher_config(&self) -> crate::dispatcher::DispatcherConfig {
        use crate::dispatcher::{ConfirmationConfig, DispatcherConfig};

        DispatcherConfig {
            enabled: self.enabled,
            l3_enabled: self.l3_enabled,
            l3_timeout_ms: self.l3_timeout_ms,
            l3_confidence_threshold: self.confirmation_threshold,
            confirmation: ConfirmationConfig {
                enabled: self.confirmation_enabled,
                threshold: self.confirmation_threshold,
                timeout_ms: self.confirmation_timeout_ms,
                show_parameters: true,
                skip_native_tools: false,
            },
        }
    }
}
