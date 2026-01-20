//! Dispatcher configuration types
//!
//! Contains Dispatcher Layer (Aether Cortex) configuration:
//! - DispatcherConfigToml: Multi-layer routing and confirmation settings
//! - AgentConfigToml: L3 Agent (multi-step planning) settings
//! - ModelRouterConfigToml: Model routing with retry/failover/budget settings (P1)

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
///
/// [dispatcher.agent]
/// enabled = true
/// max_steps = 10
/// step_timeout_ms = 30000
/// enable_rollback = true
/// plan_confirmation_required = true
/// allow_irreversible_without_confirmation = false
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

    /// L3 Agent configuration for multi-step planning
    #[serde(default)]
    pub agent: AgentConfigToml,
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
            agent: AgentConfigToml::default(),
        }
    }
}

// =============================================================================
// AgentConfigToml - L3 Agent (Multi-step Planning) Configuration
// =============================================================================

/// Configuration for L3 Agent multi-step planning and execution
///
/// The L3 Agent enables intelligent multi-step task planning where the AI
/// decomposes complex requests into sequential tool invocations.
///
/// # Example TOML
///
/// ```toml
/// [dispatcher.agent]
/// enabled = true
/// max_steps = 10
/// step_timeout_ms = 30000
/// enable_rollback = true
/// plan_confirmation_required = true
/// allow_irreversible_without_confirmation = false
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfigToml {
    /// Whether agent mode is enabled (default: true)
    #[serde(default = "default_agent_enabled")]
    pub enabled: bool,

    /// Maximum number of steps allowed in a plan (default: 10)
    #[serde(default = "default_agent_max_steps")]
    pub max_steps: u32,

    /// Timeout for each step in milliseconds (default: 30000)
    #[serde(default = "default_agent_step_timeout")]
    pub step_timeout_ms: u64,

    /// Whether to attempt rollback on failure (default: true)
    #[serde(default = "default_agent_enable_rollback")]
    pub enable_rollback: bool,

    /// Whether to require user confirmation before executing plans (default: true)
    #[serde(default = "default_agent_plan_confirmation_required")]
    pub plan_confirmation_required: bool,

    /// Whether irreversible steps can run without additional confirmation (default: false)
    /// When false, plans with irreversible steps will show a warning.
    #[serde(default = "default_agent_allow_irreversible")]
    pub allow_irreversible_without_confirmation: bool,

    /// Heuristics threshold for triggering planning (default: 2)
    /// Number of action verbs/connectors needed to trigger multi-step planning
    #[serde(default = "default_agent_heuristics_threshold")]
    pub heuristics_threshold: u32,
}

pub fn default_agent_enabled() -> bool {
    true
}

pub fn default_agent_max_steps() -> u32 {
    10
}

pub fn default_agent_step_timeout() -> u64 {
    30000 // 30 seconds per step
}

pub fn default_agent_enable_rollback() -> bool {
    true
}

pub fn default_agent_plan_confirmation_required() -> bool {
    true
}

pub fn default_agent_allow_irreversible() -> bool {
    false
}

pub fn default_agent_heuristics_threshold() -> u32 {
    2 // At least 2 action signals to trigger planning
}

impl Default for AgentConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_agent_enabled(),
            max_steps: default_agent_max_steps(),
            step_timeout_ms: default_agent_step_timeout(),
            enable_rollback: default_agent_enable_rollback(),
            plan_confirmation_required: default_agent_plan_confirmation_required(),
            allow_irreversible_without_confirmation: default_agent_allow_irreversible(),
            heuristics_threshold: default_agent_heuristics_threshold(),
        }
    }
}

impl AgentConfigToml {
    /// Validate the agent configuration
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.max_steps == 0 {
            return Err("agent.max_steps must be > 0".to_string());
        }
        if self.max_steps > 50 {
            warn!(
                max_steps = self.max_steps,
                "agent.max_steps > 50 may cause excessive processing"
            );
        }

        if self.step_timeout_ms == 0 {
            return Err("agent.step_timeout_ms must be > 0".to_string());
        }
        if self.step_timeout_ms > 120000 {
            warn!(
                timeout = self.step_timeout_ms,
                "agent.step_timeout_ms > 120000ms may cause poor user experience"
            );
        }

        Ok(())
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

