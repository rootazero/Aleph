//! Tests for `spend::sqlite` — the durable `SpendLedger` backend.

use std::thread;

use super::*;
use crate::gateway::security::SecurityStore;
use crate::spend::{Delta, Principal};
use crate::sync_primitives::Arc;

fn ledger() -> SqliteSpendLedger {
    SqliteSpendLedger::for_test()
}

#[test]
fn record_and_spent_for_round_trip() {
    let ledger = ledger();
    let alice = Principal::User("u-alice".to_string());

    ledger.record(&alice, 1_000, Delta::Usd(2.5)).unwrap();

    let spent = ledger.spent_for(&alice, 1_000).unwrap();
    assert_eq!(spent.usd, 2.5);
    assert_eq!(spent.unpriced_calls, 0);
    assert_eq!(spent.partial_calls, 0);
}

#[test]
fn spent_for_an_unknown_principal_or_period_is_zero_not_an_error() {
    let ledger = ledger();
    let spent = ledger
        .spent_for(&Principal::User("u-nobody".to_string()), 1_000)
        .unwrap();
    assert_eq!(spent.usd, 0.0);
    assert_eq!(spent.unpriced_calls, 0);
    assert_eq!(spent.partial_calls, 0);
}

#[test]
fn record_upsert_accumulates_rather_than_replaces() {
    let ledger = ledger();
    let alice = Principal::User("u-alice".to_string());

    // Each `record` call moves exactly one `Delta` dimension (see
    // `Delta`'s doc), so reaching usd=3.5 / unpriced=1 / partial=1 takes
    // four calls. If `record` were read-modify-write instead of a single
    // UPSERT, the accumulation itself would still look right here — it is
    // `concurrent_record_from_n_threads_sums_exactly` below that a
    // read-modify-write shape cannot pass.
    ledger.record(&alice, 1_000, Delta::Usd(1.0)).unwrap();
    ledger.record(&alice, 1_000, Delta::Usd(2.0)).unwrap();
    ledger.record(&alice, 1_000, Delta::Partial(0.5)).unwrap();
    ledger.record(&alice, 1_000, Delta::Unpriced).unwrap();

    let spent = ledger.spent_for(&alice, 1_000).unwrap();
    assert_eq!(
        spent.usd, 3.5,
        "usd accumulates, it is not replaced by the last write"
    );
    assert_eq!(spent.unpriced_calls, 1);
    assert_eq!(spent.partial_calls, 1);
}

#[test]
fn record_keeps_distinct_periods_separate() {
    let ledger = ledger();
    let alice = Principal::User("u-alice".to_string());

    ledger.record(&alice, 1_000, Delta::Usd(5.0)).unwrap();
    ledger.record(&alice, 2_000, Delta::Usd(9.0)).unwrap();

    assert_eq!(ledger.spent_for(&alice, 1_000).unwrap().usd, 5.0);
    assert_eq!(ledger.spent_for(&alice, 2_000).unwrap().usd, 9.0);
}

#[test]
fn unpriced_delta_increments_the_counter_and_leaves_usd_at_exactly_zero() {
    let ledger = ledger();
    let alice = Principal::User("u-alice".to_string());

    ledger.record(&alice, 1_000, Delta::Unpriced).unwrap();
    ledger.record(&alice, 1_000, Delta::Unpriced).unwrap();

    let spent = ledger.spent_for(&alice, 1_000).unwrap();
    assert_eq!(
        spent.usd, 0.0,
        "an unpriced call must never move a dollar figure"
    );
    assert_eq!(spent.unpriced_calls, 2);
    assert_eq!(spent.partial_calls, 0);
}

