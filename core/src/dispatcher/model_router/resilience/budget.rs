//! Budget Management for Model Router
//!
//! This module provides cost control and budget enforcement for AI model usage.
//! It tracks spending in real-time, enforces configurable limits at multiple scopes,
//! and provides visibility into budget status for UI display.

use crate::dispatcher::model_router::{CallRecord, CostTier};
use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// =============================================================================
// Budget Scope
// =============================================================================

/// Budget limit scope for hierarchical cost control
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
#[derive(Default)]
pub enum BudgetScope {
    /// Global limit across all usage
    #[default]
    Global,
    /// Per-project limit
    Project(String),
    /// Per-session limit (conversation)
    Session(String),
    /// Per-model limit
    Model(String),
}

impl BudgetScope {
    /// Create a global scope
    pub fn global() -> Self {
        Self::Global
    }

    /// Create a project scope
    pub fn project(id: impl Into<String>) -> Self {
        Self::Project(id.into())
    }

    /// Create a session scope
    pub fn session(id: impl Into<String>) -> Self {
        Self::Session(id.into())
    }

    /// Create a model scope
    pub fn model(id: impl Into<String>) -> Self {
        Self::Model(id.into())
    }

    /// Get scope priority (lower number = broader scope)
    pub fn priority(&self) -> u8 {
        match self {
            Self::Global => 0,
            Self::Project(_) => 1,
            Self::Session(_) => 2,
            Self::Model(_) => 3,
        }
    }

    /// Check if this scope matches or is broader than another
    pub fn contains(&self, other: &BudgetScope) -> bool {
        match (self, other) {
            (Self::Global, _) => true,
            (Self::Project(a), Self::Project(b)) => a == b,
            (Self::Session(a), Self::Session(b)) => a == b,
            (Self::Model(a), Self::Model(b)) => a == b,
            _ => false,
        }
    }

    /// Get display name
    pub fn as_str(&self) -> String {
        match self {
            Self::Global => "global".to_string(),
            Self::Project(id) => format!("project:{}", id),
            Self::Session(id) => format!("session:{}", id),
            Self::Model(id) => format!("model:{}", id),
        }
    }
}

// =============================================================================
// Budget Period
// =============================================================================

/// Time period for budget reset
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BudgetPeriod {
    /// Never resets
    Lifetime,

    /// Resets daily at configured hour (UTC)
    Daily {
        #[serde(default)]
        reset_hour: u8,
    },

    /// Resets weekly on configured day and hour
    Weekly {
        #[serde(default)]
        reset_day: u8, // 0 = Monday, 6 = Sunday
        #[serde(default)]
        reset_hour: u8,
    },

    /// Resets monthly on configured day and hour
    Monthly {
        #[serde(default = "default_reset_day")]
        reset_day: u8, // 1-31, clamped to last day of month
        #[serde(default)]
        reset_hour: u8,
    },
}

fn default_reset_day() -> u8 {
    1
}

impl Default for BudgetPeriod {
    fn default() -> Self {
        Self::Daily { reset_hour: 0 }
    }
}

impl BudgetPeriod {
    /// Create daily period with midnight reset
    pub fn daily() -> Self {
        Self::Daily { reset_hour: 0 }
    }

    /// Create daily period with specific reset hour
    pub fn daily_at(hour: u8) -> Self {
        Self::Daily {
            reset_hour: hour.min(23),
        }
    }

    /// Create weekly period with Monday midnight reset
    pub fn weekly() -> Self {
        Self::Weekly {
            reset_day: 0,
            reset_hour: 0,
        }
    }

    /// Create monthly period with first day reset
    pub fn monthly() -> Self {
        Self::Monthly {
            reset_day: 1,
            reset_hour: 0,
        }
    }

