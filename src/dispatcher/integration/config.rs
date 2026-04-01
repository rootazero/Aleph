//! Configuration for the Dispatcher integration

use crate::dispatcher::ConfirmationConfig;
use serde::{Deserialize, Serialize};

use super::ConfidenceThresholds;

/// Configuration for the Dispatcher integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatcherConfig {
    /// Whether the dispatcher is enabled
    pub enabled: bool,

    /// L3 routing configuration
    pub l3_enabled: bool,
    pub l3_timeout_ms: u64,
    pub l3_confidence_threshold: f32,

    /// Confirmation configuration
    pub confirmation: ConfirmationConfig,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            l3_enabled: true,
            l3_timeout_ms: 5000,
            l3_confidence_threshold: 0.3,
            confirmation: ConfirmationConfig::default(),
        }
    }
}

impl DispatcherConfig {
    /// Create a minimal config (L3 disabled, no confirmation)
    pub fn minimal() -> Self {
        Self {
            enabled: true,
            l3_enabled: false,
            l3_timeout_ms: 5000,
            l3_confidence_threshold: 0.3,
            confirmation: ConfirmationConfig::disabled(),
        }
    }

    /// Create a full config with all features
    pub fn full() -> Self {
        Self::default()
    }

    /// Get the confidence thresholds from this config
    pub fn confidence_thresholds(&self) -> ConfidenceThresholds {
        ConfidenceThresholds {
            no_match: self.l3_confidence_threshold,
            requires_confirmation: self.confirmation.threshold,
            auto_execute: 0.9,
        }
    }
}