/// The durable backend must implement the same fail-closed window rule as
/// the in-memory one (spend I-1 — see the trait method's doc): rows keyed
/// inside the window count, the coarse-ancestor row counts, and an older
/// non-ancestor row stays out.
#[test]
fn total_for_sums_the_window_rule_and_excludes_older_non_ancestor_rows() {
    let ledger = ledger();
    let alice = Principal::User("u-alice".to_string());
    let bob = Principal::User("u-bob".to_string());
    let carol = Principal::User("u-carol".to_string());

    // Window-start rows: the steady-state case.
    ledger.record(&alice, 1_000, Delta::Usd(1.0)).unwrap();
    ledger.record(&bob, 1_000, Delta::Usd(2.0)).unwrap();
    ledger.record(&carol, 1_000, Delta::Usd(3.0)).unwrap();
    ledger.record(&bob, 1_000, Delta::Unpriced).unwrap();
    ledger.record(&carol, 1_000, Delta::Partial(0.0)).unwrap();
    // A finer boundary inside the window — e.g. a day-keyed row recorded
    // before a Day → Month switch. Must count.
    ledger.record(&alice, 2_000, Delta::Usd(100.0)).unwrap();
    // The coarse-ancestor boundary — e.g. a month-keyed row before a
    // Month → Day switch. Must count (the deliberate over-count).
    ledger.record(&carol, 100, Delta::Usd(7.0)).unwrap();
    // An old row that is neither inside the window nor the coarse
    // ancestor: must NOT count.
    ledger.record(&alice, 500, Delta::Usd(50.0)).unwrap();

    let total = ledger.total_for(1_000, 100).unwrap();
    assert_eq!(total.usd, 113.0, "1 + 2 + 3 + 100 + 7; the 500 row is out");
    assert_eq!(total.unpriced_calls, 1);
    assert_eq!(total.partial_calls, 1);
    assert_eq!(total.period_start_ms, 1_000);
}

/// spend I-1 on the durable backend, both switch directions: rows
/// recorded under a Day policy must keep counting toward a Month window
/// (the `>=` arm), and a row recorded under a Month policy must keep
/// counting toward a Day window (the coarse-ancestor arm) — keyed on real
/// local-calendar boundaries, the same values `check_with` would compute.
#[test]
fn total_for_survives_a_policy_switch_in_both_directions() {
    use crate::config::types::policies::SpendPeriod;
    let now_ms = 1_700_000_000_000i64; // mid-month in any timezone
    let month_start = crate::spend::period::period_start_ms(now_ms, SpendPeriod::Month);
    let day_start = crate::spend::period::period_start_ms(now_ms, SpendPeriod::Day);
    assert!(month_start < day_start, "test setup: now must be mid-month");

    // Day → Month: the day-keyed row lies inside the month window.
    let ledger = ledger();
    let alice = Principal::User("u-alice".to_string());
    ledger.record(&alice, day_start, Delta::Usd(3.0)).unwrap();
    let total = ledger.total_for(month_start, month_start).unwrap();
    assert_eq!(
        total.usd, 3.0,
        "after a Day → Month switch the month total must not read zero"
    );

    // Month → Day: the month-keyed row is the coarse ancestor of the day
    // window (the deliberate fail-closed over-count).
    let ledger = ledger();
    ledger.record(&alice, month_start, Delta::Usd(40.0)).unwrap();
    let total = ledger.total_for(day_start, month_start).unwrap();
    assert_eq!(
        total.usd, 40.0,
        "after a Month → Day switch the day total must keep the month-keyed row"
    );
    // …and it must NOT leak into a day window of a different month, where
    // the row's period no longer contains the queried window.
    let next_month_start = crate::spend::period::period_end_ms(now_ms, SpendPeriod::Month);
    let next_day_start = crate::spend::period::period_start_ms(next_month_start, SpendPeriod::Day);
    let total = ledger
        .total_for(next_day_start, next_month_start)
        .unwrap();
    assert_eq!(
        total.usd, 0.0,
        "the ancestor arm must not reach into a month the row does not cover"
    );
}

/// `SUM()` over zero matching rows is `NULL` in SQLite, not `0` — this pins
/// that the ledger converts it rather than letting a `NULL` become an error
/// or an `Option` the caller has to unwrap.
#[test]
fn total_for_an_empty_period_is_zero_not_an_error() {
    let ledger = ledger();
    let total = ledger.total_for(1_000, 100).unwrap();
    assert_eq!(total.usd, 0.0);
    assert_eq!(total.unpriced_calls, 0);
    assert_eq!(total.partial_calls, 0);
}

