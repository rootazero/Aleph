//! `spend.query` — the read face of the per-principal spend ledger.
//!
//! # What was missing
//!
//! `SpendLedger` gained a writer at every LLM call (the metering floor,
//! `providers::metering::MeteringProvider::enforce_spend_ceiling` /
//! `record_spend`) and at every run's admission
//! (`gateway::execution_engine::run_loop::deny_if_over_spend`), but nothing
//! could read the ledger back out. The deliverable this handler exists for
//! is exactly "an admin can read what has been spent, by whom, in the
//! window that is open now" — see `aleph_protocol::spend` for the wire
//! contract this handler builds its response from.
//!
//! # Why admin-gated
//!
//! A spend report names every principal on the machine and their dollar
//! figures — org-level accountability, not caller's-own-data — so it sits
//! behind the `spend.` prefix in [`crate::gateway::method_admin`], the same
//! reasoning `security.audit.query` already documents for its own prefix.
//!
//! # Why there is no `spend.reset`
//!
//! See `aleph_protocol::spend`'s module doc: zeroing a ledger row is
//! indistinguishable, after the fact, from a write that never happened.
//! Raising the ceiling is the reversible way to say the same thing.
//!
//! # The two-function split, and why `handle_query` itself is untested
//!
//! [`handle_query`] reads both of `crate::spend`'s process-global handles
//! (`current_policy()`, `global_ledger()`) fresh on every call — mirrors
//! [`crate::spend::check`], which does the same for the same reason:
//! `policy` is hot-reloadable via `spend::update_policy`, so it must never
//! be snapshotted once (at registration time, say) rather than read live.
//! It then delegates immediately to [`handle_query_with`], which takes
//! those two facts as plain parameters.
//!
//! Every test in this module calls [`handle_query_with`], never
//! [`handle_query`] — same split, same reason, as
//! [`crate::spend::check`]/[`crate::spend::check_with`] and
//! [`crate::gateway::execution_engine::run_loop::deny_if_over_spend`]/
//! `admission_result_for`: this crate's tests share one binary
//! (`cargo test --lib`), and `crate::spend`'s two process-global
//! `OnceLock`s would otherwise race whichever other test in that binary
//! installs or reads them next (`providers::metering`'s
//! `install_test_spend_globals` installs a real, process-wide policy for
//! its own wiring tests). Taking policy/ledger/now as plain parameters is
//! the same hazard-free split `check_with` exists for. [`handle_query`]
//! itself is two lines of pure delegation with nothing left to test beyond
//! what `handle_query_with`'s tests already cover.

use aleph_protocol::spend::{SpendQueryParams, SpendQueryResult, SpendRow};

use crate::config::types::policies::SpendPolicy;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::spend::SpendLedger;

/// `spend.query {}` → [`SpendQueryResult`]. The production entrypoint
/// registered against the gateway — see the module doc for why it reads
/// both process-global handles live rather than taking them as parameters.
pub async fn handle_query(request: JsonRpcRequest) -> JsonRpcResponse {
    let policy = crate::spend::current_policy();
    let ledger = crate::spend::global_ledger();
    let now_ms = chrono::Utc::now().timestamp_millis();
    handle_query_with(request, now_ms, &policy, ledger.as_ref())
}