    /// Calculate next reset time from given time
    pub fn next_reset_from(&self, from: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Self::Lifetime => {
                // Far future
                Utc.with_ymd_and_hms(2100, 1, 1, 0, 0, 0).unwrap()
            }

            Self::Daily { reset_hour } => {
                let hour = *reset_hour as u32;
                let mut next = from
                    .with_hour(hour)
                    .unwrap()
                    .with_minute(0)
                    .unwrap()
                    .with_second(0)
                    .unwrap();

                if next <= from {
                    next += chrono::Duration::days(1);
                }
                next
            }

            Self::Weekly {
                reset_day,
                reset_hour,
            } => {
                let target_weekday = match *reset_day {
                    0 => Weekday::Mon,
                    1 => Weekday::Tue,
                    2 => Weekday::Wed,
                    3 => Weekday::Thu,
                    4 => Weekday::Fri,
                    5 => Weekday::Sat,
                    _ => Weekday::Sun,
                };

                let current_weekday = from.weekday();
                let days_until = (target_weekday.num_days_from_monday() as i64
                    - current_weekday.num_days_from_monday() as i64
                    + 7)
                    % 7;

                let mut next = from + chrono::Duration::days(days_until);
                next = next
                    .with_hour(*reset_hour as u32)
                    .unwrap()
                    .with_minute(0)
                    .unwrap()
                    .with_second(0)
                    .unwrap();

                if next <= from {
                    next += chrono::Duration::weeks(1);
                }
                next
            }

            Self::Monthly {
                reset_day,
                reset_hour,
            } => {
                let day = (*reset_day).max(1);
                let hour = *reset_hour as u32;

                // Try current month
                let year = from.year();
                let month = from.month();

                // Clamp day to last day of month
                let last_day = last_day_of_month(year, month);
                let target_day = day.min(last_day);

                let mut next = Utc
                    .with_ymd_and_hms(year, month, target_day as u32, hour, 0, 0)
                    .unwrap();

                if next <= from {
                    // Move to next month
                    let (next_year, next_month) = if month == 12 {
                        (year + 1, 1)
                    } else {
                        (year, month + 1)
                    };
                    let last_day = last_day_of_month(next_year, next_month);
                    let target_day = day.min(last_day);
                    next = Utc
                        .with_ymd_and_hms(next_year, next_month, target_day as u32, hour, 0, 0)
                        .unwrap();
                }
                next
            }
        }
    }

    /// Calculate next reset time from now
    pub fn next_reset(&self) -> DateTime<Utc> {
        self.next_reset_from(Utc::now())
    }

    /// Get period description
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Lifetime => "lifetime",
            Self::Daily { .. } => "daily",
            Self::Weekly { .. } => "weekly",
            Self::Monthly { .. } => "monthly",
        }
    }
}

fn last_day_of_month(year: i32, month: u32) -> u8 {
    let next_month = if month == 12 { 1 } else { month + 1 };
    let next_year = if month == 12 { year + 1 } else { year };

    let first_of_next = Utc
        .with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
        .unwrap();
    let last = first_of_next - chrono::Duration::days(1);
    last.day() as u8
}

// =============================================================================
// Budget Enforcement
// =============================================================================

/// Action when budget limit is exceeded
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BudgetEnforcement {
    /// Log warning but allow requests
    WarnOnly,
    /// Block new requests, allow in-flight
    #[default]
    SoftBlock,
    /// Block immediately
    HardBlock,
}

impl BudgetEnforcement {
    /// Get display name
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WarnOnly => "warn_only",
            Self::SoftBlock => "soft_block",
            Self::HardBlock => "hard_block",
        }
    }

    /// Check if this enforcement blocks requests
    pub fn blocks(&self) -> bool {
        matches!(self, Self::SoftBlock | Self::HardBlock)
    }
}

// =============================================================================
// Budget Limit
// =============================================================================

/// A single budget configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetLimit {
    /// Unique identifier
    pub id: String,

    /// Scope this limit applies to
    #[serde(default)]
    pub scope: BudgetScope,

    /// Reset period
    #[serde(default)]
    pub period: BudgetPeriod,

    /// Maximum spend in USD
    pub limit_usd: f64,

    /// Warning thresholds (fractions, e.g., [0.5, 0.8, 0.95])
    #[serde(default = "default_warning_thresholds")]
    pub warning_thresholds: Vec<f64>,

    /// Action when limit exceeded
    #[serde(default)]
    pub enforcement: BudgetEnforcement,
}

fn default_warning_thresholds() -> Vec<f64> {
    vec![0.5, 0.8, 0.95]
}

impl BudgetLimit {
    /// Create a new budget limit
    pub fn new(id: impl Into<String>, limit_usd: f64) -> Self {
        Self {
            id: id.into(),
            scope: BudgetScope::Global,
            period: BudgetPeriod::Daily { reset_hour: 0 },
            limit_usd,
            warning_thresholds: default_warning_thresholds(),
            enforcement: BudgetEnforcement::SoftBlock,
        }
    }

    /// Builder: set scope
    pub fn with_scope(mut self, scope: BudgetScope) -> Self {
        self.scope = scope;
        self
    }

    /// Builder: set period
    pub fn with_period(mut self, period: BudgetPeriod) -> Self {
        self.period = period;
        self
    }

    /// Builder: set warning thresholds
    pub fn with_warning_thresholds(mut self, thresholds: Vec<f64>) -> Self {
        self.warning_thresholds = thresholds.into_iter().map(|t| t.clamp(0.0, 1.0)).collect();
        self.warning_thresholds
            .sort_by(|a, b| a.partial_cmp(b).unwrap());
        self
    }

    /// Builder: set enforcement
    pub fn with_enforcement(mut self, enforcement: BudgetEnforcement) -> Self {
        self.enforcement = enforcement;
        self
    }

    /// Check if a spend amount would exceed this limit
    pub fn would_exceed(&self, current_spent: f64, additional: f64) -> bool {
        current_spent + additional > self.limit_usd
    }

    /// Get remaining budget
    pub fn remaining(&self, current_spent: f64) -> f64 {
        (self.limit_usd - current_spent).max(0.0)
    }

