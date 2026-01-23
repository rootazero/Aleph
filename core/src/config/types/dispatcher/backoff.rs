//! Backoff strategy configuration types
//!
//! Contains BackoffConfigToml for backoff strategy settings.

use serde::{Deserialize, Serialize};

// =============================================================================
// BackoffConfigToml - Backoff Strategy Configuration
// =============================================================================

/// Configuration for backoff strategy
///
/// Supported strategies:
/// - constant: Fixed delay between attempts
/// - exponential: Exponential backoff without jitter
/// - exponential_jitter: Exponential backoff with random jitter (recommended)
/// - rate_limit_aware: Respect Retry-After headers from rate limits
///
/// # Example TOML
///
/// ```toml
/// [model_router.retry.backoff]
/// strategy = "exponential_jitter"
/// initial_ms = 100
/// max_ms = 5000
/// multiplier = 2.0
/// jitter_factor = 0.2
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackoffConfigToml {
    /// Backoff strategy (default: "exponential_jitter")
    /// Options: "constant", "exponential", "exponential_jitter", "rate_limit_aware"
    #[serde(default = "default_backoff_strategy")]
    pub strategy: String,

    /// Initial delay in milliseconds (default: 100)
    #[serde(default = "default_backoff_initial")]
    pub initial_ms: u64,

    /// Maximum delay in milliseconds (default: 5000)
    #[serde(default = "default_backoff_max")]
    pub max_ms: u64,

    /// Multiplier for exponential backoff (default: 2.0)
    #[serde(default = "default_backoff_multiplier")]
    pub multiplier: f64,

    /// Jitter factor for exponential_jitter (0.0-1.0, default: 0.2)
    #[serde(default = "default_backoff_jitter_factor")]
    pub jitter_factor: f64,
}

fn default_backoff_strategy() -> String {
    "exponential_jitter".to_string()
}

fn default_backoff_initial() -> u64 {
    100 // 100ms
}

fn default_backoff_max() -> u64 {
    5000 // 5 seconds
}

fn default_backoff_multiplier() -> f64 {
    2.0
}

fn default_backoff_jitter_factor() -> f64 {
    0.2 // 20% jitter
}

impl Default for BackoffConfigToml {
    fn default() -> Self {
        Self {
            strategy: default_backoff_strategy(),
            initial_ms: default_backoff_initial(),
            max_ms: default_backoff_max(),
            multiplier: default_backoff_multiplier(),
            jitter_factor: default_backoff_jitter_factor(),
        }
    }
}

impl BackoffConfigToml {
    /// Validate the backoff configuration
    pub fn validate(&self) -> std::result::Result<(), String> {
        let valid_strategies = [
            "constant",
            "exponential",
            "exponential_jitter",
            "rate_limit_aware",
        ];
        if !valid_strategies.contains(&self.strategy.as_str()) {
            return Err(format!(
                "backoff.strategy must be one of {:?}, got '{}'",
                valid_strategies, self.strategy
            ));
        }

        if self.initial_ms == 0 {
            return Err("backoff.initial_ms must be > 0".to_string());
        }

        if self.max_ms < self.initial_ms {
            return Err("backoff.max_ms must be >= initial_ms".to_string());
        }

        if self.multiplier <= 0.0 {
            return Err("backoff.multiplier must be > 0".to_string());
        }

        if self.jitter_factor < 0.0 || self.jitter_factor > 1.0 {
            return Err("backoff.jitter_factor must be between 0.0 and 1.0".to_string());
        }

        Ok(())
    }

    /// Convert to internal BackoffStrategy
    pub fn to_backoff_strategy(&self) -> crate::dispatcher::model_router::BackoffStrategy {
        use crate::dispatcher::model_router::BackoffStrategy;

        match self.strategy.as_str() {
            "constant" => BackoffStrategy::Constant {
                delay_ms: self.initial_ms,
            },
            "exponential" => BackoffStrategy::Exponential {
                initial_ms: self.initial_ms,
                max_ms: self.max_ms,
                multiplier: self.multiplier,
            },
            "exponential_jitter" => BackoffStrategy::ExponentialJitter {
                initial_ms: self.initial_ms,
                max_ms: self.max_ms,
                jitter_factor: self.jitter_factor,
            },
            "rate_limit_aware" => BackoffStrategy::RateLimitAware {
                fallback_initial_ms: self.initial_ms,
                fallback_max_ms: self.max_ms,
            },
            _ => BackoffStrategy::ExponentialJitter {
                initial_ms: self.initial_ms,
                max_ms: self.max_ms,
                jitter_factor: self.jitter_factor,
            },
        }
    }
}
