//! Health check system configuration
//!
//! Contains HealthConfigToml, CircuitBreakerConfigToml, and ProbeConfigToml
//! for configuring model health monitoring and circuit breaker behavior.

use serde::{Deserialize, Serialize};

use crate::dispatcher::model_router::{CircuitBreakerConfig, HealthConfig, ProbeConfig};

// =============================================================================
// HealthConfigToml
// =============================================================================

/// Health check system configuration from TOML
///
/// Configures model health monitoring and circuit breaker behavior.
///
/// # Example TOML
/// ```toml
/// [cowork.model_routing.health]
/// enabled = true
/// active_probing = true
/// failure_threshold = 3
/// recovery_successes = 2
/// latency_degradation_threshold_ms = 10000
/// latency_healthy_threshold_ms = 5000
/// rate_limit_warning_threshold = 0.2
///
/// [cowork.model_routing.health.circuit_breaker]
/// failure_threshold = 5
/// window_secs = 60
/// cooldown_secs = 30
/// half_open_successes = 2
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfigToml {
    /// Enable health tracking
    #[serde(default = "default_health_enabled")]
    pub enabled: bool,

    /// Enable active probing of unhealthy models
    #[serde(default = "default_active_probing")]
    pub active_probing: bool,

    /// Number of consecutive failures to mark unhealthy
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,

    /// Number of successes to recover from unhealthy
    #[serde(default = "default_recovery_successes")]
    pub recovery_successes: u32,

    /// Number of successes to recover from degraded
    #[serde(default = "default_degraded_recovery_successes")]
    pub degraded_recovery_successes: u32,

    /// Latency threshold (p95 ms) to mark as degraded
    #[serde(default = "default_latency_degradation_threshold")]
    pub latency_degradation_threshold_ms: u64,

    /// Latency threshold (p95 ms) to recover from degraded
    #[serde(default = "default_latency_healthy_threshold")]
    pub latency_healthy_threshold_ms: u64,

    /// Rate limit remaining percentage to trigger warning
    #[serde(default = "default_rate_limit_warning_threshold")]
    pub rate_limit_warning_threshold: f64,

    /// Circuit breaker configuration
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerConfigToml,

    /// Probe configuration
    #[serde(default)]
    pub probe: ProbeConfigToml,
}

impl Default for HealthConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_health_enabled(),
            active_probing: default_active_probing(),
            failure_threshold: default_failure_threshold(),
            recovery_successes: default_recovery_successes(),
            degraded_recovery_successes: default_degraded_recovery_successes(),
            latency_degradation_threshold_ms: default_latency_degradation_threshold(),
            latency_healthy_threshold_ms: default_latency_healthy_threshold(),
            rate_limit_warning_threshold: default_rate_limit_warning_threshold(),
            circuit_breaker: CircuitBreakerConfigToml::default(),
            probe: ProbeConfigToml::default(),
        }
    }
}

impl HealthConfigToml {
    /// Convert to HealthConfig for the health manager
    pub fn to_health_config(&self) -> HealthConfig {
        HealthConfig {
            enabled: self.enabled,
            active_probing: self.active_probing,
            failure_threshold: self.failure_threshold,
            recovery_successes: self.recovery_successes,
            degraded_recovery_successes: self.degraded_recovery_successes,
            latency_degradation_threshold_ms: self.latency_degradation_threshold_ms,
            latency_healthy_threshold_ms: self.latency_healthy_threshold_ms,
            rate_limit_warning_threshold: self.rate_limit_warning_threshold,
            circuit_breaker: self.circuit_breaker.to_circuit_breaker_config(),
            probe: self.probe.to_probe_config(),
        }
    }

    /// Validate health configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.failure_threshold == 0 {
            return Err(
                "agent.model_routing.health.failure_threshold must be greater than 0".to_string(),
            );
        }

        if self.recovery_successes == 0 {
            return Err(
                "agent.model_routing.health.recovery_successes must be greater than 0".to_string(),
            );
        }

        if self.latency_healthy_threshold_ms >= self.latency_degradation_threshold_ms {
            return Err(format!(
                "latency_healthy_threshold_ms ({}) must be less than latency_degradation_threshold_ms ({})",
                self.latency_healthy_threshold_ms, self.latency_degradation_threshold_ms
            ));
        }

        if self.rate_limit_warning_threshold < 0.0 || self.rate_limit_warning_threshold > 1.0 {
            return Err(format!(
                "rate_limit_warning_threshold must be between 0.0 and 1.0, got {}",
                self.rate_limit_warning_threshold
            ));
        }

        self.circuit_breaker.validate()?;

        Ok(())
    }
}