    /// Get used percentage
    pub fn used_percent(&self, current_spent: f64) -> f64 {
        if self.limit_usd <= 0.0 {
            return 1.0;
        }
        (current_spent / self.limit_usd).clamp(0.0, 1.0)
    }
}

// =============================================================================
// Budget State
// =============================================================================

/// Current state of a budget
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetState {
    /// Reference to limit config ID
    pub limit_id: String,

    /// Current period start
    pub period_start: DateTime<Utc>,

    /// Next reset time
    pub next_reset: DateTime<Utc>,

    /// Accumulated spend in current period (USD)
    pub spent_usd: f64,

    /// Remaining budget (USD)
    pub remaining_usd: f64,

    /// Percentage used (0.0 - 1.0)
    pub used_percent: f64,

    /// Warning thresholds already fired in this period
    pub warnings_fired: Vec<f64>,
}

impl BudgetState {
    /// Create initial state for a limit
    pub fn new(limit: &BudgetLimit) -> Self {
        let now = Utc::now();
        let next_reset = limit.period.next_reset_from(now);

        Self {
            limit_id: limit.id.clone(),
            period_start: now,
            next_reset,
            spent_usd: 0.0,
            remaining_usd: limit.limit_usd,
            used_percent: 0.0,
            warnings_fired: Vec::new(),
        }
    }

    /// Update state after recording cost
    pub fn record_cost(&mut self, cost_usd: f64, limit: &BudgetLimit) {
        self.spent_usd += cost_usd;
        self.remaining_usd = (limit.limit_usd - self.spent_usd).max(0.0);
        self.used_percent = if limit.limit_usd > 0.0 {
            (self.spent_usd / limit.limit_usd).clamp(0.0, 1.0)
        } else {
            1.0
        };
    }

    /// Reset state for new period
    pub fn reset(&mut self, limit: &BudgetLimit) {
        let now = Utc::now();
        self.period_start = now;
        self.next_reset = limit.period.next_reset_from(now);
        self.spent_usd = 0.0;
        self.remaining_usd = limit.limit_usd;
        self.used_percent = 0.0;
        self.warnings_fired.clear();
    }

    /// Check if reset is due
    pub fn needs_reset(&self) -> bool {
        Utc::now() >= self.next_reset
    }

    /// Get unfired warning thresholds that would be triggered
    pub fn check_warnings(&self, thresholds: &[f64]) -> Vec<f64> {
        thresholds
            .iter()
            .filter(|t| **t <= self.used_percent && !self.warnings_fired.contains(t))
            .copied()
            .collect()
    }

    /// Mark a warning threshold as fired
    pub fn fire_warning(&mut self, threshold: f64) {
        if !self.warnings_fired.contains(&threshold) {
            self.warnings_fired.push(threshold);
        }
    }
}

// =============================================================================
// Budget Check Result
// =============================================================================

/// Result of budget check before execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BudgetCheckResult {
    /// OK to proceed
    Allowed { remaining_usd: f64 },

    /// Warning threshold crossed but allowed
    Warning {
        threshold: f64,
        remaining_usd: f64,
        message: String,
    },

    /// Blocked by soft limit
    SoftBlocked {
        limit_id: String,
        spent_usd: f64,
        limit_usd: f64,
    },

    /// Blocked by hard limit
    HardBlocked {
        limit_id: String,
        spent_usd: f64,
        limit_usd: f64,
    },
}

impl BudgetCheckResult {
    /// Check if result allows proceeding
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed { .. } | Self::Warning { .. })
    }

    /// Check if result is blocked
    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::SoftBlocked { .. } | Self::HardBlocked { .. })
    }

    /// Get remaining budget if allowed
    pub fn remaining(&self) -> Option<f64> {
        match self {
            Self::Allowed { remaining_usd } => Some(*remaining_usd),
            Self::Warning { remaining_usd, .. } => Some(*remaining_usd),
            _ => None,
        }
    }

    /// Get display message
    pub fn message(&self) -> String {
        match self {
            Self::Allowed { remaining_usd } => {
                format!("Budget OK: ${:.2} remaining", remaining_usd)
            }
            Self::Warning { message, .. } => message.clone(),
            Self::SoftBlocked {
                limit_id,
                spent_usd,
                limit_usd,
            } => {
                format!(
                    "Budget limit '{}' exceeded: ${:.2}/${:.2}",
                    limit_id, spent_usd, limit_usd
                )
            }
            Self::HardBlocked {
                limit_id,
                spent_usd,
                limit_usd,
            } => {
                format!(
                    "Hard budget limit '{}' exceeded: ${:.2}/${:.2}",
                    limit_id, spent_usd, limit_usd
                )
            }
        }
    }
}

impl std::fmt::Display for BudgetCheckResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

// =============================================================================
// Cost Estimator
// =============================================================================

/// Model pricing data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Price per million input tokens (USD)
    pub input_price_per_1m: f64,
    /// Price per million output tokens (USD)
    pub output_price_per_1m: f64,
    /// Price per million cached input tokens (if supported)
    pub cached_input_price_per_1m: Option<f64>,
}