/// The injectable core [`handle_query`] delegates to. See the module doc's
/// "two-function split" section for why every test in this module calls
/// this function directly.
///
/// The response is **built from** the contract type — `serde_json::to_value`
/// of a constructed [`SpendQueryResult`] — never a `json!` literal beside
/// it. That is what makes over-sending a compile-time impossibility instead
/// of an assertion somebody has to remember to write: the `workspace.get`
/// leak (four fields on the wire with no reader and no writer anywhere) got
/// there through a literal that parsed fine.
fn handle_query_with(
    request: JsonRpcRequest,
    now_ms: i64,
    policy: &SpendPolicy,
    ledger: &dyn SpendLedger,
) -> JsonRpcResponse {
    // Absent params is the default (and only) query, not a malformed one —
    // matches `handlers::security_audit::handle_query`, this handler's
    // model.
    let _params: SpendQueryParams = match request.params.clone() {
        None | Some(serde_json::Value::Null) => SpendQueryParams::default(),
        Some(v) => match serde_json::from_value(v) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("invalid params: {e}"),
                )
            }
        },
    };

    let period_start_ms = crate::spend::period::period_start_ms(now_ms, policy.period);
    let period_end_ms = crate::spend::period::period_end_ms(now_ms, policy.period);

    // `configured` comes from `policy.enabled()` and nothing else — spend is
    // recorded whether or not a ceiling is set, so the rows below are
    // returned regardless of this value (see `SpendQueryResult::configured`'s
    // doc). Rows are fetched unconditionally, not skipped when
    // `!policy.enabled()`: an unconfigured box still answers "what was
    // spent", it simply says no ceiling is acting on it.
    let rows = match ledger.principals_in(period_start_ms) {
        Ok(rows) => rows,
        // A ledger read failure is "I could not read the ledger", which
        // must never render as "nobody spent anything this period" — the
        // one reading that would let a broken query pass for a clean
        // window, the same failure `security.audit.query` guards against
        // for its own store.
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("failed to read spend ledger: {e}"),
            )
        }
    };

    let result = SpendQueryResult {
        configured: policy.enabled(),
        period_start_ms,
        period_end_ms,
        rows: rows
            .into_iter()
            .map(|(principal, spent)| SpendRow {
                principal: principal.as_key().to_string(),
                usd: spent.usd,
                unpriced_calls: spent.unpriced_calls,
                partial_calls: spent.partial_calls,
            })
            .collect(),
    };

    match serde_json::to_value(&result) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("failed to encode spend result: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;

    use super::*;
    use crate::config::types::policies::SpendPeriod;
    use crate::spend::{Delta, InMemorySpendLedger, Principal};

    fn req(params: Option<serde_json::Value>) -> JsonRpcRequest {
        JsonRpcRequest::with_id("spend.query", params, json!(1))
    }

    fn disabled_policy() -> SpendPolicy {
        SpendPolicy::default()
    }

    fn enabled_policy(period: SpendPeriod) -> SpendPolicy {
        SpendPolicy {
            per_user_usd: Some(10.0),
            total_usd: None,
            period,
        }
    }

    /// The keys a `T` actually serializes to, for the "derived from the
    /// contract type" key-set assertions the plan and addendum require —
    /// never written as a literal list, which would be the same
    /// enumeration bug the contract type itself exists to rule out one
    /// level up.
    fn keys_of<T: serde::Serialize>(value: &T) -> BTreeSet<String> {
        match serde_json::to_value(value).unwrap() {
            serde_json::Value::Object(map) => map.keys().cloned().collect(),
            other => panic!("expected a JSON object, got {other:?}"),
        }
    }

    fn keys_of_value(value: &serde_json::Value) -> BTreeSet<String> {
        match value {
            serde_json::Value::Object(map) => map.keys().cloned().collect(),
            other => panic!("expected a JSON object, got {other:?}"),
        }
    }

    // ========================================================================
    // Params handling
    // ========================================================================

    #[test]
    fn a_missing_params_object_is_the_default_query_not_an_error() {
        let ledger = InMemorySpendLedger::default();
        let resp = handle_query_with(req(None), 1_700_000_000_000, &disabled_policy(), &ledger);
        assert!(resp.is_success(), "{resp:?}");
    }

    #[test]
    fn a_null_params_value_is_the_default_query_not_an_error() {
        let ledger = InMemorySpendLedger::default();
        let resp = handle_query_with(
            req(Some(serde_json::Value::Null)),
            1_700_000_000_000,
            &disabled_policy(),
            &ledger,
        );
        assert!(resp.is_success(), "{resp:?}");
    }

    #[test]
    fn an_unknown_param_key_is_refused() {
        let ledger = InMemorySpendLedger::default();
        let resp = handle_query_with(
            req(Some(json!({"principal": "u-alice"}))),
            1_700_000_000_000,
            &disabled_policy(),
            &ledger,
        );
        assert!(
            resp.error.is_some(),
            "an unrecognised key must be refused, not silently ignored"
        );
    }

    // ========================================================================
    // `configured` — never derived from anything but `policy.enabled()`
    // ========================================================================

    /// An unconfigured box answers `configured: false`, never zeros, and it
    /// still returns the rows it has: spend is recorded whether or not a
    /// ceiling is set to act on it.
    #[test]
    fn an_unconfigured_box_answers_configured_false_and_still_returns_its_rows() {
        let ledger = InMemorySpendLedger::default();
        let alice = Principal::User("u-alice".to_string());
        let now_ms = 1_700_000_000_000;
        let period_start_ms = crate::spend::period::period_start_ms(now_ms, SpendPeriod::Month);
        ledger
            .record(&alice, period_start_ms, Delta::Usd(4.5))
            .unwrap();

        let resp = handle_query_with(req(None), now_ms, &disabled_policy(), &ledger);
        assert!(resp.is_success(), "{resp:?}");
        let result: SpendQueryResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(!result.configured, "no ceiling is set on either axis");
        assert_eq!(
            result.rows.len(),
            1,
            "a disabled ceiling must not hide recorded spend"
        );
        assert_eq!(result.rows[0].usd, 4.5);
    }

    #[test]
    fn a_configured_box_answers_configured_true() {
        let ledger = InMemorySpendLedger::default();
        let resp = handle_query_with(
            req(None),
            1_700_000_000_000,
            &enabled_policy(SpendPeriod::Month),
            &ledger,
        );
        assert!(resp.is_success(), "{resp:?}");
        let result: SpendQueryResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(result.configured);
    }

    // ========================================================================
    // Rows: attribution, precision fields, ordering
    // ========================================================================

    #[test]
    fn unattributed_spend_appears_as_its_own_row_never_hidden_or_folded_in() {
        let ledger = InMemorySpendLedger::default();
        let now_ms = 1_700_000_000_000;
        let period_start_ms = crate::spend::period::period_start_ms(now_ms, SpendPeriod::Month);
        ledger
            .record(&Principal::Unattributed, period_start_ms, Delta::Usd(1.25))
            .unwrap();

        let resp = handle_query_with(req(None), now_ms, &disabled_policy(), &ledger);
        let result: SpendQueryResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(
            result.rows.len(),
            1,
            "the `@unattributed` row must survive as an ordinary row. Filtering it \
             out — or folding its dollars into somebody else's total — makes spend \
             this surface cannot explain disappear from the report instead of \
             standing out in it, which is the whole reason the sentinel exists"
        );
        assert_eq!(result.rows[0].principal, "@unattributed");
        assert_eq!(result.rows[0].usd, 1.25);
    }

    #[test]
    fn unpriced_and_partial_call_counts_ride_through_to_the_row() {
        let ledger = InMemorySpendLedger::default();
        let alice = Principal::User("u-alice".to_string());
        let now_ms = 1_700_000_000_000;
        let period_start_ms = crate::spend::period::period_start_ms(now_ms, SpendPeriod::Month);
        ledger
            .record(&alice, period_start_ms, Delta::Partial(2.0))
            .unwrap();
        ledger
            .record(&alice, period_start_ms, Delta::Unpriced)
            .unwrap();
        ledger
            .record(&alice, period_start_ms, Delta::Unpriced)
            .unwrap();

        let resp = handle_query_with(req(None), now_ms, &disabled_policy(), &ledger);
        let result: SpendQueryResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].usd, 2.0);
        assert_eq!(result.rows[0].partial_calls, 1);
        assert_eq!(result.rows[0].unpriced_calls, 2);
    }

    /// The handler must not reorder what `SpendLedger::principals_in`
    /// already returned in the trait's contracted order (usd descending,
    /// then key ascending) — that ordering is tested at the ledger level in
    /// `spend::tests` and `spend::sqlite::tests`; this only guards the
    /// handler's own `Vec` mapping from silently losing it (e.g. collecting
    /// through an unordered container).
    #[test]
    fn row_order_from_the_ledger_survives_the_handlers_own_mapping() {
        let ledger = InMemorySpendLedger::default();
        let now_ms = 1_700_000_000_000;
        let period_start_ms = crate::spend::period::period_start_ms(now_ms, SpendPeriod::Month);
        let alice = Principal::User("u-alice".to_string());
        let bob = Principal::User("u-bob".to_string());
        let carol = Principal::User("u-carol".to_string());
        ledger
            .record(&carol, period_start_ms, Delta::Usd(5.0))
            .unwrap();
        ledger
            .record(&alice, period_start_ms, Delta::Usd(5.0))
            .unwrap();
        ledger
            .record(&bob, period_start_ms, Delta::Usd(9.0))
            .unwrap();

        let resp = handle_query_with(req(None), now_ms, &disabled_policy(), &ledger);
        let result: SpendQueryResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        let principals: Vec<&str> = result.rows.iter().map(|r| r.principal.as_str()).collect();
        assert_eq!(principals, vec!["u-bob", "u-alice", "u-carol"]);
    }

    #[test]
    fn a_period_with_no_spend_returns_an_empty_row_list_not_an_error() {
        let ledger = InMemorySpendLedger::default();
        let resp = handle_query_with(req(None), 1_700_000_000_000, &disabled_policy(), &ledger);
        assert!(resp.is_success(), "{resp:?}");
        let result: SpendQueryResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(result.rows.is_empty());
    }

    // ========================================================================
    // Period boundaries: wired from `policy.period`, not hardcoded
    // ========================================================================

    #[test]
    fn boundaries_bracket_now_for_both_periods() {
        let ledger = InMemorySpendLedger::default();
        let now_ms = 1_700_000_000_000;
        for period in [SpendPeriod::Day, SpendPeriod::Month] {
            let resp = handle_query_with(req(None), now_ms, &enabled_policy(period), &ledger);
            let result: SpendQueryResult = serde_json::from_value(resp.result.unwrap()).unwrap();
            assert!(
                result.period_start_ms <= now_ms,
                "{period:?}: start must not be after now"
            );
            assert!(
                result.period_end_ms > now_ms,
                "{period:?}: end must be after now"
            );
        }
    }

    /// Proves the handler threads `policy.period` through to
    /// `spend::period::period_start_ms`/`period_end_ms` rather than, say,
    /// always resolving `SpendPeriod::Month` — a `Day` policy and a `Month`
    /// policy queried at the same instant must disagree.
    #[test]
    fn a_day_policy_and_a_month_policy_disagree_on_the_boundary() {
        let ledger = InMemorySpendLedger::default();
        let now_ms = 1_700_000_000_000;

        let day_resp = handle_query_with(
            req(None),
            now_ms,
            &enabled_policy(SpendPeriod::Day),
            &ledger,
        );
        let day_result: SpendQueryResult =
            serde_json::from_value(day_resp.result.unwrap()).unwrap();
        let month_resp = handle_query_with(
            req(None),
            now_ms,
            &enabled_policy(SpendPeriod::Month),
            &ledger,
        );
        let month_result: SpendQueryResult =
            serde_json::from_value(month_resp.result.unwrap()).unwrap();

        assert_eq!(
            day_result.period_start_ms,
            crate::spend::period::period_start_ms(now_ms, SpendPeriod::Day)
        );
        assert_eq!(
            month_result.period_start_ms,
            crate::spend::period::period_start_ms(now_ms, SpendPeriod::Month)
        );
        assert_ne!(
            day_result.period_start_ms, month_result.period_start_ms,
            "a Day boundary and a Month boundary for the same instant must not coincide \
             (both dates were chosen so that a real host cannot make them collide)"
        );
    }

    // ========================================================================
    // Ledger failure
    // ========================================================================

    struct ErroringLedger;

    impl SpendLedger for ErroringLedger {
        fn record(&self, _: &Principal, _: i64, _: Delta) -> anyhow::Result<()> {
            unimplemented!("not exercised by these tests")
        }
        fn spent_for(&self, _: &Principal, _: i64) -> anyhow::Result<crate::spend::Spent> {
            unimplemented!("not exercised by these tests")
        }
        fn total_for(&self, _: i64, _: i64) -> anyhow::Result<crate::spend::Spent> {
            unimplemented!("not exercised by these tests")
        }
        fn sweep_before(&self, _: i64) -> anyhow::Result<usize> {
            unimplemented!("not exercised by these tests")
        }
        fn principals_in(&self, _: i64) -> anyhow::Result<Vec<(Principal, crate::spend::Spent)>> {
            anyhow::bail!("ErroringLedger: principals_in is unavailable")
        }
    }

    /// A ledger read failure must render as an error, never as an empty
    /// (and therefore indistinguishable from "nobody spent anything")
    /// report.
    #[test]
    fn a_ledger_read_failure_is_an_internal_error_not_an_empty_report() {
        let resp = handle_query_with(
            req(None),
            1_700_000_000_000,
            &disabled_policy(),
            &ErroringLedger,
        );
        assert!(
            resp.error.is_some(),
            "a broken ledger read must not report success"
        );
        assert!(
            resp.result.is_none(),
            "an errored response must not also carry a result — that would read as \
             success with an empty (and therefore falsely clean) window"
        );
    }

    // ========================================================================
    // Key-set equality against the contract type, both directions
    // ========================================================================

    /// Every key `SpendQueryResult` declares is present in the real
    /// handler's response, and the response contains no key the contract
    /// does not declare. The expected set is derived from the contract type
    /// itself (serialize a constructed value, take its keys) rather than
    /// written as a literal list — a literal list here would be the exact
    /// enumeration bug this contract type exists to close off one level up
    /// (see the module doc on `aleph_protocol::spend`).
    #[test]
    fn result_keys_match_the_contract_type_in_both_directions() {
        let ledger = InMemorySpendLedger::default();
        let alice = Principal::User("u-alice".to_string());
        let now_ms = 1_700_000_000_000;
        let period_start_ms = crate::spend::period::period_start_ms(now_ms, SpendPeriod::Month);
        // At least one row, so the `SpendRow` shape is exercised in the same
        // pass rather than needing a second round trip.
        ledger
            .record(&alice, period_start_ms, Delta::Usd(1.0))
            .unwrap();

        let resp = handle_query_with(
            req(None),
            now_ms,
            &enabled_policy(SpendPeriod::Month),
            &ledger,
        );
        assert!(resp.is_success(), "{resp:?}");
        let value = resp.result.unwrap();

        let expected_result_keys = keys_of(&SpendQueryResult {
            configured: true,
            period_start_ms: 0,
            period_end_ms: 0,
            rows: Vec::new(),
        });
        assert_eq!(
            keys_of_value(&value),
            expected_result_keys,
            "the real response must declare exactly the contract's keys — no fewer, no more"
        );

        let rows = value
            .get("rows")
            .and_then(|r| r.as_array())
            .expect("rows array");
        assert_eq!(
            rows.len(),
            1,
            "the row this test just recorded must come back"
        );
        let expected_row_keys = keys_of(&SpendRow {
            principal: String::new(),
            usd: 0.0,
            unpriced_calls: 0,
            partial_calls: 0,
        });
        assert_eq!(
            keys_of_value(&rows[0]),
            expected_row_keys,
            "the real row must declare exactly the contract's keys — no fewer, no more"
        );
    }
}