        // Validate agent configuration
        self.agent.validate()?;

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

// =============================================================================
// RetryConfigToml - Retry and Failover Configuration
// =============================================================================

/// Configuration for retry and failover behavior
///
/// # Example TOML
///
/// ```toml
/// [model_router.retry]
/// enabled = true
/// max_attempts = 3
/// attempt_timeout_ms = 30000
/// total_timeout_ms = 90000
/// failover_on_non_retryable = true
/// retryable_errors = ["timeout", "rate_limited", "network_error", "server_error"]
///
/// [model_router.retry.backoff]
/// strategy = "exponential_jitter"
/// initial_ms = 100
/// max_ms = 5000
/// jitter_factor = 0.2
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfigToml {
    /// Whether retry is enabled (default: true)
    #[serde(default = "default_retry_enabled")]
    pub enabled: bool,

    /// Maximum number of attempts (including initial) (default: 3)
    #[serde(default = "default_retry_max_attempts")]
    pub max_attempts: u32,

    /// Timeout for each individual attempt in milliseconds (default: 30000)
    #[serde(default = "default_retry_attempt_timeout")]
    pub attempt_timeout_ms: u64,

    /// Total timeout across all attempts in milliseconds (default: 90000)
    /// Set to 0 to disable total timeout
    #[serde(default = "default_retry_total_timeout")]
    pub total_timeout_ms: u64,

    /// Whether to use failover on non-retryable errors (default: true)
    #[serde(default = "default_retry_failover_on_non_retryable")]
    pub failover_on_non_retryable: bool,

    /// Error types that trigger retry (default: timeout, rate_limited, network_error, server_error)
    #[serde(default = "default_retry_retryable_errors")]
    pub retryable_errors: Vec<String>,

    /// Backoff configuration
    #[serde(default)]
    pub backoff: BackoffConfigToml,
}

fn default_retry_enabled() -> bool {
    true
}

fn default_retry_max_attempts() -> u32 {
    3
}

fn default_retry_attempt_timeout() -> u64 {
    30000 // 30 seconds
}

fn default_retry_total_timeout() -> u64 {
    90000 // 90 seconds
}

fn default_retry_failover_on_non_retryable() -> bool {
    true
}

fn default_retry_retryable_errors() -> Vec<String> {
    vec![
        "timeout".to_string(),
        "rate_limited".to_string(),
        "network_error".to_string(),
        "server_error".to_string(),
    ]
}

impl Default for RetryConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_retry_enabled(),
            max_attempts: default_retry_max_attempts(),
            attempt_timeout_ms: default_retry_attempt_timeout(),
            total_timeout_ms: default_retry_total_timeout(),
            failover_on_non_retryable: default_retry_failover_on_non_retryable(),
            retryable_errors: default_retry_retryable_errors(),
            backoff: BackoffConfigToml::default(),
        }
    }
}

impl RetryConfigToml {
    /// Validate the retry configuration
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.max_attempts == 0 {
            return Err("retry.max_attempts must be > 0".to_string());
        }
        if self.max_attempts > 10 {
            warn!(
                max_attempts = self.max_attempts,
                "retry.max_attempts > 10 may cause excessive retry loops"
            );
        }

        if self.attempt_timeout_ms == 0 {
            return Err("retry.attempt_timeout_ms must be > 0".to_string());
        }

        if self.total_timeout_ms > 0 && self.total_timeout_ms < self.attempt_timeout_ms {
            warn!(
                total = self.total_timeout_ms,
                attempt = self.attempt_timeout_ms,
                "retry.total_timeout_ms < attempt_timeout_ms may prevent retries"
            );
        }