impl Default for ModelPricing {
    fn default() -> Self {
        // Default to Medium tier pricing
        Self {
            input_price_per_1m: 3.0,
            output_price_per_1m: 15.0,
            cached_input_price_per_1m: None,
        }
    }
}

impl ModelPricing {
    /// Create pricing from cost tier
    pub fn from_cost_tier(tier: CostTier) -> Self {
        match tier {
            CostTier::Free => Self {
                input_price_per_1m: 0.0,
                output_price_per_1m: 0.0,
                cached_input_price_per_1m: Some(0.0),
            },
            CostTier::Low => Self {
                input_price_per_1m: 0.25,
                output_price_per_1m: 1.25,
                cached_input_price_per_1m: Some(0.025),
            },
            CostTier::Medium => Self {
                input_price_per_1m: 3.0,
                output_price_per_1m: 15.0,
                cached_input_price_per_1m: Some(0.3),
            },
            CostTier::High => Self {
                input_price_per_1m: 15.0,
                output_price_per_1m: 75.0,
                cached_input_price_per_1m: Some(1.5),
            },
        }
    }

    /// Calculate cost for given token counts
    pub fn calculate_cost(&self, input_tokens: u32, output_tokens: u32) -> f64 {
        let input_cost = (input_tokens as f64 / 1_000_000.0) * self.input_price_per_1m;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * self.output_price_per_1m;
        input_cost + output_cost
    }
}

/// Pre-call cost estimation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Model ID
    pub model_id: String,
    /// Input tokens
    pub input_tokens: u32,
    /// Estimated output tokens
    pub estimated_output_tokens: u32,
    /// Base cost estimate (USD)
    pub base_cost_usd: f64,
    /// Cost with safety margin (USD)
    pub with_margin_usd: f64,
    /// Source of pricing data
    pub pricing_source: PricingSource,
}

impl CostEstimate {
    /// Get the conservative estimate (with margin)
    pub fn cost(&self) -> f64 {
        self.with_margin_usd
    }
}

/// Source of pricing data
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PricingSource {
    /// From static configuration (ModelProfile)
    Profile,
    /// Learned from actual usage
    Learned,
    /// From provider API
    ProviderApi,
    /// Default tier-based estimate
    Default,
}

/// Cost estimator for pre-call budget checks
#[derive(Debug, Clone)]
pub struct CostEstimator {
    /// Model pricing data
    pricing: HashMap<String, ModelPricing>,
    /// Safety margin for estimation (e.g., 1.2 = 20% buffer)
    safety_margin: f64,
    /// Default output token estimate when not provided
    default_output_estimate: u32,
}

impl Default for CostEstimator {
    fn default() -> Self {
        Self {
            pricing: HashMap::new(),
            safety_margin: 1.2,
            default_output_estimate: 500,
        }
    }
}

impl CostEstimator {
    /// Create a new cost estimator
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: set safety margin
    pub fn with_safety_margin(mut self, margin: f64) -> Self {
        self.safety_margin = margin.max(1.0);
        self
    }

    /// Builder: set default output estimate
    pub fn with_default_output_estimate(mut self, tokens: u32) -> Self {
        self.default_output_estimate = tokens;
        self
    }

    /// Set pricing for a model
    pub fn set_pricing(&mut self, model_id: impl Into<String>, pricing: ModelPricing) {
        self.pricing.insert(model_id.into(), pricing);
    }

    /// Set pricing from cost tier
    pub fn set_pricing_from_tier(&mut self, model_id: impl Into<String>, tier: CostTier) {
        self.pricing
            .insert(model_id.into(), ModelPricing::from_cost_tier(tier));
    }

    /// Get pricing for a model
    pub fn get_pricing(&self, model_id: &str) -> Option<&ModelPricing> {
        self.pricing.get(model_id)
    }

    /// Estimate cost before making a call
    pub fn estimate(
        &self,
        model_id: &str,
        input_tokens: u32,
        estimated_output_tokens: Option<u32>,
    ) -> CostEstimate {
        let output_tokens = estimated_output_tokens.unwrap_or(self.default_output_estimate);

        let (pricing, source) = self.pricing.get(model_id).map_or_else(
            || (ModelPricing::default(), PricingSource::Default),
            |p| (p.clone(), PricingSource::Profile),
        );

        let base_cost = pricing.calculate_cost(input_tokens, output_tokens);
        let with_margin = base_cost * self.safety_margin;

        CostEstimate {
            model_id: model_id.to_string(),
            input_tokens,
            estimated_output_tokens: output_tokens,
            base_cost_usd: base_cost,
            with_margin_usd: with_margin,
            pricing_source: source,
        }
    }

