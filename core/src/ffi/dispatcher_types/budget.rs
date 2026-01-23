//! Budget FFI Types (Model Router P1)
//!
//! Contains Budget management FFI types:
//! - BudgetScopeFFI: Budget scope (Global, Project, Session, Model)
//! - BudgetPeriodFFI: Budget period (Lifetime, Daily, Weekly, Monthly)
//! - BudgetEnforcementFFI: Enforcement action
//! - BudgetLimitStatusFFI: Status of a single limit
//! - BudgetStatusFFI: Overall budget status

use crate::dispatcher::model_router::{
    BudgetEnforcement, BudgetLimit, BudgetPeriod, BudgetScope, BudgetState,
};

// ============================================================================
// Budget Scope FFI
// ============================================================================

/// Budget scope for FFI (Global, Project, Session, Model)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetScopeFFI {
    Global,
    Project { id: String },
    Session { id: String },
    Model { id: String },
}

impl From<&BudgetScope> for BudgetScopeFFI {
    fn from(scope: &BudgetScope) -> Self {
        match scope {
            BudgetScope::Global => BudgetScopeFFI::Global,
            BudgetScope::Project(id) => BudgetScopeFFI::Project { id: id.clone() },
            BudgetScope::Session(id) => BudgetScopeFFI::Session { id: id.clone() },
            BudgetScope::Model(id) => BudgetScopeFFI::Model { id: id.clone() },
        }
    }
}

impl From<BudgetScope> for BudgetScopeFFI {
    fn from(scope: BudgetScope) -> Self {
        BudgetScopeFFI::from(&scope)
    }
}

impl From<&BudgetScopeFFI> for BudgetScope {
    fn from(ffi: &BudgetScopeFFI) -> Self {
        match ffi {
            BudgetScopeFFI::Global => BudgetScope::Global,
            BudgetScopeFFI::Project { id } => BudgetScope::Project(id.clone()),
            BudgetScopeFFI::Session { id } => BudgetScope::Session(id.clone()),
            BudgetScopeFFI::Model { id } => BudgetScope::Model(id.clone()),
        }
    }
}

impl From<BudgetScopeFFI> for BudgetScope {
    fn from(ffi: BudgetScopeFFI) -> Self {
        BudgetScope::from(&ffi)
    }
}

// ============================================================================
// Budget Period FFI
// ============================================================================

/// Budget period for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetPeriodFFI {
    Lifetime,
    Daily,
    Weekly,
    Monthly,
}

impl From<&BudgetPeriod> for BudgetPeriodFFI {
    fn from(period: &BudgetPeriod) -> Self {
        match period {
            BudgetPeriod::Lifetime => BudgetPeriodFFI::Lifetime,
            BudgetPeriod::Daily { .. } => BudgetPeriodFFI::Daily,
            BudgetPeriod::Weekly { .. } => BudgetPeriodFFI::Weekly,
            BudgetPeriod::Monthly { .. } => BudgetPeriodFFI::Monthly,
        }
    }
}

impl From<BudgetPeriod> for BudgetPeriodFFI {
    fn from(period: BudgetPeriod) -> Self {
        BudgetPeriodFFI::from(&period)
    }
}

// ============================================================================
// Budget Enforcement FFI
// ============================================================================

/// Budget enforcement action for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetEnforcementFFI {
    WarnOnly,
    SoftBlock,
    HardBlock,
}

impl From<BudgetEnforcement> for BudgetEnforcementFFI {
    fn from(enforcement: BudgetEnforcement) -> Self {
        match enforcement {
            BudgetEnforcement::WarnOnly => BudgetEnforcementFFI::WarnOnly,
            BudgetEnforcement::SoftBlock => BudgetEnforcementFFI::SoftBlock,
            BudgetEnforcement::HardBlock => BudgetEnforcementFFI::HardBlock,
        }
    }
}

impl From<BudgetEnforcementFFI> for BudgetEnforcement {
    fn from(ffi: BudgetEnforcementFFI) -> Self {
        match ffi {
            BudgetEnforcementFFI::WarnOnly => BudgetEnforcement::WarnOnly,
            BudgetEnforcementFFI::SoftBlock => BudgetEnforcement::SoftBlock,
            BudgetEnforcementFFI::HardBlock => BudgetEnforcement::HardBlock,
        }
    }
}

// ============================================================================
// Budget Limit Status FFI
// ============================================================================

