//! Budget Types
//!
//! Core types for budget management: scope, period, enforcement, limits, and state.

use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};

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

fn default_warning_thresholds() -> Vec<f64> {
    vec![0.5, 0.8, 0.95]
}

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
// Budget Event
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