    /// Update pricing from actual costs (learning)
    pub fn learn_from_actual(&mut self, record: &CallRecord) {
        if let Some(actual_cost) = record.cost_usd {
            let total_tokens = record.input_tokens + record.output_tokens;
            if total_tokens == 0 {
                return;
            }

            // Calculate actual per-token costs
            // This is a simplification - in practice we'd need input/output breakdown
            let pricing = self.pricing.entry(record.model_id.clone()).or_default();

            // Use exponential moving average to update
            let alpha = 0.1; // Learning rate

            // Estimate split based on typical ratios
            let input_ratio = record.input_tokens as f64 / total_tokens as f64;
            let output_ratio = record.output_tokens as f64 / total_tokens as f64;

            if record.input_tokens > 0 {
                let input_cost_est = actual_cost * input_ratio;
                let actual_input_per_1m =
                    (input_cost_est / record.input_tokens as f64) * 1_000_000.0;
                pricing.input_price_per_1m =
                    pricing.input_price_per_1m * (1.0 - alpha) + actual_input_per_1m * alpha;
            }

            if record.output_tokens > 0 {
                let output_cost_est = actual_cost * output_ratio;
                let actual_output_per_1m =
                    (output_cost_est / record.output_tokens as f64) * 1_000_000.0;
                pricing.output_price_per_1m =
                    pricing.output_price_per_1m * (1.0 - alpha) + actual_output_per_1m * alpha;
            }
        }
    }
}

// =============================================================================
// Budget Manager
// =============================================================================

/// Event emitted by budget manager
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BudgetEvent {
    /// Warning threshold crossed
    Warning {
        limit_id: String,
        threshold: f64,
        spent_usd: f64,
        limit_usd: f64,
    },
    /// Budget limit exceeded
    Exceeded {
        limit_id: String,
        spent_usd: f64,
        limit_usd: f64,
    },
    /// Budget reset occurred
    Reset { limit_id: String },
    /// Cost recorded
    CostRecorded {
        limit_id: String,
        cost_usd: f64,
        spent_usd: f64,
        remaining_usd: f64,
    },
}

/// Central budget management
pub struct BudgetManager {
    /// Configured limits
    limits: Vec<BudgetLimit>,
    /// Current state per limit
    states: Arc<RwLock<HashMap<String, BudgetState>>>,
    /// Cost estimator
    estimator: Arc<RwLock<CostEstimator>>,
    /// Event sender
    event_tx: Option<tokio::sync::broadcast::Sender<BudgetEvent>>,
}

impl BudgetManager {
    /// Create a new budget manager with limits
    pub fn new(limits: Vec<BudgetLimit>) -> Self {
        let states: HashMap<String, BudgetState> = limits
            .iter()
            .map(|l| (l.id.clone(), BudgetState::new(l)))
            .collect();

        let (event_tx, _) = tokio::sync::broadcast::channel(100);

        Self {
            limits,
            states: Arc::new(RwLock::new(states)),
            estimator: Arc::new(RwLock::new(CostEstimator::new())),
            event_tx: Some(event_tx),
        }
    }

    /// Create with no limits (disabled)
    pub fn disabled() -> Self {
        Self {
            limits: Vec::new(),
            states: Arc::new(RwLock::new(HashMap::new())),
            estimator: Arc::new(RwLock::new(CostEstimator::new())),
            event_tx: None,
        }
    }

    /// Check if budget management is enabled
    pub fn is_enabled(&self) -> bool {
        !self.limits.is_empty()
    }

    /// Subscribe to budget events
    pub fn subscribe(&self) -> Option<tokio::sync::broadcast::Receiver<BudgetEvent>> {
        self.event_tx.as_ref().map(|tx| tx.subscribe())
    }

    /// Set cost estimator
    pub async fn set_estimator(&self, estimator: CostEstimator) {
        let mut est = self.estimator.write().await;
        *est = estimator;
    }

    /// Get cost estimator reference
    pub async fn estimator(&self) -> CostEstimator {
        self.estimator.read().await.clone()
    }

