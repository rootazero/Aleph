//! Budget management configuration types
//!
//! Contains BudgetConfigToml and BudgetLimitConfigToml for cost control.

use serde::{Deserialize, Serialize};
use tracing::warn;

// =============================================================================
// BudgetConfigToml - Budget Management Configuration
// =============================================================================

/// Configuration for budget management
///
/// # Example TOML
///
/// ```toml
/// [model_router.budget]
/// enabled = true
/// default_enforcement = "soft_block"
///
/// [[model_router.budget.limits]]
/// id = "daily_global"
/// scope = "global"
/// period = "daily"
/// reset_hour = 0
/// limit_usd = 10.0
/// warning_thresholds = [0.5, 0.8, 0.95]
/// enforcement = "soft_block"
///
/// [[model_router.budget.limits]]
/// id = "session_limit"
/// scope = "session"
/// period = "lifetime"
/// limit_usd = 1.0
/// warning_thresholds = [0.8]
/// enforcement = "warn_only"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfigToml {
    /// Whether budget management is enabled (default: true)
    #[serde(default = "default_budget_enabled")]
    pub enabled: bool,

    /// Default enforcement mode for limits without explicit enforcement
    /// Options: "warn_only", "soft_block", "hard_block"
    #[serde(default = "default_budget_enforcement")]
    pub default_enforcement: String,

    /// Safety margin for cost estimation (default: 1.2 = 20% buffer)
    #[serde(default = "default_budget_safety_margin")]
    pub estimation_safety_margin: f64,

    /// Budget limits
    #[serde(default)]
    pub limits: Vec<BudgetLimitConfigToml>,
}

fn default_budget_enabled() -> bool {
    true
}

fn default_budget_enforcement() -> String {
    "soft_block".to_string()
}

fn default_budget_safety_margin() -> f64 {
    1.2 // 20% buffer
}

impl Default for BudgetConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_budget_enabled(),
            default_enforcement: default_budget_enforcement(),
            estimation_safety_margin: default_budget_safety_margin(),
            limits: Vec::new(),
        }
    }
}

impl BudgetConfigToml {
    /// Validate the budget configuration
    pub fn validate(&self) -> std::result::Result<(), String> {
        let valid_enforcements = ["warn_only", "soft_block", "hard_block"];
        if !valid_enforcements.contains(&self.default_enforcement.as_str()) {
            return Err(format!(
                "budget.default_enforcement must be one of {:?}, got '{}'",
                valid_enforcements, self.default_enforcement
            ));
        }

        if self.estimation_safety_margin < 1.0 {
            warn!(
                margin = self.estimation_safety_margin,
                "budget.estimation_safety_margin < 1.0 may underestimate costs"
            );
        }

        for limit in &self.limits {
            limit.validate()?;
        }

        Ok(())
    }
}

// =============================================================================
// BudgetLimitConfigToml - Individual Budget Limit Configuration
// =============================================================================

/// Configuration for a single budget limit
///
/// # Example TOML
///
/// ```toml
/// [[model_router.budget.limits]]
/// id = "daily_global"
/// scope = "global"
/// period = "daily"
/// reset_hour = 0
/// limit_usd = 10.0
/// warning_thresholds = [0.5, 0.8, 0.95]
/// enforcement = "soft_block"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetLimitConfigToml {
    /// Unique identifier for this limit
    pub id: String,

    /// Scope: "global", "project", "session", "model"
    #[serde(default = "default_limit_scope")]
    pub scope: String,

    /// Scope value (for project/session/model scopes)
    pub scope_value: Option<String>,

    /// Reset period: "lifetime", "daily", "weekly", "monthly"
    #[serde(default = "default_limit_period")]
    pub period: String,

    /// Reset hour (0-23) for daily/weekly/monthly periods
    #[serde(default)]
    pub reset_hour: u8,

    /// Reset day (1-7 for weekly, 1-28 for monthly)
    #[serde(default)]
    pub reset_day: u8,

    /// Maximum spend in USD
    pub limit_usd: f64,

    /// Warning thresholds as fractions (e.g., [0.5, 0.8, 0.95])
    #[serde(default)]
    pub warning_thresholds: Vec<f64>,

    /// Enforcement mode: "warn_only", "soft_block", "hard_block"
    pub enforcement: Option<String>,
}

