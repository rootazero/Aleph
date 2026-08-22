//! Spend query contract — `spend.query`.
//!
//! # Why this module exists
//!
//! `SpendLedger` records every principal's per-period USD spend the moment
//! `[policies.spend]` ceilings are enforced (the metering floor and the
//! admission gate), but until this contract nothing could read the ledger
//! back out. The deliverable is exactly "an admin can read what has been
//! spent, by whom, in the window that is open now" — the read half of a
//! feature whose write half already shipped.
//!
//! # Why the shape lives in this crate
//!
//! Same reason as [`crate::audit`] and [`crate::workspace`]: the client is
//! `aleph-cli`, which deliberately cannot depend on `alephcore`. A wire
//! contract hand-copied into two crates is the defect that made
//! `aleph workspace create` fail with `INVALID_PARAMS` for months while both
//! sides' tests stayed green — one type here makes a rename a compile error
//! on both sides instead of a silent drift.
//!
//! # Why there is no `spend.reset`
//!
//! Zeroing a ledger row is indistinguishable, after the fact, from a write
//! that never happened. Raising the ceiling is the reversible way to say
//! the same thing to an over-limit principal, and it leaves a trail in
//! `[policies.spend]`'s own history; a reset would not.

use serde::{Deserialize, Serialize};

/// Parameters for `spend.query`.
///
/// Empty today: there is exactly one window worth asking about — the period
/// that is open right now, for every principal with a row in it — so there
/// is nothing yet to filter by. `deny_unknown_fields` all the same, so a
/// misspelled or not-yet-implemented key is refused rather than silently
/// ignored, matching [`crate::audit::AuditQueryParams`]'s reasoning.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpendQueryParams {}

/// One principal's accumulated spend within the queried period.
///
/// `unpriced_calls` / `partial_calls` ride alongside `usd` on every row
/// because a total whose confidence is invisible invites a decision the
/// number cannot support: a principal with a large `partial_calls` count has
/// a `usd` figure that is a documented lower bound, not a total, and a
/// reader who cannot see that will read it as one anyway.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpendRow {
    /// The ledger's primary-key text: a `users.user_id`, or the reserved
    /// `"@unattributed"` sentinel for calls whose principal could not be
    /// resolved.
    ///
    /// `"@unattributed"` is a deliberate, ordinary row, never hidden and
    /// never folded into anyone else's total: it is the visible surface of
    /// the known producer-attribution gap, and hiding it would let spend
    /// this surface cannot explain disappear from the report instead of
    /// standing out in it.
    pub principal: String,

    /// Dollars spent by `principal` in the queried period. A lower bound
    /// when `partial_calls > 0` for this row — see that field's doc.
    pub usd: f64,

    /// Calls this principal made that carried no price at all
    /// (`CostStatus::Unknown`) — real spend that `usd` does not reflect in
    /// any amount, not even partially.
    pub unpriced_calls: u64,

    /// Calls this principal made whose price was only partially known
    /// (`CostStatus::PartialMissingPrice`) — `usd` for this row is a lower
    /// bound, not a total, whenever this is nonzero.
    pub partial_calls: u64,
}

/// Response for `spend.query`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpendQueryResult {
    /// Whether a ceiling is actually being enforced right now
    /// (`SpendPolicy::enabled()`: either `per_user_usd` or `total_usd` is
    /// set).
    ///
    /// `false` does **not** mean zero spend, and the rows below are sent
    /// regardless of this value: spend is recorded whether or not a
    /// ceiling is configured to act on it, so `configured: false` says "no
    /// ceiling is being enforced," never "nothing was measured." A reader
    /// who cannot tell those two facts apart would read an unconfigured box
    /// as a thrifty one.
    pub configured: bool,

    /// Start of the period every row below covers, in epoch milliseconds
    /// (the local-calendar boundary `SpendPeriod` resolves to around "now"
    /// — see `spend::period` on the server side; that type is not in this
    /// crate, since the client only ever needs the resolved instant, not
    /// the period-length enum).
    pub period_start_ms: i64,

    /// End of the period every row below covers — the instant the ledger
    /// resets — in epoch milliseconds.
    pub period_end_ms: i64,

    /// Every principal with recorded spend in the queried period, `usd`
    /// descending then key ascending. Deliberately unbounded: bounded by
    /// principals-with-spend in one window, and a silently truncated spend
    /// report is worse than a large one.
    pub rows: Vec<SpendRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_params_serialise_to_an_empty_object() {
        let wire = serde_json::to_value(SpendQueryParams::default()).unwrap();
        assert_eq!(wire, serde_json::json!({}));
    }

    #[test]
    fn an_unknown_param_key_is_refused() {
        let err = serde_json::from_value::<SpendQueryParams>(serde_json::json!({
            "principal": "u-alice"
        }));
        assert!(
            err.is_err(),
            "an unknown key must not deserialize into a params object that quietly ignores it"
        );
    }

    #[test]
    fn an_empty_object_parses_as_the_default_query() {
        let params: SpendQueryParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(params, SpendQueryParams::default());
    }
}
