//! Model router configuration types
//!
//! Contains ModelRouterConfigToml for model routing with retry/failover/budget settings.

use serde::{Deserialize, Serialize};

use super::budget::BudgetConfigToml;
use super::retry::RetryConfigToml;

// =============================================================================
// ModelRouterConfigToml - Model Router with Retry/Failover/Budget (P1)
// =============================================================================

/// Configuration for the Model Router
///
/// The Model Router provides intelligent model selection with:
/// - Retry and failover for resilient execution
/// - Budget management for cost control
///
/// # Example TOML
///
/// ```toml
/// [model_router]
/// enabled = true
///
/// [model_router.retry]
/// enabled = true
/// max_attempts = 3
/// attempt_timeout_ms = 30000
/// total_timeout_ms = 90000
///
/// [model_router.retry.backoff]
/// strategy = "exponential_jitter"
/// initial_ms = 100
/// max_ms = 5000
/// jitter_factor = 0.2
///
/// [model_router.budget]
/// enabled = true
/// default_enforcement = "soft_block"
///
/// [[model_router.budget.limits]]
/// id = "daily_global"
/// scope = "global"
/// period = "daily"
/// limit_usd = 10.0
/// warning_thresholds = [0.5, 0.8, 0.95]
/// ```
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRouterConfigToml {
    /// Whether the model router is enabled (default: true)
    #[serde(default = "default_model_router_enabled")]
    pub enabled: bool,

    /// Retry configuration
    #[serde(default)]
    pub retry: RetryConfigToml,

    /// Budget configuration
    #[serde(default)]
    pub budget: BudgetConfigToml,
}

#[allow(dead_code)]
fn default_model_router_enabled() -> bool {
    true
}

impl Default for ModelRouterConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_model_router_enabled(),
            retry: RetryConfigToml::default(),
            budget: BudgetConfigToml::default(),
        }
    }
}

impl ModelRouterConfigToml {
    /// Validate the configuration
    #[allow(dead_code)]
    pub fn validate(&self) -> std::result::Result<(), String> {
        self.retry.validate()?;
        self.budget.validate()?;
        Ok(())
    }
}
