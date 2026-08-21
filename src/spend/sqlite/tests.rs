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
    assert_eq!(spent.usd, 3.5, "usd accumulates, it is not replaced by the last write");
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
    assert_eq!(spent.usd, 0.0, "an unpriced call must never move a dollar figure");
    assert_eq!(spent.unpriced_calls, 2);
    assert_eq!(spent.partial_calls, 0);
}

#[test]
fn total_for_sums_three_principals_and_ignores_other_periods() {
    let ledger = ledger();
    let alice = Principal::User("u-alice".to_string());
    let bob = Principal::User("u-bob".to_string());
    let carol = Principal::User("u-carol".to_string());

    ledger.record(&alice, 1_000, Delta::Usd(1.0)).unwrap();
    ledger.record(&bob, 1_000, Delta::Usd(2.0)).unwrap();
    ledger.record(&carol, 1_000, Delta::Usd(3.0)).unwrap();
    ledger.record(&bob, 1_000, Delta::Unpriced).unwrap();
    ledger.record(&carol, 1_000, Delta::Partial(0.0)).unwrap();
    // A different period must not contribute to the 1_000 total.
    ledger.record(&alice, 2_000, Delta::Usd(100.0)).unwrap();

    let total = ledger.total_for(1_000).unwrap();
    assert_eq!(total.usd, 6.0);
    assert_eq!(total.unpriced_calls, 1);
    assert_eq!(total.partial_calls, 1);
}

/// `SUM()` over zero matching rows is `NULL` in SQLite, not `0` — this pins
/// that the ledger converts it rather than letting a `NULL` become an error
/// or an `Option` the caller has to unwrap.
#[test]
fn total_for_an_empty_period_is_zero_not_an_error() {
    let ledger = ledger();
    let total = ledger.total_for(1_000).unwrap();
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
    assert_eq!(removed, 1, "only the 1_000 row is strictly before the cutoff");

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