/// Status of a single budget limit for FFI
#[derive(Debug, Clone)]
pub struct BudgetLimitStatusFFI {
    /// Limit unique identifier
    pub limit_id: String,
    /// Scope this limit applies to
    pub scope: BudgetScopeFFI,
    /// Scope as display string
    pub scope_display: String,
    /// Budget period type
    pub period: BudgetPeriodFFI,
    /// Period as display string
    pub period_display: String,
    /// Configured limit in USD
    pub limit_usd: f64,
    /// Current spend in USD
    pub spent_usd: f64,
    /// Remaining budget in USD
    pub remaining_usd: f64,
    /// Percentage used (0.0 - 1.0)
    pub used_percent: f64,
    /// Enforcement action when exceeded
    pub enforcement: BudgetEnforcementFFI,
    /// Whether the limit is currently exceeded
    pub is_exceeded: bool,
    /// Whether any warning threshold has been crossed
    pub is_warning: bool,
    /// Next reset timestamp (Unix epoch seconds)
    pub next_reset_timestamp: i64,
    /// Human-readable time until reset
    pub next_reset_display: String,
}

impl BudgetLimitStatusFFI {
    /// Create from a BudgetLimit and BudgetState
    pub fn from_limit_and_state(limit: &BudgetLimit, state: &BudgetState) -> Self {
        let now = chrono::Utc::now();
        let duration_until_reset = state.next_reset.signed_duration_since(now);

        let next_reset_display = if duration_until_reset.num_hours() < 1 {
            format!("{} minutes", duration_until_reset.num_minutes().max(1))
        } else if duration_until_reset.num_days() < 1 {
            format!("{} hours", duration_until_reset.num_hours())
        } else {
            format!("{} days", duration_until_reset.num_days())
        };

        Self {
            limit_id: limit.id.clone(),
            scope: BudgetScopeFFI::from(&limit.scope),
            scope_display: limit.scope.as_str(),
            period: BudgetPeriodFFI::from(&limit.period),
            period_display: limit.period.as_str().to_string(),
            limit_usd: limit.limit_usd,
            spent_usd: state.spent_usd,
            remaining_usd: state.remaining_usd,
            used_percent: state.used_percent,
            enforcement: BudgetEnforcementFFI::from(limit.enforcement),
            is_exceeded: state.spent_usd >= limit.limit_usd,
            is_warning: !state.warnings_fired.is_empty(),
            next_reset_timestamp: state.next_reset.timestamp(),
            next_reset_display,
        }
    }
}

// ============================================================================
// Budget Status FFI
// ============================================================================

/// Overall budget status summary for FFI
#[derive(Debug, Clone)]
pub struct BudgetStatusFFI {
    /// Whether budget management is enabled
    pub enabled: bool,
    /// Total number of configured limits
    pub total_limits: u32,
    /// Number of limits currently exceeded
    pub exceeded_count: u32,
    /// Number of limits with active warnings
    pub warning_count: u32,
    /// Total spent across all scopes in USD
    pub total_spent_usd: f64,
    /// Total remaining across all limits in USD (minimum remaining)
    pub min_remaining_usd: f64,
    /// Status per configured limit
    pub limits: Vec<BudgetLimitStatusFFI>,
    /// Status emoji for quick display
    pub status_emoji: String,
    /// Human-readable status message
    pub status_message: String,
}