// =============================================================================
// CircuitBreakerConfigToml
// =============================================================================

/// Circuit breaker configuration from TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfigToml {
    /// Number of failures to open circuit
    #[serde(default = "default_cb_failure_threshold")]
    pub failure_threshold: u32,

    /// Window in seconds for counting failures
    #[serde(default = "default_cb_window_secs")]
    pub window_secs: u64,

    /// Base cooldown in seconds before half-open
    #[serde(default = "default_cb_cooldown_secs")]
    pub cooldown_secs: u64,

    /// Number of successes in half-open to close circuit
    #[serde(default = "default_cb_half_open_successes")]
    pub half_open_successes: u32,
}

impl Default for CircuitBreakerConfigToml {
    fn default() -> Self {
        Self {
            failure_threshold: default_cb_failure_threshold(),
            window_secs: default_cb_window_secs(),
            cooldown_secs: default_cb_cooldown_secs(),
            half_open_successes: default_cb_half_open_successes(),
        }
    }
}

impl CircuitBreakerConfigToml {
    /// Convert to CircuitBreakerConfig
    pub fn to_circuit_breaker_config(&self) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: self.failure_threshold,
            window_secs: self.window_secs,
            cooldown_secs: self.cooldown_secs,
            half_open_successes: self.half_open_successes,
        }
    }

    /// Validate circuit breaker configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.failure_threshold == 0 {
            return Err("circuit_breaker.failure_threshold must be greater than 0".to_string());
        }

        if self.cooldown_secs == 0 {
            return Err("circuit_breaker.cooldown_secs must be greater than 0".to_string());
        }

        if self.half_open_successes == 0 {
            return Err("circuit_breaker.half_open_successes must be greater than 0".to_string());
        }

        Ok(())
    }
}

// =============================================================================
// ProbeConfigToml
// =============================================================================

/// Probe configuration from TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeConfigToml {
    /// Interval between probes in seconds
    #[serde(default = "default_probe_interval_secs")]
    pub interval_secs: u64,

    /// Timeout for probe requests in seconds
    #[serde(default = "default_probe_timeout_secs")]
    pub timeout_secs: u64,

    /// Minimal test prompt for probing
    #[serde(default = "default_probe_test_prompt")]
    pub test_prompt: String,
}

impl Default for ProbeConfigToml {
    fn default() -> Self {
        Self {
            interval_secs: default_probe_interval_secs(),
            timeout_secs: default_probe_timeout_secs(),
            test_prompt: default_probe_test_prompt(),
        }
    }
}

impl ProbeConfigToml {
    /// Convert to ProbeConfig
    pub fn to_probe_config(&self) -> ProbeConfig {
        ProbeConfig {
            interval_secs: self.interval_secs,
            timeout_secs: self.timeout_secs,
            test_prompt: self.test_prompt.clone(),
        }
    }
}

// =============================================================================
// Default Functions
// =============================================================================

fn default_health_enabled() -> bool {
    true
}

fn default_active_probing() -> bool {
    false
}

fn default_failure_threshold() -> u32 {
    3
}

fn default_recovery_successes() -> u32 {
    2
}

fn default_degraded_recovery_successes() -> u32 {
    3
}

fn default_latency_degradation_threshold() -> u64 {
    10000
}

fn default_latency_healthy_threshold() -> u64 {
    5000
}

fn default_rate_limit_warning_threshold() -> f64 {
    0.2
}

fn default_cb_failure_threshold() -> u32 {
    5
}

fn default_cb_window_secs() -> u64 {
    60
}

fn default_cb_cooldown_secs() -> u64 {
    30
}

fn default_cb_half_open_successes() -> u32 {
    2
}

fn default_probe_interval_secs() -> u64 {
    30
}

fn default_probe_timeout_secs() -> u64 {
    10
}

fn default_probe_test_prompt() -> String {
    "Hi".to_string()
}