fn default_limit_scope() -> String {
    "global".to_string()
}

fn default_limit_period() -> String {
    "daily".to_string()
}

impl Default for BudgetLimitConfigToml {
    fn default() -> Self {
        Self {
            id: String::new(),
            scope: default_limit_scope(),
            scope_value: None,
            period: default_limit_period(),
            reset_hour: 0,
            reset_day: 1,
            limit_usd: 0.0,
            warning_thresholds: Vec::new(),
            enforcement: None,
        }
    }
}

impl BudgetLimitConfigToml {
    /// Validate the budget limit configuration
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.id.is_empty() {
            return Err("budget.limits[].id cannot be empty".to_string());
        }

        let valid_scopes = ["global", "project", "session", "model"];
        if !valid_scopes.contains(&self.scope.as_str()) {
            return Err(format!(
                "budget.limits[{}].scope must be one of {:?}, got '{}'",
                self.id, valid_scopes, self.scope
            ));
        }

        let valid_periods = ["lifetime", "daily", "weekly", "monthly"];
        if !valid_periods.contains(&self.period.as_str()) {
            return Err(format!(
                "budget.limits[{}].period must be one of {:?}, got '{}'",
                self.id, valid_periods, self.period
            ));
        }

        if self.limit_usd <= 0.0 {
            return Err(format!("budget.limits[{}].limit_usd must be > 0", self.id));
        }

        for threshold in &self.warning_thresholds {
            if *threshold < 0.0 || *threshold > 1.0 {
                return Err(format!(
                    "budget.limits[{}].warning_thresholds must be between 0.0 and 1.0",
                    self.id
                ));
            }
        }

        if let Some(enforcement) = &self.enforcement {
            let valid_enforcements = ["warn_only", "soft_block", "hard_block"];
            if !valid_enforcements.contains(&enforcement.as_str()) {
                return Err(format!(
                    "budget.limits[{}].enforcement must be one of {:?}, got '{}'",
                    self.id, valid_enforcements, enforcement
                ));
            }
        }

        Ok(())
    }

    /// Convert to internal BudgetLimit
    pub fn to_budget_limit(
        &self,
        default_enforcement: &str,
    ) -> crate::dispatcher::model_router::BudgetLimit {
        use crate::dispatcher::model_router::{
            BudgetEnforcement, BudgetLimit, BudgetPeriod, BudgetScope,
        };

        let scope = match self.scope.as_str() {
            "global" => BudgetScope::Global,
            "project" => BudgetScope::Project(self.scope_value.clone().unwrap_or_default()),
            "session" => BudgetScope::Session(self.scope_value.clone().unwrap_or_default()),
            "model" => BudgetScope::Model(self.scope_value.clone().unwrap_or_default()),
            _ => BudgetScope::Global,
        };

        let period = match self.period.as_str() {
            "lifetime" => BudgetPeriod::Lifetime,
            "daily" => BudgetPeriod::Daily {
                reset_hour: self.reset_hour,
            },
            "weekly" => BudgetPeriod::Weekly {
                reset_day: self.reset_day,
                reset_hour: self.reset_hour,
            },
            "monthly" => BudgetPeriod::Monthly {
                reset_day: self.reset_day.max(1),
                reset_hour: self.reset_hour,
            },
            _ => BudgetPeriod::Daily {
                reset_hour: self.reset_hour,
            },
        };

        let enforcement_str = self.enforcement.as_deref().unwrap_or(default_enforcement);
        let enforcement = match enforcement_str {
            "warn_only" => BudgetEnforcement::WarnOnly,
            "soft_block" => BudgetEnforcement::SoftBlock,
            "hard_block" => BudgetEnforcement::HardBlock,
            _ => BudgetEnforcement::SoftBlock,
        };

        BudgetLimit {
            id: self.id.clone(),
            scope,
            period,
            limit_usd: self.limit_usd,
            warning_thresholds: self.warning_thresholds.clone(),
            enforcement,
        }
    }
}