        self.backoff.validate()?;
        Ok(())
    }

    /// Convert to internal RetryPolicy
    pub fn to_retry_policy(&self) -> crate::dispatcher::model_router::RetryPolicy {
        use crate::dispatcher::model_router::{RetryPolicy, RetryableOutcome};

        let retryable_outcomes: Vec<RetryableOutcome> = self
            .retryable_errors
            .iter()
            .filter_map(|s| match s.as_str() {
                "timeout" => Some(RetryableOutcome::Timeout),
                "rate_limited" => Some(RetryableOutcome::RateLimited),
                "network_error" => Some(RetryableOutcome::NetworkError),
                "server_error" => Some(RetryableOutcome::ServerError),
                _ => None,
            })
            .collect();

        RetryPolicy {
            max_attempts: self.max_attempts,
            attempt_timeout_ms: self.attempt_timeout_ms,
            total_timeout_ms: if self.total_timeout_ms > 0 {
                Some(self.total_timeout_ms)
            } else {
                None
            },
            retryable_outcomes,
            failover_on_non_retryable: self.failover_on_non_retryable,
        }
    }
}

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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_router_config_default() {
        let config = ModelRouterConfigToml::default();
        assert!(config.enabled);
        assert!(config.retry.enabled);
        assert!(config.budget.enabled);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfigToml::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.attempt_timeout_ms, 30000);
        assert_eq!(config.total_timeout_ms, 90000);
        assert!(config.failover_on_non_retryable);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_retry_config_validation() {
        let mut config = RetryConfigToml::default();
        config.max_attempts = 0;
        assert!(config.validate().is_err());

        config.max_attempts = 3;
        config.backoff.strategy = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_backoff_config_default() {
        let config = BackoffConfigToml::default();
        assert_eq!(config.strategy, "exponential_jitter");
        assert_eq!(config.initial_ms, 100);
        assert_eq!(config.max_ms, 5000);
        assert_eq!(config.multiplier, 2.0);
        assert_eq!(config.jitter_factor, 0.2);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_backoff_config_validation() {
        let mut config = BackoffConfigToml::default();

        // Invalid strategy
        config.strategy = "invalid".to_string();
        assert!(config.validate().is_err());

        // Reset and test jitter factor
        config.strategy = "exponential_jitter".to_string();
        config.jitter_factor = 1.5;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_budget_config_default() {
        let config = BudgetConfigToml::default();
        assert!(config.enabled);
        assert_eq!(config.default_enforcement, "soft_block");
        assert_eq!(config.estimation_safety_margin, 1.2);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_budget_limit_config_validation() {
        let limit = BudgetLimitConfigToml {
            id: "test".to_string(),
            scope: "global".to_string(),
            scope_value: None,
            period: "daily".to_string(),
            reset_hour: 0,
            reset_day: 0,
            limit_usd: 10.0,
            warning_thresholds: vec![0.5, 0.8],
            enforcement: Some("soft_block".to_string()),
        };
        assert!(limit.validate().is_ok());

        // Test invalid scope
        let mut invalid = limit.clone();
        invalid.scope = "invalid".to_string();
        assert!(invalid.validate().is_err());

        // Test invalid period
        let mut invalid = limit.clone();
        invalid.period = "invalid".to_string();
        assert!(invalid.validate().is_err());

        // Test invalid limit_usd
        let mut invalid = limit.clone();
        invalid.limit_usd = -1.0;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_retry_config_to_policy() {
        let config = RetryConfigToml::default();
        let policy = config.to_retry_policy();

        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.attempt_timeout_ms, 30000);
        assert_eq!(policy.total_timeout_ms, Some(90000));
    }

    #[test]
    fn test_backoff_config_to_strategy() {
        let config = BackoffConfigToml::default();
        let strategy = config.to_backoff_strategy();

        match strategy {
            crate::dispatcher::model_router::BackoffStrategy::ExponentialJitter {
                initial_ms,
                max_ms,
                jitter_factor,
            } => {
                assert_eq!(initial_ms, 100);
                assert_eq!(max_ms, 5000);
                assert_eq!(jitter_factor, 0.2);
            }
            _ => panic!("Expected ExponentialJitter strategy"),
        }
    }

    #[test]
    fn test_budget_limit_to_internal() {
        let limit = BudgetLimitConfigToml {
            id: "test".to_string(),
            scope: "project".to_string(),
            scope_value: Some("my-project".to_string()),
            period: "weekly".to_string(),
            reset_hour: 8,
            reset_day: 1,
            limit_usd: 50.0,
            warning_thresholds: vec![0.5, 0.8],
            enforcement: Some("hard_block".to_string()),
        };

        let internal = limit.to_budget_limit("soft_block");

        assert_eq!(internal.id, "test");
        assert_eq!(internal.limit_usd, 50.0);
        assert_eq!(internal.warning_thresholds, vec![0.5, 0.8]);
    }
}
