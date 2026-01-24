//! Metrics collection configuration
//!
//! Contains MetricsConfigToml, TimeWindowsConfigToml, and ScoringConfigToml
//! for configuring runtime metrics collection for intelligent model routing.

use serde::{Deserialize, Serialize};

use crate::dispatcher::model_router::ScoringConfig;

// =============================================================================
// MetricsConfigToml
// =============================================================================

/// Metrics collection configuration from TOML
///
/// Configures runtime metrics collection for intelligent model routing.
///
/// # Example TOML
/// ```toml
/// [cowork.model_routing.metrics]
/// enabled = true
/// buffer_size = 10000
/// aggregation_interval_secs = 60
/// flush_interval_secs = 300
/// db_path = "~/.aether/metrics.db"
/// exploration_rate = 0.05
///
/// [cowork.model_routing.metrics.windows]
/// short_term_secs = 300
/// medium_term_secs = 3600
/// long_term_secs = 86400
///
/// [cowork.model_routing.metrics.scoring]
/// latency_weight = 0.25
/// cost_weight = 0.25
/// reliability_weight = 0.35
/// quality_weight = 0.15
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfigToml {
    /// Enable metrics collection
    #[serde(default = "default_metrics_enabled")]
    pub enabled: bool,

    /// Ring buffer size for call records
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,

    /// Interval for aggregating metrics (seconds)
    #[serde(default = "default_aggregation_interval")]
    pub aggregation_interval_secs: u64,

    /// Interval for flushing to persistent storage (seconds)
    #[serde(default = "default_flush_interval")]
    pub flush_interval_secs: u64,

    /// Path to SQLite database for persistence
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_path: Option<String>,

    /// Exploration rate for epsilon-greedy routing (0.0-1.0)
    #[serde(default = "default_exploration_rate")]
    pub exploration_rate: f64,

    /// Time window configuration
    #[serde(default)]
    pub windows: TimeWindowsConfigToml,

    /// Scoring configuration
    #[serde(default)]
    pub scoring: ScoringConfigToml,
}

impl Default for MetricsConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_metrics_enabled(),
            buffer_size: default_buffer_size(),
            aggregation_interval_secs: default_aggregation_interval(),
            flush_interval_secs: default_flush_interval(),
            db_path: None,
            exploration_rate: default_exploration_rate(),
            windows: TimeWindowsConfigToml::default(),
            scoring: ScoringConfigToml::default(),
        }
    }
}

impl MetricsConfigToml {
    /// Convert to MetricsConfig for the collector
    pub fn to_metrics_config(&self) -> crate::dispatcher::model_router::MetricsConfig {
        use crate::dispatcher::model_router::WindowConfig;

        crate::dispatcher::model_router::MetricsConfig {
            buffer_size: self.buffer_size,
            aggregation_interval: std::time::Duration::from_secs(self.aggregation_interval_secs),
            flush_interval: std::time::Duration::from_secs(self.flush_interval_secs),
            window_config: WindowConfig {
                short_term: std::time::Duration::from_secs(self.windows.short_term_secs),
                medium_term: std::time::Duration::from_secs(self.windows.medium_term_secs),
                long_term: std::time::Duration::from_secs(self.windows.long_term_secs),
            },
            persist_enabled: self.db_path.is_some(),
        }
    }

    /// Convert to ScoringConfig for the scorer
    pub fn to_scoring_config(&self) -> ScoringConfig {
        self.scoring.to_scoring_config()
    }

    /// Validate metrics configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.buffer_size == 0 {
            return Err(
                "agent.model_routing.metrics.buffer_size must be greater than 0".to_string(),
            );
        }

        if self.exploration_rate < 0.0 || self.exploration_rate > 1.0 {
            return Err(format!(
                "agent.model_routing.metrics.exploration_rate must be between 0.0 and 1.0, got {}",
                self.exploration_rate
            ));
        }

        self.scoring.validate()?;

        Ok(())
    }
}

// =============================================================================
// TimeWindowsConfigToml
// =============================================================================

/// Time windows configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeWindowsConfigToml {
    /// Short-term window in seconds (default 5 minutes)
    #[serde(default = "default_short_term_secs")]
    pub short_term_secs: u64,

    /// Medium-term window in seconds (default 1 hour)
    #[serde(default = "default_medium_term_secs")]
    pub medium_term_secs: u64,

    /// Long-term window in seconds (default 24 hours)
    #[serde(default = "default_long_term_secs")]
    pub long_term_secs: u64,
}