#[test]
fn sweep_before_leaves_the_current_period_alone() {
    let ledger = ledger();
    let alice = Principal::User("u-alice".to_string());

    ledger.record(&alice, 1_000, Delta::Usd(1.0)).unwrap();
    ledger.record(&alice, 2_000, Delta::Usd(2.0)).unwrap();
    ledger.record(&alice, 3_000, Delta::Usd(3.0)).unwrap();

    let removed = ledger.sweep_before(2_000).unwrap();
    assert_eq!(
        removed, 1,
        "only the 1_000 row is strictly before the cutoff"
    );

    assert_eq!(ledger.spent_for(&alice, 1_000).unwrap().usd, 0.0);
    assert_eq!(
        ledger.spent_for(&alice, 2_000).unwrap().usd,
        2.0,
        "the cutoff period itself survives"
    );
    assert_eq!(ledger.spent_for(&alice, 3_000).unwrap().usd, 3.0);
}

/// A swept row must not go on answering from a stale cache entry — the
/// write-through cache and the table it fronts have to agree after a
/// delete, the same way they have to agree after an insert.
#[test]
fn sweep_before_evicts_the_cache_for_swept_periods() {
    let ledger = ledger();
    let alice = Principal::User("u-alice".to_string());
    ledger.record(&alice, 1_000, Delta::Usd(1.0)).unwrap();
    ledger.record(&alice, 2_000, Delta::Usd(2.0)).unwrap();

    ledger.sweep_before(2_000).unwrap();

    assert_eq!(
        ledger.spent_for(&alice, 1_000).unwrap().usd,
        0.0,
        "a cached entry for a swept period must not paper over the delete"
    );
    assert_eq!(ledger.spent_for(&alice, 2_000).unwrap().usd, 2.0);
}

#[test]
fn principals_in_orders_by_usd_descending_then_key_ascending() {
    let ledger = ledger();
    let alice = Principal::User("u-alice".to_string());
    let bob = Principal::User("u-bob".to_string());
    let carol = Principal::User("u-carol".to_string());

    // Two principals tie on `usd` — must break by key, not by insertion
    // order, or the CLI table would reshuffle between two calls on
    // unchanged data.
    ledger.record(&carol, 1_000, Delta::Usd(5.0)).unwrap();
    ledger.record(&alice, 1_000, Delta::Usd(5.0)).unwrap();
    ledger.record(&bob, 1_000, Delta::Usd(9.0)).unwrap();
    // A different period must not appear in this window's rows.
    ledger.record(&alice, 2_000, Delta::Usd(100.0)).unwrap();

    let rows = ledger.principals_in(1_000).unwrap();
    let keys: Vec<&str> = rows.iter().map(|(p, _)| p.as_key()).collect();
    assert_eq!(
        keys,
        vec!["u-bob", "u-alice", "u-carol"],
        "9.0 first, then the 5.0 tie broken by key ascending"
    );
    for (_, spent) in &rows {
        assert_eq!(
            spent.period_end_ms, None,
            "the ledger does not know period length — see Spent::period_end_ms's doc"
        );
    }
}

#[test]
fn principals_in_reconstructs_the_unattributed_sentinel() {
    let ledger = ledger();
    ledger
        .record(&Principal::Unattributed, 1_000, Delta::Usd(1.0))
        .unwrap();

    let rows = ledger.principals_in(1_000).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, Principal::Unattributed);
}

#[test]
fn principals_in_is_empty_for_a_period_with_no_rows() {
    let ledger = ledger();
    assert!(ledger.principals_in(1_000).unwrap().is_empty());
}

/// `principals_in` must read the table, not the write-through cache: a
/// second `SqliteSpendLedger` over the same store starts with an empty
/// cache and must still see rows the first instance wrote — see the
/// method's doc for why enumeration is deliberately not cached.
#[test]
fn principals_in_sees_rows_written_by_a_sibling_ledger_with_a_cold_cache() {
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let writer = SqliteSpendLedger::new(store.clone());
    let alice = Principal::User("u-alice".to_string());
    writer.record(&alice, 1_000, Delta::Usd(3.0)).unwrap();

    let reader = SqliteSpendLedger::new(store);
    let rows = reader.principals_in(1_000).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, alice);
    assert_eq!(rows[0].1.usd, 3.0);
}