impl BudgetStatusFFI {
    /// Create a disabled budget status
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            total_limits: 0,
            exceeded_count: 0,
            warning_count: 0,
            total_spent_usd: 0.0,
            min_remaining_usd: f64::MAX,
            limits: Vec::new(),
            status_emoji: "⚫".to_string(),
            status_message: "Budget management disabled".to_string(),
        }
    }

    /// Create status from BudgetManager data
    pub fn from_limits_and_states(
        limits: &[BudgetLimit],
        states: &std::collections::HashMap<String, BudgetState>,
    ) -> Self {
        if limits.is_empty() {
            return Self::disabled();
        }

        let mut limit_statuses = Vec::new();
        let mut total_spent = 0.0;
        let mut min_remaining = f64::MAX;
        let mut exceeded_count = 0u32;
        let mut warning_count = 0u32;

        for limit in limits {
            if let Some(state) = states.get(&limit.id) {
                let status = BudgetLimitStatusFFI::from_limit_and_state(limit, state);

                total_spent += state.spent_usd;
                if status.remaining_usd < min_remaining {
                    min_remaining = status.remaining_usd;
                }
                if status.is_exceeded {
                    exceeded_count += 1;
                }
                if status.is_warning {
                    warning_count += 1;
                }

                limit_statuses.push(status);
            }
        }

        let (status_emoji, status_message) = if exceeded_count > 0 {
            (
                "🔴".to_string(),
                format!("{} budget(s) exceeded", exceeded_count),
            )
        } else if warning_count > 0 {
            (
                "🟡".to_string(),
                format!("{} budget warning(s)", warning_count),
            )
        } else {
            ("🟢".to_string(), "All budgets healthy".to_string())
        };

        Self {
            enabled: true,
            total_limits: limits.len() as u32,
            exceeded_count,
            warning_count,
            total_spent_usd: total_spent,
            min_remaining_usd: if min_remaining == f64::MAX {
                0.0
            } else {
                min_remaining
            },
            limits: limit_statuses,
            status_emoji,
            status_message,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_budget_scope_ffi_conversion() {
        // Global scope
        let global = BudgetScope::Global;
        let ffi: BudgetScopeFFI = (&global).into();
        assert_eq!(ffi, BudgetScopeFFI::Global);
        let back: BudgetScope = ffi.into();
        assert_eq!(back, global);

        // Project scope
        let project = BudgetScope::Project("test-project".to_string());
        let ffi: BudgetScopeFFI = (&project).into();
        assert_eq!(
            ffi,
            BudgetScopeFFI::Project {
                id: "test-project".to_string()
            }
        );
        let back: BudgetScope = ffi.into();
        assert_eq!(back, project);

        // Session scope
        let session = BudgetScope::Session("session-123".to_string());
        let ffi: BudgetScopeFFI = (&session).into();
        assert_eq!(
            ffi,
            BudgetScopeFFI::Session {
                id: "session-123".to_string()
            }
        );

        // Model scope
        let model = BudgetScope::Model("claude-opus".to_string());
        let ffi: BudgetScopeFFI = (&model).into();
        assert_eq!(
            ffi,
            BudgetScopeFFI::Model {
                id: "claude-opus".to_string()
            }
        );
    }

    #[test]
    fn test_budget_period_ffi_conversion() {
        let periods = [
            (BudgetPeriod::Lifetime, BudgetPeriodFFI::Lifetime),
            (BudgetPeriod::daily(), BudgetPeriodFFI::Daily),
            (BudgetPeriod::weekly(), BudgetPeriodFFI::Weekly),
            (BudgetPeriod::monthly(), BudgetPeriodFFI::Monthly),
        ];

        for (period, expected_ffi) in periods {
            let ffi: BudgetPeriodFFI = (&period).into();
            assert_eq!(ffi, expected_ffi);
        }
    }

    #[test]
    fn test_budget_enforcement_ffi_conversion() {
        let enforcements = [
            (BudgetEnforcement::WarnOnly, BudgetEnforcementFFI::WarnOnly),
            (
                BudgetEnforcement::SoftBlock,
                BudgetEnforcementFFI::SoftBlock,
            ),
            (
                BudgetEnforcement::HardBlock,
                BudgetEnforcementFFI::HardBlock,
            ),
        ];

        for (enforcement, expected_ffi) in enforcements {
            let ffi: BudgetEnforcementFFI = enforcement.into();
            assert_eq!(ffi, expected_ffi);

            let back: BudgetEnforcement = ffi.into();
            assert_eq!(back, enforcement);
        }
    }

    #[test]
    fn test_budget_limit_status_ffi_creation() {
        let limit = BudgetLimit::new("daily-global", 10.0)
            .with_scope(BudgetScope::Global)
            .with_period(BudgetPeriod::daily());

        let state = BudgetState::new(&limit);

        let ffi = BudgetLimitStatusFFI::from_limit_and_state(&limit, &state);

        assert_eq!(ffi.limit_id, "daily-global");
        assert_eq!(ffi.scope, BudgetScopeFFI::Global);
        assert_eq!(ffi.scope_display, "global");
        assert_eq!(ffi.period, BudgetPeriodFFI::Daily);
        assert_eq!(ffi.period_display, "daily");
        assert!((ffi.limit_usd - 10.0).abs() < 0.001);
        assert!((ffi.spent_usd - 0.0).abs() < 0.001);
        assert!((ffi.remaining_usd - 10.0).abs() < 0.001);
        assert!((ffi.used_percent - 0.0).abs() < 0.001);
        assert!(!ffi.is_exceeded);
        assert!(!ffi.is_warning);
    }

    #[test]
    fn test_budget_status_ffi_disabled() {
        let status = BudgetStatusFFI::disabled();

        assert!(!status.enabled);
        assert_eq!(status.total_limits, 0);
        assert_eq!(status.exceeded_count, 0);
        assert_eq!(status.warning_count, 0);
        assert!(status.limits.is_empty());
        assert_eq!(status.status_emoji, "⚫");
        assert!(status.status_message.contains("disabled"));
    }

    #[test]
    fn test_budget_status_ffi_from_limits() {
        // Create some test limits
        let limits = vec![
            BudgetLimit::new("daily", 10.0)
                .with_scope(BudgetScope::Global)
                .with_period(BudgetPeriod::daily()),
            BudgetLimit::new("monthly", 100.0)
                .with_scope(BudgetScope::Global)
                .with_period(BudgetPeriod::monthly()),
        ];

        // Create states
        let mut states = HashMap::new();
        for limit in &limits {
            states.insert(limit.id.clone(), BudgetState::new(limit));
        }

        let status = BudgetStatusFFI::from_limits_and_states(&limits, &states);

        assert!(status.enabled);
        assert_eq!(status.total_limits, 2);
        assert_eq!(status.exceeded_count, 0);
        assert_eq!(status.warning_count, 0);
        assert_eq!(status.limits.len(), 2);
        assert_eq!(status.status_emoji, "🟢");
        assert!(status.status_message.contains("healthy"));
    }
}