    /// Check budget before execution
    pub async fn check_budget(
        &self,
        scope: &BudgetScope,
        estimate: &CostEstimate,
    ) -> BudgetCheckResult {
        if self.limits.is_empty() {
            return BudgetCheckResult::Allowed {
                remaining_usd: f64::MAX,
            };
        }

        // Check for resets first
        self.check_and_apply_resets().await;

        let states = self.states.read().await;
        let mut min_remaining = f64::MAX;
        let mut warning_to_fire: Option<(String, f64, f64, f64)> = None;

        for limit in &self.limits {
            // Check if limit applies to this scope
            if !limit.scope.contains(scope) && limit.scope != BudgetScope::Global {
                continue;
            }

            if let Some(state) = states.get(&limit.id) {
                let would_spend = state.spent_usd + estimate.cost();

                // Check if would exceed limit
                if would_spend > limit.limit_usd {
                    match limit.enforcement {
                        BudgetEnforcement::HardBlock => {
                            return BudgetCheckResult::HardBlocked {
                                limit_id: limit.id.clone(),
                                spent_usd: state.spent_usd,
                                limit_usd: limit.limit_usd,
                            };
                        }
                        BudgetEnforcement::SoftBlock => {
                            return BudgetCheckResult::SoftBlocked {
                                limit_id: limit.id.clone(),
                                spent_usd: state.spent_usd,
                                limit_usd: limit.limit_usd,
                            };
                        }
                        BudgetEnforcement::WarnOnly => {
                            // Continue checking, will return warning
                        }
                    }
                }

                // Check warning thresholds
                let new_percent = would_spend / limit.limit_usd;
                for &threshold in &limit.warning_thresholds {
                    if new_percent >= threshold && !state.warnings_fired.contains(&threshold) {
                        warning_to_fire =
                            Some((limit.id.clone(), threshold, would_spend, limit.limit_usd));
                        break;
                    }
                }

                // Track minimum remaining
                let remaining = limit.limit_usd - would_spend;
                if remaining < min_remaining {
                    min_remaining = remaining;
                }
            }
        }

        // Return warning if any threshold would be crossed
        if let Some((limit_id, threshold, spent, limit)) = warning_to_fire {
            return BudgetCheckResult::Warning {
                threshold,
                remaining_usd: (limit - spent).max(0.0),
                message: format!(
                    "Budget {}% used on '{}': ${:.2}/${:.2}",
                    (threshold * 100.0) as u32,
                    limit_id,
                    spent,
                    limit
                ),
            };
        }

        BudgetCheckResult::Allowed {
            remaining_usd: min_remaining.max(0.0),
        }
    }