/// The reason `record` has to be a single UPSERT rather than
/// read-modify-write: this is the test a read-modify-write shape cannot
/// pass, because two threads' read of the old total can interleave with
/// each other's write of the new one and one increment vanishes.
#[test]
fn concurrent_record_from_n_threads_sums_exactly() {
    let ledger = Arc::new(ledger());
    let alice = Principal::User("u-alice".to_string());
    const THREADS: usize = 16;
    const PER_THREAD: usize = 50;
    let barrier = Arc::new(std::sync::Barrier::new(THREADS));

    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let ledger = Arc::clone(&ledger);
            let alice = alice.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..PER_THREAD {
                    ledger.record(&alice, 1_000, Delta::Usd(1.0)).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let spent = ledger.spent_for(&alice, 1_000).unwrap();
    assert_eq!(
        spent.usd,
        (THREADS * PER_THREAD) as f64,
        "an UPSERT serialized through a single connection lock must not lose any of the {} concurrent increments",
        THREADS * PER_THREAD
    );
}

/// The half `for_test()` cannot exercise: a real, file-backed store closed
/// and reopened, with a brand-new `SqliteSpendLedger` (and therefore a cold,
/// empty cache) reading the row back through the table alone.
#[test]
fn durability_survives_a_close_and_reopen_of_the_store() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("security.db");
    let alice = Principal::User("u-alice".to_string());

    {
        let store = Arc::new(SecurityStore::open(&db_path).unwrap());
        let ledger = SqliteSpendLedger::new(store);
        ledger.record(&alice, 1_000, Delta::Usd(4.25)).unwrap();
        // `ledger` and `store` drop here, closing the connection.
    }

    let store = Arc::new(SecurityStore::open(&db_path).unwrap());
    let ledger = SqliteSpendLedger::new(store);
    let spent = ledger.spent_for(&alice, 1_000).unwrap();
    assert_eq!(
        spent.usd, 4.25,
        "a fresh instance's cold cache must read the durable row through"
    );
}

/// Pin the new `record` shape: the UPSERT and the read-back are a single
/// `INSERT ... RETURNING` statement, so the cache must be populated from
/// the values that statement returned (not from a follow-up SELECT that
/// could fail and leave the cache stale). The previous two-statement shape
/// had a window where the row was durably written but the read-back
/// failed; this test guards against regressing to that shape by asserting
/// that a single `record` call both commits to the table AND populates the
/// cache with the *accumulated* totals — which only a RETURNING round-trip
/// can deliver without a second query.
#[test]
fn record_upsert_succeeds_and_cache_populated_via_returning() {
    let ledger = ledger();
    let alice = Principal::User("u-alice".to_string());

    // First write: cache miss before, populated from RETURNING after.
    ledger.record(&alice, 1_000, Delta::Usd(1.0)).unwrap();
    let spent = ledger.spent_for(&alice, 1_000).unwrap();
    assert_eq!(
        spent.usd, 1.0,
        "cache populated from RETURNING on first write"
    );
    assert_eq!(spent.unpriced_calls, 0);
    assert_eq!(spent.partial_calls, 0);

    // Second write on the same key+period: triggers the ON CONFLICT arm.
    // If `record` had fallen back to UPSERT-then-SELECT and that SELECT
    // returned the pre-update row (or no row, depending on timing), the
    // cache could read 1.0 here instead of the correct 1.0 + 2.5 = 3.5.
    // RETURNING delivers the post-update totals, so 3.5 is what the cache
    // must hold.
    ledger.record(&alice, 1_000, Delta::Usd(2.5)).unwrap();
    let spent = ledger.spent_for(&alice, 1_000).unwrap();
    assert_eq!(
        spent.usd, 3.5,
        "cache must reflect the post-update total, not the pre-update one"
    );

    // A different Delta dimension also accumulates correctly — same
    // RETURNING contract covers all three `Delta` arms.
    ledger.record(&alice, 1_000, Delta::Partial(0.25)).unwrap();
    ledger.record(&alice, 1_000, Delta::Unpriced).unwrap();
    let spent = ledger.spent_for(&alice, 1_000).unwrap();
    assert_eq!(spent.usd, 3.75);
    assert_eq!(spent.unpriced_calls, 1);
    assert_eq!(spent.partial_calls, 1);
}
