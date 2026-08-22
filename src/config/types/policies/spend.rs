//! Per-principal and machine-total USD spend ceilings.
//!
//! The word here is **spend**, never `budget`: [`crate::context::budget`] is
//! the context-window budget (how much of the prompt a turn may occupy), and
//! `TerminateReason::BudgetExhaustedPartialResult` is the per-run token
//! budget (when a single run must stop early and hand back a partial
//! result). Three different things — reusing `budget` for a USD ceiling
//! would make every future grep for either of the other two ambiguous.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Billing cycle duration for per-principal spend limits.
///
/// Determines the window over which `per_user_usd` is accumulated and checked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SpendPeriod {
    /// Monthly billing cycle (default).
    #[default]
    Month,
    /// Daily billing cycle.
    Day,
}

/// Per-principal and machine-total USD spend ceilings.
///
/// Omitting either `per_user_usd` or `total_usd` means that axis has no limit.
/// Call [`Self::enabled`] to check if any ceiling is actually enforced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpendPolicy {
    /// Per-principal USD spend ceiling.
    ///
    /// Omitted means unlimited spend per principal within this period.
    /// See [`SpendPeriod`] for the active cycle length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_user_usd: Option<f64>,

    /// Machine-total USD spend ceiling.
    ///
    /// Omitted means unlimited cumulative spend across all principals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_usd: Option<f64>,

    /// Billing cycle duration.
    #[serde(default)]
    pub period: SpendPeriod,
}

impl SpendPolicy {
    /// Whether any spend ceiling is active.
    ///
    /// Returns `false` when no code may read or write the ledger.
    pub const fn enabled(&self) -> bool {
        self.per_user_usd.is_some() || self.total_usd.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty `[policies.spend]` table parses, and with neither ceiling
    /// set the policy is disabled — "no limit at all" is the default.
    #[test]
    fn empty_table_parses_and_is_disabled() {
        let policy: SpendPolicy = toml::from_str("").unwrap();
        assert!(!policy.enabled());
        assert_eq!(policy.per_user_usd, None);
        assert_eq!(policy.total_usd, None);
        assert_eq!(policy.period, SpendPeriod::Month);
    }

    /// `enabled()` flips true the moment either ceiling is set.
    #[test]
    fn enabled_when_either_ceiling_is_set() {
        let per_user_only = SpendPolicy {
            per_user_usd: Some(10.0),
            ..SpendPolicy::default()
        };
        assert!(per_user_only.enabled());

        let total_only = SpendPolicy {
            total_usd: Some(100.0),
            ..SpendPolicy::default()
        };
        assert!(total_only.enabled());
    }

    /// `period = "day"` parses to `SpendPeriod::Day`. An unknown period
    /// string is an error, not a silent default — a typo that quietly means
    /// "month" is exactly the class of failure this round exists to
    /// prevent.
    #[test]
    fn day_period_parses_and_unknown_period_is_an_error() {
        let policy: SpendPolicy = toml::from_str(r#"period = "day""#).unwrap();
        assert_eq!(policy.period, SpendPeriod::Day);

        assert!(toml::from_str::<SpendPolicy>(r#"period = "fortnight""#).is_err());
    }

    /// A `SpendPolicy` carrying only `total_usd` round-trips through TOML
    /// with no `per_user_usd` key emitted — that is what
    /// `skip_serializing_if` buys, and `save_incremental`'s clearing path
    /// depends on it.
    #[test]
    fn total_usd_only_round_trips_without_emitting_per_user_usd() {
        let policy = SpendPolicy {
            total_usd: Some(500.0),
            ..SpendPolicy::default()
        };
        let toml_str = toml::to_string(&policy).unwrap();
        assert!(!toml_str.contains("per_user_usd"));
        assert!(toml_str.contains("total_usd"));

        let round_tripped: SpendPolicy = toml::from_str(&toml_str).unwrap();
        assert_eq!(round_tripped, policy);
    }
}