    /// Record actual cost after call completes
    pub async fn record_cost(&self, scope: &BudgetScope, record: &CallRecord) {
        let cost = record.cost_usd.unwrap_or_else(|| {
            // Estimate from tokens if actual cost not available
            let estimator = self.estimator.blocking_read();
            let estimate = estimator.estimate(
                &record.model_id,
                record.input_tokens,
                Some(record.output_tokens),
            );
            estimate.base_cost_usd
        });

        let mut states = self.states.write().await;

        for limit in &self.limits {
            if !limit.scope.contains(scope) && limit.scope != BudgetScope::Global {
                continue;
            }

            if let Some(state) = states.get_mut(&limit.id) {
                let _old_percent = state.used_percent;
                state.record_cost(cost, limit);

                // Check for new warning thresholds
                let new_warnings = state.check_warnings(&limit.warning_thresholds);
                for threshold in &new_warnings {
                    state.fire_warning(*threshold);

                    // Emit warning event
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(BudgetEvent::Warning {
                            limit_id: limit.id.clone(),
                            threshold: *threshold,
                            spent_usd: state.spent_usd,
                            limit_usd: limit.limit_usd,
                        });
                    }
                }

                // Emit cost recorded event
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(BudgetEvent::CostRecorded {
                        limit_id: limit.id.clone(),
                        cost_usd: cost,
                        spent_usd: state.spent_usd,
                        remaining_usd: state.remaining_usd,
                    });
                }
            }
        }

        // Update estimator with actual cost
        if record.cost_usd.is_some() {
            let mut estimator = self.estimator.write().await;
            estimator.learn_from_actual(record);
        }
    }

    /// Get current status for a scope
    pub async fn get_status(&self, scope: &BudgetScope) -> Vec<BudgetState> {
        let states = self.states.read().await;

        self.limits
            .iter()
            .filter(|l| l.scope.contains(scope) || l.scope == BudgetScope::Global)
            .filter_map(|l| states.get(&l.id).cloned())
            .collect()
    }

    /// Get all budget states
    pub async fn all_states(&self) -> HashMap<String, BudgetState> {
        self.states.read().await.clone()
    }

    /// Manually reset a limit
    pub async fn reset_limit(&self, limit_id: &str) {
        let limit = self.limits.iter().find(|l| l.id == limit_id);
        if let Some(limit) = limit {
            let mut states = self.states.write().await;
            if let Some(state) = states.get_mut(limit_id) {
                state.reset(limit);

                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(BudgetEvent::Reset {
                        limit_id: limit_id.to_string(),
                    });
                }
            }
        }
    }

    /// Convenience method: estimate cost for a call
    pub fn estimate_cost(
        &self,
        model_id: &str,
        input_tokens: u32,
        estimated_output_tokens: u32,
    ) -> CostEstimate {
        let estimator = self.estimator.blocking_read();
        estimator.estimate(model_id, input_tokens, Some(estimated_output_tokens))
    }

    /// Convenience method: record cost directly without a CallRecord
    pub async fn record_cost_direct(&self, scope: &BudgetScope, cost_usd: f64) {
        let mut states = self.states.write().await;

        for limit in &self.limits {
            if !limit.scope.contains(scope) && limit.scope != BudgetScope::Global {
                continue;
            }

            if let Some(state) = states.get_mut(&limit.id) {
                state.record_cost(cost_usd, limit);

                // Check for new warning thresholds
                let new_warnings = state.check_warnings(&limit.warning_thresholds);
                for threshold in &new_warnings {
                    state.fire_warning(*threshold);

                    // Emit warning event
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(BudgetEvent::Warning {
                            limit_id: limit.id.clone(),
                            threshold: *threshold,
                            spent_usd: state.spent_usd,
                            limit_usd: limit.limit_usd,
                        });
                    }
                }

                // Emit cost recorded event
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(BudgetEvent::CostRecorded {
                        limit_id: limit.id.clone(),
                        cost_usd,
                        spent_usd: state.spent_usd,
                        remaining_usd: state.remaining_usd,
                    });
                }
            }
        }
    }

    /// Record actual cost after call completes using a CallRecord
    pub async fn record_cost_from_call(&self, scope: &BudgetScope, record: &CallRecord) {
        let cost = record.cost_usd.unwrap_or_else(|| {
            // Estimate from tokens if actual cost not available
            let estimator = self.estimator.blocking_read();
            let estimate = estimator.estimate(
                &record.model_id,
                record.input_tokens,
                Some(record.output_tokens),
            );
            estimate.base_cost_usd
        });

        self.record_cost_direct(scope, cost).await;

        // Update estimator with actual cost
        if record.cost_usd.is_some() {
            let mut estimator = self.estimator.write().await;
            estimator.learn_from_actual(record);
        }
    }

    /// Check and apply any due resets
    async fn check_and_apply_resets(&self) {
        let mut states = self.states.write().await;

        for limit in &self.limits {
            if let Some(state) = states.get_mut(&limit.id) {
                if state.needs_reset() {
                    state.reset(limit);

                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(BudgetEvent::Reset {
                            limit_id: limit.id.clone(),
                        });
                    }
                }
            }
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
    fn test_budget_scope_priority() {
        assert!(BudgetScope::Global.priority() < BudgetScope::Project("a".into()).priority());
        assert!(
            BudgetScope::Project("a".into()).priority()
                < BudgetScope::Session("a".into()).priority()
        );
        assert!(
            BudgetScope::Session("a".into()).priority() < BudgetScope::Model("a".into()).priority()
        );
    }

    #[test]
    fn test_budget_scope_contains() {
        let global = BudgetScope::Global;
        let project = BudgetScope::project("test");
        let session = BudgetScope::session("s1");

        assert!(global.contains(&project));
        assert!(global.contains(&session));
        assert!(!project.contains(&global));
        assert!(!session.contains(&project));
    }

    #[test]
    fn test_budget_period_daily_next_reset() {
        let period = BudgetPeriod::daily_at(0);
        let now = Utc::now();
        let next = period.next_reset_from(now);

        assert!(next > now);
        assert_eq!(next.hour(), 0);
        assert_eq!(next.minute(), 0);
    }

    #[test]
    fn test_budget_period_monthly_last_day() {
        // Test February with day 31 - should clamp
        let period = BudgetPeriod::Monthly {
            reset_day: 31,
            reset_hour: 0,
        };

        let feb = Utc.with_ymd_and_hms(2024, 2, 1, 0, 0, 0).unwrap();
        let next = period.next_reset_from(feb);

        // Should be Feb 29 (2024 is leap year)
        assert_eq!(next.day(), 29);
    }

    #[test]
    fn test_budget_limit_builder() {
        let limit = BudgetLimit::new("daily", 10.0)
            .with_scope(BudgetScope::Global)
            .with_period(BudgetPeriod::daily())
            .with_warning_thresholds(vec![0.5, 0.8])
            .with_enforcement(BudgetEnforcement::HardBlock);

        assert_eq!(limit.id, "daily");
        assert_eq!(limit.limit_usd, 10.0);
        assert_eq!(limit.warning_thresholds, vec![0.5, 0.8]);
        assert_eq!(limit.enforcement, BudgetEnforcement::HardBlock);
    }

    #[test]
    fn test_budget_limit_calculations() {
        let limit = BudgetLimit::new("test", 10.0);

        assert!(limit.would_exceed(9.0, 2.0));
        assert!(!limit.would_exceed(5.0, 2.0));

        assert_eq!(limit.remaining(3.0), 7.0);
        assert_eq!(limit.remaining(15.0), 0.0); // Clamped to 0

        assert!((limit.used_percent(5.0) - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_budget_state_record_cost() {
        let limit = BudgetLimit::new("test", 10.0);
        let mut state = BudgetState::new(&limit);

        state.record_cost(3.0, &limit);
        assert!((state.spent_usd - 3.0).abs() < 0.001);
        assert!((state.remaining_usd - 7.0).abs() < 0.001);
        assert!((state.used_percent - 0.3).abs() < 0.001);

        state.record_cost(5.0, &limit);
        assert!((state.spent_usd - 8.0).abs() < 0.001);
    }

    #[test]
    fn test_budget_state_warnings() {
        let limit = BudgetLimit::new("test", 10.0).with_warning_thresholds(vec![0.5, 0.8]);
        let mut state = BudgetState::new(&limit);

        state.record_cost(6.0, &limit); // 60%
        let warnings = state.check_warnings(&limit.warning_thresholds);
        assert!(warnings.contains(&0.5));
        assert!(!warnings.contains(&0.8)); // 80% not yet

        state.fire_warning(0.5);
        let warnings = state.check_warnings(&limit.warning_thresholds);
        assert!(!warnings.contains(&0.5)); // Already fired
    }

    #[test]
    fn test_budget_check_result() {
        let allowed = BudgetCheckResult::Allowed { remaining_usd: 5.0 };
        assert!(allowed.is_allowed());
        assert!(!allowed.is_blocked());
        assert_eq!(allowed.remaining(), Some(5.0));

        let blocked = BudgetCheckResult::HardBlocked {
            limit_id: "test".into(),
            spent_usd: 10.0,
            limit_usd: 10.0,
        };
        assert!(!blocked.is_allowed());
        assert!(blocked.is_blocked());
    }

    #[test]
    fn test_model_pricing_from_tier() {
        let free = ModelPricing::from_cost_tier(CostTier::Free);
        assert_eq!(free.input_price_per_1m, 0.0);

        let high = ModelPricing::from_cost_tier(CostTier::High);
        assert!(high.input_price_per_1m > 0.0);
        assert!(high.output_price_per_1m > high.input_price_per_1m);
    }

    #[test]
    fn test_model_pricing_calculate() {
        let pricing = ModelPricing {
            input_price_per_1m: 3.0,
            output_price_per_1m: 15.0,
            cached_input_price_per_1m: None,
        };

        let cost = pricing.calculate_cost(1_000_000, 100_000);
        // 1M input * $3/M + 100K output * $15/M = $3 + $1.5 = $4.5
        assert!((cost - 4.5).abs() < 0.001);
    }

    #[test]
    fn test_cost_estimator_estimate() {
        let mut estimator = CostEstimator::new().with_safety_margin(1.2);

        estimator.set_pricing(
            "gpt-4o",
            ModelPricing {
                input_price_per_1m: 5.0,
                output_price_per_1m: 15.0,
                cached_input_price_per_1m: None,
            },
        );

        let estimate = estimator.estimate("gpt-4o", 1000, Some(500));
        assert!(estimate.base_cost_usd > 0.0);
        assert!(estimate.with_margin_usd > estimate.base_cost_usd);
        assert_eq!(estimate.pricing_source, PricingSource::Profile);
    }

    #[test]
    fn test_cost_estimator_default_pricing() {
        let estimator = CostEstimator::new();
        let estimate = estimator.estimate("unknown-model", 1000, Some(500));
        assert_eq!(estimate.pricing_source, PricingSource::Default);
    }

    #[tokio::test]
    async fn test_budget_manager_check() {
        let limits =
            vec![BudgetLimit::new("daily", 10.0).with_enforcement(BudgetEnforcement::SoftBlock)];

        let manager = BudgetManager::new(limits);

        let estimate = CostEstimate {
            model_id: "test".into(),
            input_tokens: 1000,
            estimated_output_tokens: 500,
            base_cost_usd: 0.01,
            with_margin_usd: 0.012,
            pricing_source: PricingSource::Default,
        };

        let result = manager.check_budget(&BudgetScope::Global, &estimate).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_budget_manager_blocked() {
        let limits = vec![BudgetLimit::new("daily", 0.01) // Very low limit
            .with_enforcement(BudgetEnforcement::HardBlock)];

        let manager = BudgetManager::new(limits);

        // First, record some cost to exceed the limit
        {
            let mut states = manager.states.write().await;
            if let Some(state) = states.get_mut("daily") {
                state.spent_usd = 0.02; // Already exceeded
            }
        }

        let estimate = CostEstimate {
            model_id: "test".into(),
            input_tokens: 1000,
            estimated_output_tokens: 500,
            base_cost_usd: 0.01,
            with_margin_usd: 0.012,
            pricing_source: PricingSource::Default,
        };

        let result = manager.check_budget(&BudgetScope::Global, &estimate).await;
        assert!(result.is_blocked());
    }

    #[tokio::test]
    async fn test_budget_manager_disabled() {
        let manager = BudgetManager::disabled();
        assert!(!manager.is_enabled());

        let estimate = CostEstimate {
            model_id: "test".into(),
            input_tokens: 1000,
            estimated_output_tokens: 500,
            base_cost_usd: 100.0, // High cost
            with_margin_usd: 120.0,
            pricing_source: PricingSource::Default,
        };

        // Should always be allowed when disabled
        let result = manager.check_budget(&BudgetScope::Global, &estimate).await;
        assert!(result.is_allowed());
    }

    #[test]
    fn test_budget_enforcement() {
        assert!(!BudgetEnforcement::WarnOnly.blocks());
        assert!(BudgetEnforcement::SoftBlock.blocks());
        assert!(BudgetEnforcement::HardBlock.blocks());
    }
}