impl Default for TimeWindowsConfigToml {
    fn default() -> Self {
        Self {
            short_term_secs: default_short_term_secs(),
            medium_term_secs: default_medium_term_secs(),
            long_term_secs: default_long_term_secs(),
        }
    }
}

// =============================================================================
// ScoringConfigToml
// =============================================================================

/// Scoring weights configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringConfigToml {
    /// Weight for latency score (0.0-1.0)
    #[serde(default = "default_latency_weight")]
    pub latency_weight: f64,

    /// Weight for cost score (0.0-1.0)
    #[serde(default = "default_cost_weight")]
    pub cost_weight: f64,

    /// Weight for reliability score (0.0-1.0)
    #[serde(default = "default_reliability_weight")]
    pub reliability_weight: f64,

    /// Weight for quality score (0.0-1.0)
    #[serde(default = "default_quality_weight")]
    pub quality_weight: f64,

    /// Target latency in milliseconds for scoring
    #[serde(default = "default_latency_target_ms")]
    pub latency_target_ms: u64,

    /// Maximum acceptable latency in milliseconds
    #[serde(default = "default_latency_max_ms")]
    pub latency_max_ms: u64,

    /// Minimum success rate for full reliability score
    #[serde(default = "default_min_success_rate")]
    pub min_success_rate: f64,
}

impl Default for ScoringConfigToml {
    fn default() -> Self {
        Self {
            latency_weight: default_latency_weight(),
            cost_weight: default_cost_weight(),
            reliability_weight: default_reliability_weight(),
            quality_weight: default_quality_weight(),
            latency_target_ms: default_latency_target_ms(),
            latency_max_ms: default_latency_max_ms(),
            min_success_rate: default_min_success_rate(),
        }
    }
}

impl ScoringConfigToml {
    /// Convert to ScoringConfig
    pub fn to_scoring_config(&self) -> ScoringConfig {
        ScoringConfig {
            latency_weight: self.latency_weight,
            cost_weight: self.cost_weight,
            reliability_weight: self.reliability_weight,
            quality_weight: self.quality_weight,
            latency_target_ms: self.latency_target_ms as f64,
            latency_max_ms: self.latency_max_ms as f64,
            min_success_rate: self.min_success_rate,
            degradation_threshold: 3, // Default
            min_samples: 10,          // Default
        }
    }

    /// Validate scoring configuration
    pub fn validate(&self) -> Result<(), String> {
        let total =
            self.latency_weight + self.cost_weight + self.reliability_weight + self.quality_weight;
        if (total - 1.0).abs() > 0.01 {
            tracing::warn!(
                total = total,
                "Scoring weights do not sum to 1.0, they will be normalized"
            );
        }

        if self.latency_target_ms >= self.latency_max_ms {
            return Err(format!(
                "latency_target_ms ({}) must be less than latency_max_ms ({})",
                self.latency_target_ms, self.latency_max_ms
            ));
        }

        if self.min_success_rate < 0.0 || self.min_success_rate > 1.0 {
            return Err(format!(
                "min_success_rate must be between 0.0 and 1.0, got {}",
                self.min_success_rate
            ));
        }

        Ok(())
    }
}

// =============================================================================
// Default Functions
// =============================================================================

fn default_metrics_enabled() -> bool {
    true
}

fn default_buffer_size() -> usize {
    10000
}

fn default_aggregation_interval() -> u64 {
    60
}

fn default_flush_interval() -> u64 {
    300
}

fn default_exploration_rate() -> f64 {
    0.05
}

fn default_short_term_secs() -> u64 {
    300 // 5 minutes
}

fn default_medium_term_secs() -> u64 {
    3600 // 1 hour
}

fn default_long_term_secs() -> u64 {
    86400 // 24 hours
}

fn default_latency_weight() -> f64 {
    0.25
}

fn default_cost_weight() -> f64 {
    0.25
}

fn default_reliability_weight() -> f64 {
    0.35
}

fn default_quality_weight() -> f64 {
    0.15
}

fn default_latency_target_ms() -> u64 {
    2000
}

fn default_latency_max_ms() -> u64 {
    30000
}

fn default_min_success_rate() -> f64 {
    0.9
}
