//! Tests for `spend::mod` — the core types, the in-process ledger default,
//! `principal_from_metadata`, and `check` (via its injectable core,
//! `check_with` — see that function's doc for why its tests never call
//! `install_ledger`/`install_policy`). `ambient_principal`'s equivalence
//! with `principal_from_metadata` (G13) lives in
//! `gateway::execution_engine::run_loop::tests`, next to
//! `with_request_scope`, because it needs that function's `pub(super)`
//! visibility — see this round's task-3 brief for why widening that
//! visibility instead was rejected.

use std::collections::HashMap;

use super::*;

// ============================================================================
// Principal
// ============================================================================

#[test]
fn as_key_user_returns_the_id_verbatim() {
    assert_eq!(Principal::User("u-alice".to_string()).as_key(), "u-alice");
}

#[test]
fn as_key_unattributed_returns_the_reserved_sentinel() {
    assert_eq!(Principal::Unattributed.as_key(), "@unattributed");
}

#[test]
fn from_key_is_the_inverse_of_as_key_for_a_user() {
    assert_eq!(
        Principal::from_key("u-alice"),
        Principal::User("u-alice".to_string())
    );
}

#[test]
fn from_key_recognises_the_unattributed_sentinel() {
    assert_eq!(
        Principal::from_key("@unattributed"),
        Principal::Unattributed
    );
}

/// `Principal::user` is the safe construction path the untrusted-input
/// resolvers route through: any reserved sentinel collapses to
/// `Unattributed` rather than producing a `User(String)` whose `as_key`
/// shares the `principal_id` row space with the sentinel variant.
#[test]
fn principal_user_with_unattributed_id_round_trips_to_unattributed() {
    // The literal sentinel — the exact collision the constructor exists to
    // prevent. Without the coercion, `Principal::user("@unattributed").as_key()`
    // would equal `Principal::Unattributed.as_key()`, and a row written by
    // one variant would be readable as the other via `from_key`.
    assert_eq!(
        Principal::user("@unattributed"),
        Principal::Unattributed,
        "the literal sentinel must coerce to Unattributed, not become a User with that key"
    );

    // Any `@`-prefixed id — same reservation reasoning. A future caller
    // that stamps `author_user_id = "@admin"` should not be able to write
    // to a `principal_id` row whose first character is `@`, because the
    // `users.user_id` shape is `u-`-prefixed.
    assert_eq!(Principal::user("@admin"), Principal::Unattributed);
    assert_eq!(Principal::user("@bot"), Principal::Unattributed);

    // An empty id has no user to charge; coerce it to the sentinel rather
    // than let it ride as a zero-length `principal_id` PRIMARY KEY string
    // (the SQLite store would happily accept it).
    assert_eq!(Principal::user(""), Principal::Unattributed);

    // A valid `u-`-prefixed id passes through unchanged — the constructor
    // must not reject the shape it exists to protect, or every existing
    // call site would have to grow a fallback arm.
    assert_eq!(
        Principal::user("u-alice"),
        Principal::User("u-alice".to_string())
    );
}

/// Pin the round-trip: a `User` constructed from the literal sentinel must
/// not be distinguishable from `Unattributed` after a `from_key` round trip
/// (the read path every `principals_in` implementation uses). This is the
/// concrete failure mode the constructor exists to rule out: silently
/// misreporting spend charged to a user named `@unattributed` as
/// `Principal::Unattributed`.
#[test]
fn principal_user_constructor_does_not_create_a_colliding_user() {
    // Both paths resolve to the same `principal_id` text — but only one
    // is the sentinel variant. The constructor's coercion is what keeps
    // them the same variant, which is what makes `from_key` round-trip
    // safe regardless of which path wrote the row.
    let user_via_constructor = Principal::user("@unattributed");
    let direct_sentinel = Principal::Unattributed;
    assert_eq!(user_via_constructor.as_key(), direct_sentinel.as_key());
    assert_eq!(
        user_via_constructor, direct_sentinel,
        "the constructor must coerce, not produce a User(String) with the sentinel key"
    );
}

/// `principal_from_metadata` routes through `Principal::user`, so an
/// `AUTHOR_USER_KEY` stamped as the sentinel must resolve to `Unattributed`
/// — the untrusted-input half of the same reservation.
#[test]
fn principal_from_metadata_author_stamped_as_unattributed_resolves_to_unattributed() {
    let mut meta = HashMap::new();
    meta.insert(
        crate::gateway::execution_engine::AUTHOR_USER_KEY.to_string(),
        "@unattributed".to_string(),
    );

    assert_eq!(
        principal_from_metadata(&meta),
        Principal::Unattributed,
        "AUTHOR_USER_KEY = \"@unattributed\" must coerce to Unattributed, not become \
         Principal::User(\"@unattributed\")"
    );
}

// ============================================================================
// principal_from_metadata
// ============================================================================

#[test]
fn principal_from_metadata_prefers_author_over_owner() {
    let mut meta = HashMap::new();
    meta.insert(
        crate::gateway::execution_engine::AUTHOR_USER_KEY.to_string(),
        "u-speaker".to_string(),
    );
    meta.insert(
        crate::scope::OWNER_META_KEY.to_string(),
        "u-room-owner".to_string(),
    );

    assert_eq!(
        principal_from_metadata(&meta),
        Principal::User("u-speaker".to_string())
    );
}

#[test]
fn principal_from_metadata_falls_back_to_owner_when_no_author() {
    // No AUTHOR_USER_KEY. The owner fallback routes through
    // `scope::scope_from_metadata`, which requires OWNER_META_KEY and
    // SCOPE_META_KEY together — the shape `stamp_metadata` always produces —
    // so this builds both rather than the bare owner key alone. See
    // `principal_from_metadata_is_unattributed_when_the_owner_key_has_no_scope_key`
    // below for the asymmetric shape this distinction exists to catch.
    let mut meta = HashMap::new();
    crate::scope::stamp_metadata(
        &mut meta,
        &crate::scope::ScopeAttribution::personal("u-room-owner"),
    );

    assert_eq!(
        principal_from_metadata(&meta),
        Principal::User("u-room-owner".to_string())
    );
}

#[test]
fn principal_from_metadata_is_unattributed_when_the_owner_key_has_no_scope_key() {
    // The asymmetric shape no known producer writes (stamp_metadata always
    // writes OWNER_META_KEY and SCOPE_META_KEY together) but that the type
    // system does not rule out. Before this resolver routed through
    // `scope::scope_from_metadata`, a bare `meta.get(OWNER_META_KEY)` would
    // have resolved `Principal::User` here — diverging from the floor arm,
    // which reads `None` for this exact shape. Fails closed instead: see
    // `run_loop::tests::spend_principal_resolvers_agree_on_an_owner_key_with_no_scope_key`
    // for the two-arm version of this same case.
    let mut meta = HashMap::new();
    meta.insert(
        crate::scope::OWNER_META_KEY.to_string(),
        "u-room-owner".to_string(),
    );

    assert_eq!(principal_from_metadata(&meta), Principal::Unattributed);
}

#[test]
fn principal_from_metadata_is_unattributed_when_neither_key_is_present() {
    assert_eq!(
        principal_from_metadata(&HashMap::new()),
        Principal::Unattributed
    );
}

// ============================================================================
// ambient_principal
// ============================================================================

#[tokio::test]
async fn ambient_principal_reads_the_seeded_room_author() {
    let observed = crate::scope::with_room_author(Some("u-speaker".to_string()), async {
        ambient_principal()
    })
    .await;
    assert_eq!(observed, Principal::User("u-speaker".to_string()));
}

#[tokio::test]
async fn ambient_principal_falls_back_to_ambient_owner_when_no_room_author_is_seeded() {
    let observed = crate::scope::with_scope(
        Some(crate::scope::ScopeAttribution::personal("u-owner")),
        async { ambient_principal() },
    )
    .await;
    assert_eq!(observed, Principal::User("u-owner".to_string()));
}

#[tokio::test]
async fn ambient_principal_is_unattributed_with_nothing_ambient() {
    // No `with_room_author` / `with_scope` wrap at all: both task-locals are
    // unset, matching an unattended path (cron, A2A) that never seeded
    // either fact.
    assert_eq!(ambient_principal(), Principal::Unattributed);
}

// ============================================================================
// InMemorySpendLedger
// ============================================================================

#[test]
fn record_accumulates_across_calls_for_the_same_principal_and_period() {
    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());

    // Each `record` call moves exactly one `Delta` dimension, so reaching
    // usd=3.75 / unpriced=1 / partial=2 takes four calls, not two.
    ledger.record(&alice, 1_000, Delta::Usd(1.5)).unwrap();
    ledger.record(&alice, 1_000, Delta::Unpriced).unwrap();
    ledger.record(&alice, 1_000, Delta::Partial(1.0)).unwrap();
    ledger.record(&alice, 1_000, Delta::Partial(1.25)).unwrap();

    let spent = ledger.spent_for(&alice, 1_000).unwrap();
    assert_eq!(spent.usd, 3.75);
    assert_eq!(spent.unpriced_calls, 1);
    assert_eq!(spent.partial_calls, 2);
}

/// Regression for `severed-wire-2026-09-05-modules2 spend I-2`: NaN /
/// ±inf in `Delta::Usd` or `Delta::Partial` used to land in the row
/// verbatim. `ceiling_blown(NaN, anything)` is always false (IEEE 754),
/// so a single corrupt price silently disabled the spend ceiling for
/// the rest of the period. The InMemory backend now coerces to 0.0 and
/// bumps `unpriced_calls`, so the 'this call had a price it couldn't
/// represent' signal stays loud and the ceiling continues to evaluate
/// against a real number.
#[test]
fn record_replaces_non_finite_usd_with_unpriced_and_keeps_ceiling_alive() {
    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());

    ledger.record(&alice, 1_000, Delta::Usd(f64::NAN)).unwrap();
    ledger
        .record(&alice, 1_000, Delta::Partial(f64::INFINITY))
        .unwrap();
    ledger
        .record(&alice, 1_000, Delta::Partial(f64::NEG_INFINITY))
        .unwrap();

    let spent = ledger.spent_for(&alice, 1_000).unwrap();
    assert!(
        spent.usd.is_finite(),
        "usd column must never carry NaN/inf (got {})",
        spent.usd
    );
    assert_eq!(spent.usd, 0.0);
    assert_eq!(spent.unpriced_calls, 3);
    assert_eq!(spent.partial_calls, 0);
}

#[test]
fn record_keeps_distinct_periods_separate() {
    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());

    ledger.record(&alice, 1_000, Delta::Usd(5.0)).unwrap();
    ledger.record(&alice, 2_000, Delta::Usd(9.0)).unwrap();

    assert_eq!(ledger.spent_for(&alice, 1_000).unwrap().usd, 5.0);
    assert_eq!(ledger.spent_for(&alice, 2_000).unwrap().usd, 9.0);
}

#[test]
fn spent_for_an_unknown_principal_or_period_is_zero_not_an_error() {
    let ledger = InMemorySpendLedger::default();
    let spent = ledger
        .spent_for(&Principal::User("u-nobody".to_string()), 1_000)
        .unwrap();
    assert_eq!(spent.usd, 0.0);
    assert_eq!(spent.unpriced_calls, 0);
    assert_eq!(spent.partial_calls, 0);
}

/// The window rule `total_for` implements (spend I-1 — see the trait
/// method's doc): a row counts iff its period start is `>=` the queried
/// window start (steady-state rows and rows recorded under a finer old
/// policy inside the window) OR equals the coarse-ancestor boundary (rows
/// recorded under a coarser old policy whose window still contains this
/// one). An older row that is NOT the coarse ancestor stays out.
#[test]
fn total_for_sums_the_window_rule_and_excludes_older_non_ancestor_rows() {
    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());
    let bob = Principal::User("u-bob".to_string());
    let carol = Principal::User("u-carol".to_string());

    // Window-start rows: the steady-state case.
    ledger.record(&alice, 1_000, Delta::Usd(1.0)).unwrap();
    ledger.record(&bob, 1_000, Delta::Usd(2.0)).unwrap();
    for _ in 0..3 {
        ledger.record(&bob, 1_000, Delta::Unpriced).unwrap();
    }
    for _ in 0..4 {
        ledger.record(&bob, 1_000, Delta::Partial(0.0)).unwrap();
    }
    // A finer boundary inside the window — e.g. a day-keyed row recorded
    // before a Day → Month switch. Must count.
    ledger.record(&alice, 2_000, Delta::Usd(100.0)).unwrap();
    // The coarse-ancestor boundary, older than the window but still the
    // start of the coarsest period containing it — e.g. a month-keyed row
    // before a Month → Day switch. Must count (the deliberate over-count).
    ledger.record(&carol, 100, Delta::Usd(7.0)).unwrap();
    // An old row that is neither inside the window nor the coarse
    // ancestor: must NOT count, or the ceiling would fire on spend that
    // no live interpretation of the window covers.
    ledger.record(&alice, 500, Delta::Usd(50.0)).unwrap();

    let total = ledger.total_for(1_000, 100).unwrap();
    assert_eq!(total.usd, 110.0, "1 + 2 + 100 + 7; the 500 row is out");
    assert_eq!(total.unpriced_calls, 3);
    assert_eq!(total.partial_calls, 4);
    assert_eq!(
        total.period_start_ms, 1_000,
        "the queried window start rides the answer, as before"
    );
}

/// spend I-1, the `Day → Month` direction, on real period boundaries:
/// spend recorded while the policy was `Day` is keyed at day starts, so
/// after a hot switch to `Month` an exact-match total reads zero and the
/// machine ceiling silently stops firing for the rest of the month. The
/// `>= window_start` arm keeps those rows counting.
#[test]
fn total_for_after_a_day_to_month_policy_switch_still_counts_this_months_day_rows() {
    let now_ms = 1_700_000_000_000i64; // mid-month in any timezone (Nov 13..15 local)
    let month_start = period::period_start_ms(now_ms, SpendPeriod::Month);
    let day_start = period::period_start_ms(now_ms, SpendPeriod::Day);
    assert!(
        day_start > month_start,
        "test setup: now must be mid-month, so the day boundary is strictly \
         inside the month window"
    );

    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());
    // Recorded while the policy was Day — keyed at the day boundary.
    ledger.record(&alice, day_start, Delta::Usd(3.0)).unwrap();

    // After the switch, `check_with` queries (month_start, month_start):
    // window start and coarse ancestor coincide under a Month policy.
    let total = ledger.total_for(month_start, month_start).unwrap();
    assert_eq!(
        total.usd, 3.0,
        "the day-keyed row lies inside the month window and must keep counting"
    );
}

/// spend I-1, the `Month → Day` direction: spend recorded while the
/// policy was `Month` is keyed at the month start, which is strictly
/// BEFORE the day window's start — only the coarse-ancestor arm keeps it
/// counting toward today's total. This is the deliberate fail-closed
/// over-count: the month row covers the whole month, not just today.
#[test]
fn total_for_after_a_month_to_day_policy_switch_still_counts_the_month_row() {
    let now_ms = 1_700_000_000_000i64;
    let month_start = period::period_start_ms(now_ms, SpendPeriod::Month);
    let day_start = period::period_start_ms(now_ms, SpendPeriod::Day);
    assert!(month_start < day_start, "test setup: now must be mid-month");

    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());
    // Recorded while the policy was Month — keyed at the month boundary.
    ledger
        .record(&alice, month_start, Delta::Usd(40.0))
        .unwrap();

    let total = ledger.total_for(day_start, month_start).unwrap();
    assert_eq!(
        total.usd, 40.0,
        "the coarse-ancestor arm keeps the month-keyed row in today's total"
    );

    // Without the ancestor arm — i.e. if a caller passed a non-matching
    // ancestor — the same query reads zero, which is the I-1 hole itself.
    let hole = ledger.total_for(day_start, 42).unwrap();
    assert_eq!(
        hole.usd, 0.0,
        "test setup: an ancestor that matches no row reproduces the original hole"
    );
}

/// The ceiling-stays-armed property at the `check_with` level: record
/// spend under a Day policy, hot-switch the policy to Month, and the
/// machine total ceiling must still fire on the day-keyed spend.
#[test]
fn a_day_to_month_policy_switch_keeps_the_total_ceiling_armed() {
    let now_ms = 1_700_000_000_000i64;
    let day_start = period::period_start_ms(now_ms, SpendPeriod::Day);

    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());
    // Recorded while the policy was Day.
    ledger.record(&alice, day_start, Delta::Usd(60.0)).unwrap();

    // Hot-switched to Month, total ceiling below the recorded spend.
    let policy = SpendPolicy {
        total_usd: Some(50.0),
        period: SpendPeriod::Month,
        ..SpendPolicy::default()
    };
    match check_with(&alice, now_ms, &policy, &ledger) {
        Verdict::Denied {
            limit: Limit::Total,
            ..
        } => {}
        other => panic!(
            "the machine ceiling must still fire on spend recorded under the old Day policy; \
             got {other:?}"
        ),
    }
}

/// The `Month → Day` counterpart at the `check_with` level: the
/// month-keyed row counts toward today's machine total (the deliberate
/// over-count), so the ceiling still fires after the switch.
#[test]
fn a_month_to_day_policy_switch_keeps_the_total_ceiling_armed() {
    let now_ms = 1_700_000_000_000i64;
    let month_start = period::period_start_ms(now_ms, SpendPeriod::Month);

    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());
    // Recorded while the policy was Month.
    ledger
        .record(&alice, month_start, Delta::Usd(60.0))
        .unwrap();

    let policy = SpendPolicy {
        total_usd: Some(50.0),
        period: SpendPeriod::Day,
        ..SpendPolicy::default()
    };
    match check_with(&alice, now_ms, &policy, &ledger) {
        Verdict::Denied {
            limit: Limit::Total,
            ..
        } => {}
        other => panic!(
            "the machine ceiling must still fire on spend recorded under the old Month policy; \
             got {other:?}"
        ),
    }
}

/// Guard for the spend I-3 index: `by_period` is a second data structure
/// maintained alongside `rows` at every mutation site, and this test is
/// what keeps the two from drifting — a deterministic pseudo-random
/// sequence of records and sweeps is applied to the ledger, and after
/// every op (a) the index must enumerate exactly the row map's keys, and
/// (b) both indexed reads (`total_for`, `principals_in`) must agree with
/// a naive full-scan reference computed over the same rows.
///
/// Deterministic by construction: a seeded xorshift64* PRNG, a fixed
/// principal set, a fixed period grid, no wall clock and no `rand`
/// dependency — the same sequence on every run, so a failure is
/// reproducible byte-for-byte. Dollar amounts are whole integers so the
/// reference sum and the ledger sum are order-insensitive (f64 addition
/// is exact for integers well past these magnitudes; non-integer cents
/// would make the two iteration orders legitimately disagree in the last
/// ulp and the assertion would be about summation order, not the index).
#[test]
fn the_by_period_index_never_drifts_from_the_rows_across_random_ops() {
    let mut rng_state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        // xorshift64* — tiny, deterministic, good enough for a fuzz loop.
        rng_state ^= rng_state >> 12;
        rng_state ^= rng_state << 25;
        rng_state ^= rng_state >> 27;
        rng_state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    let ledger = InMemorySpendLedger::default();
    let principals = ["u-alice", "u-bob", "u-carol"];
    // A period grid where 0 doubles as the coarse-ancestor boundary, so
    // the ancestor arm of the window rule is exercised alongside `>=`.
    let period_grid = [0i64, 100, 200, 300, 400];
    // The reference model: `(principal key, period start)` -> usd total.
    let mut reference: HashMap<(String, i64), f64> = HashMap::new();

    for step in 0..500u32 {
        match next() % 5 {
            0 => {
                let cutoff = period_grid[(next() as usize) % period_grid.len()];
                ledger.sweep_before(cutoff).unwrap();
                reference.retain(|(_, period), _| *period >= cutoff);
            }
            _ => {
                let principal = principals[(next() as usize) % principals.len()];
                let period = period_grid[(next() as usize) % period_grid.len()];
                let usd = (next() % 50) as f64;
                ledger
                    .record(
                        &Principal::User(principal.to_string()),
                        period,
                        Delta::Usd(usd),
                    )
                    .unwrap();
                *reference
                    .entry((principal.to_string(), period))
                    .or_default() += usd;
            }
        }

        // (a) The index must enumerate exactly the row map's keys — no
        // missing entries (a read would silently undercount), no phantom
        // entries (a read would hit the debug_assert in `total_for`), no
        // duplicates (a principal would be summed twice).
        {
            let state = ledger.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut indexed: Vec<(String, i64)> = state
                .by_period
                .iter()
                .flat_map(|(period, keys)| keys.iter().map(move |k| (k.clone(), *period)))
                .collect();
            indexed.sort();
            let mut actual: Vec<(String, i64)> = state.rows.keys().cloned().collect();
            actual.sort();
            assert_eq!(
                indexed, actual,
                "step {step}: by_period must enumerate exactly the keys of rows"
            );
        }

        // (b) Reads agree with the naive full-scan reference, for a random
        // query point on the grid.
        let window = period_grid[(next() as usize) % period_grid.len()];
        let ancestor = 0i64;
        let expected_total: f64 = reference
            .iter()
            .filter(|((_, period), _)| *period >= window || *period == ancestor)
            .map(|(_, usd)| *usd)
            .sum();
        assert_eq!(
            ledger.total_for(window, ancestor).unwrap().usd,
            expected_total,
            "step {step}: total_for({window}, {ancestor}) disagrees with the full-scan reference"
        );

        let mut expected_principals: Vec<String> = reference
            .keys()
            .filter(|(_, period)| *period == window)
            .map(|(key, _)| key.clone())
            .collect();
        expected_principals.sort();
        expected_principals.dedup();
        let mut got_principals: Vec<String> = ledger
            .principals_in(window)
            .unwrap()
            .iter()
            .map(|(p, _)| p.as_key().to_string())
            .collect();
        got_principals.sort();
        assert_eq!(
            got_principals, expected_principals,
            "step {step}: principals_in({window}) disagrees with the full-scan reference"
        );
    }
}

#[test]
fn sweep_before_removes_only_rows_strictly_older_than_the_cutoff() {
    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());

    ledger.record(&alice, 1_000, Delta::Usd(1.0)).unwrap();
    ledger.record(&alice, 2_000, Delta::Usd(2.0)).unwrap();
    ledger.record(&alice, 3_000, Delta::Usd(3.0)).unwrap();

    let removed = ledger.sweep_before(2_000).unwrap();
    assert_eq!(
        removed, 1,
        "only the 1_000 row is strictly before the cutoff"
    );

    // The cutoff period itself and everything after it survive.
    assert_eq!(ledger.spent_for(&alice, 1_000).unwrap().usd, 0.0);
    assert_eq!(ledger.spent_for(&alice, 2_000).unwrap().usd, 2.0);
    assert_eq!(ledger.spent_for(&alice, 3_000).unwrap().usd, 3.0);
}

#[test]
fn principals_in_orders_by_usd_descending_then_key_ascending() {
    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());
    let bob = Principal::User("u-bob".to_string());
    let carol = Principal::User("u-carol".to_string());

    // Two principals tie on `usd` — must break by key, not by insertion or
    // hash order, or the CLI table would reshuffle between two calls on
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
    let ledger = InMemorySpendLedger::default();
    ledger
        .record(&Principal::Unattributed, 1_000, Delta::Usd(1.0))
        .unwrap();

    let rows = ledger.principals_in(1_000).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, Principal::Unattributed);
}

#[test]
fn principals_in_is_empty_for_a_period_with_no_rows() {
    let ledger = InMemorySpendLedger::default();
    assert!(ledger.principals_in(1_000).unwrap().is_empty());
}

// ============================================================================
// update_policy's `false` branch — moved, not dropped
// ============================================================================
//
// The live-apply honest-downgrade signal (G14, in `config::live_apply`) rests
// on `update_policy` returning `false` when no handle is installed. That branch
// still cannot be exercised through the real global — `install_policy` is
// idempotent and `cargo test --lib` runs this crate's whole suite in one
// process, so `providers::metering`'s `install_test_spend_globals` may install
// the handle before or after any test here, in an order this crate does not
// control. It used to be reached through an injectable
// `update_policy_into(handle, policy)`; `MutableCapabilitySlot` is that seam
// now, and keeping both would be two implementations of one hot-apply. The
// pair that lived here is
// `capability::tests::update_before_install_returns_false_and_changes_nothing`
// and `capability::tests::install_then_update_swaps_the_value_and_keeps_the_stamp`,
// each against a slot the test owns.

// ============================================================================
// check_with (the injectable core of `check`)
// ============================================================================

/// A `SpendLedger` whose every method panics. Supplied to `check_with` in
/// place of a real ledger, this is what turns "the disabled policy never
/// touches the ledger" from a claim into an assertion (G8): if `check_with`
/// ever called a ledger method on the disabled fast path, this test would
/// panic instead of quietly passing because the correct answer happened to
/// be reachable without that call.
struct PanicOnAnyCall;

impl SpendLedger for PanicOnAnyCall {
    fn record(
        &self,
        _principal: &Principal,
        _period_start_ms: i64,
        _delta: Delta,
    ) -> anyhow::Result<()> {
        panic!("check_with must not call SpendLedger::record when the policy is disabled");
    }

    fn spent_for(&self, _principal: &Principal, _period_start_ms: i64) -> anyhow::Result<Spent> {
        panic!("check_with must not call SpendLedger::spent_for when the policy is disabled");
    }

    fn total_for(
        &self,
        _window_start_ms: i64,
        _coarse_ancestor_start_ms: i64,
    ) -> anyhow::Result<Spent> {
        panic!("check_with must not call SpendLedger::total_for when the policy is disabled");
    }

    fn sweep_before(&self, _period_start_ms: i64) -> anyhow::Result<usize> {
        panic!("check_with must not call SpendLedger::sweep_before when the policy is disabled");
    }

    fn principals_in(&self, _period_start_ms: i64) -> anyhow::Result<Vec<(Principal, Spent)>> {
        panic!("check_with must not call SpendLedger::principals_in when the policy is disabled");
    }
}

/// G8 — a disabled policy (`SpendPolicy::default()`: neither ceiling set)
/// never touches the ledger at all, proven by supplying one that panics on
/// every method. See `PanicOnAnyCall`'s doc for why a panic, not a call
/// counter, is the right instrument here.
#[test]
fn g8_disabled_policy_never_touches_the_ledger() {
    let policy = SpendPolicy::default();
    assert!(
        !policy.enabled(),
        "test setup: this policy must be disabled"
    );
    let alice = Principal::User("u-alice".to_string());

    let verdict = check_with(&alice, 1_700_000_000_000, &policy, &PanicOnAnyCall);

    match verdict {
        Verdict::Allowed(spent) => {
            assert_eq!(spent.usd, 0.0);
            assert_eq!(spent.unpriced_calls, 0);
            assert_eq!(spent.partial_calls, 0);
            assert!(
                spent.period_end_ms.is_some(),
                "the window still rides a disabled verdict"
            );
        }
        Verdict::Denied { .. } => panic!("a disabled policy must never deny: {verdict:?}"),
    }
}

/// G9 — with both ceilings configured and both blown, the verdict names
/// `Limit::Total`, not `Limit::PerUser`, even though `alice` (the queried
/// principal) is also individually over her own line. `Limit::Total` is the
/// one she cannot move by asking bob to spend less.
#[test]
fn g9_both_ceilings_blown_reports_total_not_per_user() {
    let policy = SpendPolicy {
        per_user_usd: Some(5.0),
        total_usd: Some(50.0),
        ..SpendPolicy::default()
    };
    let now_ms = 1_700_000_000_000i64;
    let period_start_ms = period::period_start_ms(now_ms, policy.period);

    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());
    let bob = Principal::User("u-bob".to_string());
    // Alice alone blows her own $5 ceiling; alice + bob together blow the
    // $50 machine ceiling.
    ledger
        .record(&alice, period_start_ms, Delta::Usd(5.0))
        .unwrap();
    ledger
        .record(&bob, period_start_ms, Delta::Usd(45.0))
        .unwrap();

    let verdict = check_with(&alice, now_ms, &policy, &ledger);

    match verdict {
        Verdict::Denied {
            limit: Limit::Total,
            ..
        } => {}
        other => panic!("expected Denied{{ limit: Limit::Total, .. }}, got {other:?}"),
    }
}

/// The `PerUser` counterpart to G9: only the per-principal ceiling is
/// configured (and blown) — nothing to prefer over, so the verdict names
/// `Limit::PerUser` and carries alice's own numbers.
#[test]
fn per_user_ceiling_alone_reports_per_user_with_both_numbers() {
    let policy = SpendPolicy {
        per_user_usd: Some(5.0),
        ..SpendPolicy::default()
    };
    let now_ms = 1_700_000_000_000i64;
    let period_start_ms = period::period_start_ms(now_ms, policy.period);

    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());
    ledger
        .record(&alice, period_start_ms, Delta::Usd(7.0))
        .unwrap();

    let verdict = check_with(&alice, now_ms, &policy, &ledger);

    match verdict {
        Verdict::Denied {
            limit: Limit::PerUser { spent, limit },
            spent: outer_spent,
        } => {
            assert_eq!(spent, 7.0);
            assert_eq!(limit, 5.0);
            assert_eq!(
                outer_spent.usd, 7.0,
                "the outer Spent must agree with Limit::PerUser's own number"
            );
        }
        other => panic!("expected Denied{{ limit: Limit::PerUser {{ .. }}, .. }}, got {other:?}"),
    }
}

/// G10 — the boundary is `>=`, stated once in `ceiling_blown`: a principal
/// exactly at the ceiling is denied, and one cent under it is allowed.
#[test]
fn g10_exactly_at_ceiling_denies_one_cent_under_allows() {
    let policy = SpendPolicy {
        per_user_usd: Some(10.0),
        ..SpendPolicy::default()
    };
    let now_ms = 1_700_000_000_000i64;
    let period_start_ms = period::period_start_ms(now_ms, policy.period);
    let alice = Principal::User("u-alice".to_string());

    let at_ceiling = InMemorySpendLedger::default();
    at_ceiling
        .record(&alice, period_start_ms, Delta::Usd(10.0))
        .unwrap();
    assert!(
        matches!(
            check_with(&alice, now_ms, &policy, &at_ceiling),
            Verdict::Denied {
                limit: Limit::PerUser { .. },
                ..
            }
        ),
        "exactly at the ceiling must be denied"
    );

    let under_ceiling = InMemorySpendLedger::default();
    under_ceiling
        .record(&alice, period_start_ms, Delta::Usd(9.99))
        .unwrap();
    assert!(
        matches!(
            check_with(&alice, now_ms, &policy, &under_ceiling),
            Verdict::Allowed(_)
        ),
        "one cent under the ceiling must be allowed"
    );
}

/// With neither ceiling blown, `check_with` allows and the returned `Spent`
/// is `alice`'s own current spend — not zero, not the machine total.
#[test]
fn neither_ceiling_blown_allows_with_the_principals_own_spend() {
    let policy = SpendPolicy {
        per_user_usd: Some(10.0),
        total_usd: Some(1000.0),
        ..SpendPolicy::default()
    };
    let now_ms = 1_700_000_000_000i64;
    let period_start_ms = period::period_start_ms(now_ms, policy.period);
    let alice = Principal::User("u-alice".to_string());

    let ledger = InMemorySpendLedger::default();
    ledger
        .record(&alice, period_start_ms, Delta::Usd(3.0))
        .unwrap();

    match check_with(&alice, now_ms, &policy, &ledger) {
        Verdict::Allowed(spent) => assert_eq!(spent.usd, 3.0),
        other => panic!("expected Allowed, got {other:?}"),
    }
}

/// The window rides every verdict: `period_start_ms` matches
/// `spend::period::period_start_ms` and `period_end_ms` is populated
/// (`Some`, not the pre-Task-5 `period_start_ms` placeholder) whether the
/// call lands in `Allowed` or `Denied`.
#[test]
fn the_window_rides_both_allowed_and_denied_verdicts() {
    let policy = SpendPolicy {
        per_user_usd: Some(5.0),
        ..SpendPolicy::default()
    };
    let now_ms = 1_700_000_000_000i64;
    let expected_start = period::period_start_ms(now_ms, policy.period);
    let expected_end = period::period_end_ms(now_ms, policy.period);
    assert_ne!(
        expected_start, expected_end,
        "test setup: a real period is never zero-length"
    );
    let alice = Principal::User("u-alice".to_string());

    let allowed_ledger = InMemorySpendLedger::default();
    match check_with(&alice, now_ms, &policy, &allowed_ledger) {
        Verdict::Allowed(spent) => {
            assert_eq!(spent.period_start_ms, expected_start);
            assert_eq!(spent.period_end_ms, Some(expected_end));
        }
        other => panic!("expected Allowed, got {other:?}"),
    }

    let denied_ledger = InMemorySpendLedger::default();
    denied_ledger
        .record(&alice, expected_start, Delta::Usd(5.0))
        .unwrap();
    match check_with(&alice, now_ms, &policy, &denied_ledger) {
        Verdict::Denied { spent, .. } => {
            assert_eq!(spent.period_start_ms, expected_start);
            assert_eq!(spent.period_end_ms, Some(expected_end));
        }
        other => panic!("expected Denied, got {other:?}"),
    }
}

/// `Denied { limit: Limit::Total, spent }` must carry `alice`'s own spend in
/// `spent`, never the machine total — see `Verdict`'s doc on why this is
/// load-bearing (the machine total must not ride back in through the
/// sibling field `Limit::Total` was made fieldless to keep it off). Alice's
/// own figure ($5) and the machine total ($50) are made deliberately
/// unequal so the assertion cannot pass by coincidence.
#[test]
fn denied_total_carries_the_principals_own_spend_not_the_machine_total() {
    let policy = SpendPolicy {
        total_usd: Some(50.0),
        ..SpendPolicy::default()
    };
    let now_ms = 1_700_000_000_000i64;
    let period_start_ms = period::period_start_ms(now_ms, policy.period);

    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());
    let bob = Principal::User("u-bob".to_string());
    ledger
        .record(&alice, period_start_ms, Delta::Usd(5.0))
        .unwrap();
    ledger
        .record(&bob, period_start_ms, Delta::Usd(50.0))
        .unwrap();
    // alice alone hasn't blown any per-user ceiling (none is configured);
    // alice + bob together blow the $50 machine ceiling.
    assert_eq!(
        ledger
            .total_for(period_start_ms, period_start_ms)
            .unwrap()
            .usd,
        55.0,
        "test setup: the machine total must differ from alice's own spend \
         (ancestor == window under a Month policy, so the second arm is a no-op)"
    );

    match check_with(&alice, now_ms, &policy, &ledger) {
        Verdict::Denied {
            limit: Limit::Total,
            spent,
        } => {
            assert_eq!(
                spent.usd, 5.0,
                "spent must be alice's own $5, never the machine's $55 total"
            );
        }
        other => panic!("expected Denied{{ limit: Limit::Total, .. }}, got {other:?}"),
    }
}

// ============================================================================
// Ledger read errors fail open (see `resolve_read`'s doc for the ruling and
// why this differs from an authorization gate's "Err means refusal")
// ============================================================================

/// A `SpendLedger` whose every read fails. Pins the fail-open direction
/// `resolve_read` documents: a read error must not be turned into a denial.
struct ErroringLedger;

impl SpendLedger for ErroringLedger {
    fn record(
        &self,
        _principal: &Principal,
        _period_start_ms: i64,
        _delta: Delta,
    ) -> anyhow::Result<()> {
        anyhow::bail!("ErroringLedger: record is unavailable")
    }

    fn spent_for(&self, _principal: &Principal, _period_start_ms: i64) -> anyhow::Result<Spent> {
        anyhow::bail!("ErroringLedger: spent_for is unavailable")
    }

    fn total_for(
        &self,
        _window_start_ms: i64,
        _coarse_ancestor_start_ms: i64,
    ) -> anyhow::Result<Spent> {
        anyhow::bail!("ErroringLedger: total_for is unavailable")
    }

    fn sweep_before(&self, _period_start_ms: i64) -> anyhow::Result<usize> {
        anyhow::bail!("ErroringLedger: sweep_before is unavailable")
    }

    fn principals_in(&self, _period_start_ms: i64) -> anyhow::Result<Vec<(Principal, Spent)>> {
        anyhow::bail!("ErroringLedger: principals_in is unavailable")
    }
}

/// A `tracing_subscriber::Layer` that records every ERROR-level event's
/// formatted `message` field, scoped to one closure via
/// `tracing::subscriber::with_default` — no new dependency, just the
/// `tracing`/`tracing-subscriber` machinery this crate already depends on
/// everywhere else. Holds an `Arc<Mutex<..>>` (cheap to clone, so the layer
/// itself can be moved by value into `.with()` while `with_captured_error_events`
/// keeps its own handle to read the events back out afterward) rather than
/// implementing `Layer` for `Arc<Self>` directly, which `tracing_subscriber`
/// does not provide a blanket impl for.
#[derive(Clone)]
struct CapturedErrorEvents(Arc<Mutex<Vec<String>>>);

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturedErrorEvents {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() != tracing::Level::ERROR {
            return;
        }
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(visitor.0);
    }
}

/// Runs `f` with a subscriber installed that captures every ERROR-level
/// event fired during it, and returns `f`'s result alongside those events'
/// messages.
fn with_captured_error_events<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    use tracing_subscriber::layer::SubscriberExt as _;

    let captured = CapturedErrorEvents(Arc::new(Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::registry().with(captured.clone());
    let result = tracing::subscriber::with_default(subscriber, f);
    let events = captured.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
    (result, events)
}

/// Fix for concern 1 in the task-5 report: a `SpendLedger` read error must
/// fail open (`Allowed`, spend treated as zero for this check) rather than
/// deny — and it must be logged, not silently swallowed. The per-user
/// ceiling is set low enough ($0.01) that any real read succeeding with
/// nonzero spend would deny; the `Allowed` verdict below can therefore only
/// come from the fail-open path treating the failed read as zero, not from
/// the ceiling being unreachably high.
#[test]
fn ledger_read_error_fails_open_and_is_logged_not_denied() {
    let policy = SpendPolicy {
        per_user_usd: Some(0.01),
        ..SpendPolicy::default()
    };
    let now_ms = 1_700_000_000_000i64;
    let alice = Principal::User("u-alice".to_string());

    let (verdict, error_events) =
        with_captured_error_events(|| check_with(&alice, now_ms, &policy, &ErroringLedger));

    match verdict {
        Verdict::Allowed(spent) => {
            assert_eq!(
                spent.usd, 0.0,
                "a failed read must be treated as zero spend"
            );
        }
        other => panic!("a ledger read error must fail open (Allowed), not deny: {other:?}"),
    }
    assert_eq!(
        error_events.len(),
        1,
        "exactly one ERROR event must fire for the one failed spent_for read; got {error_events:?}"
    );
    assert!(
        error_events[0].contains("spend::check") && error_events[0].contains("spent_for"),
        "the logged error must name what failed, got: {:?}",
        error_events[0]
    );
}

// ============================================================================
// G15 — src/spend/ never calls the agent-shaped actor resolvers
// ============================================================================

/// Production `.rs` files anywhere under `src/spend/`, paired with their
/// contents. Recurses into subdirectories — mirrors `utils::paths`'s
/// `all_sources` walker — because a flat `read_dir` would go blind the day
/// the durable backend lands as `spend::sqlite::mod.rs` (a submodule
/// directory rather than a sibling file): a directory entry has no `.rs`
/// extension, so a non-recursive scan would silently exclude the whole
/// subtree from the one guard whose job is exactly to catch what that
/// subtree does.
///
/// Excludes every file named `tests.rs`, at any depth: the guard's own rule
/// text is code (not comments), so the `contains` check would flag itself
/// forever, and the same self-reference problem recurs for any nested
/// `#[cfg(test)] mod tests` this tree grows.
fn spend_sources() -> Vec<(std::path::PathBuf, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/spend");
    let mut files = Vec::new();
    walk(&dir, &mut files);

    files
        .into_iter()
        .filter(|path| path.file_name().is_none_or(|n| n != "tests.rs"))
        .filter_map(|path| std::fs::read_to_string(&path).ok().map(|text| (path, text)))
        .collect()
}

/// Non-comment lines only. A `///`/`//!`/`//` doc line explaining *why* these
/// two names are banned (as this module's own doc comments do) would
/// otherwise satisfy a naive `contains` check and make the guard
/// permanently green regardless of what the production code actually calls.
/// Mirrors `utils::paths`'s `code_lines` helper.
fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> + '_ {
    text.lines().enumerate().filter_map(|(i, line)| {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            None
        } else {
            Some((i + 1, line))
        }
    })
}

#[test]
fn g15_no_ambient_actor_or_current_agent_id_in_spend_source() {
    let sources = spend_sources();
    assert!(
        sources.len() >= 2,
        "expected at least mod.rs and period.rs under src/spend/, found {}",
        sources.len()
    );

    let mut total_code_chars = 0usize;
    let mut offenders = Vec::new();
    for (path, text) in &sources {
        for (n, line) in code_lines(text) {
            total_code_chars += line.trim().len();
            if line.contains("ambient_actor") || line.contains("current_agent_id") {
                offenders.push(format!("{}:{n}: {}", path.display(), line.trim()));
            }
        }
    }

    // A CRLF checkout (or any other accident that makes the line filter
    // read nothing) must fail loudly rather than pass vacuously: assert the
    // scan actually read a non-trivial production prefix before trusting
    // its "no offenders" verdict.
    assert!(
        total_code_chars > 200,
        "scan read a suspiciously small production prefix ({total_code_chars} chars) — \
         this guard would be vacuously green if the line filter matched nothing"
    );

    assert!(
        offenders.is_empty(),
        "src/spend/ must resolve who a run's spend is charged to using only \
         ambient_principal/principal_from_metadata — never ambient_actor() or \
         current_agent_id(), whose third fallback arm is an agent id, and an \
         agent is not a person and cannot hold a budget:\n  {}",
        offenders.join("\n  ")
    );
}

// ============================================================================
// The process-global handles, as capability slots
// ============================================================================

#[test]
fn the_policy_handle_reports_whether_it_was_installed() {
    // The §5.22 round-7 shape, now answerable: `configured: false` is a
    // true statement about an unconfigured box AND about a box whose
    // handle boot never installed. Only the outcome separates them.
    use crate::capability::SlotStatus;
    let erased: &dyn SlotStatus = &GLOBAL_POLICY;
    assert_eq!(erased.id(), "spend/policy");
}

/// The roster's entry point for these two handles.
///
/// [`crate::capability::ALL_SLOTS`] assembles from accessors like these rather
/// than from one `pub static` per handle, so the accessor — not the static —
/// is the thing that must keep working. Asserting through it also means the
/// ids are pinned on the path the roster actually walks: a slot renamed in
/// one place and not the other shows up here instead of as a roster entry
/// quietly describing the wrong handle.
#[test]
fn the_slot_accessors_expose_both_handles_to_the_roster() {
    use crate::capability::{MissingSemantics, SlotStatus};

    let slots: [&'static dyn SlotStatus; 2] = [global_ledger_slot(), global_policy_slot()];
    let ids: Vec<&str> = slots.iter().map(|s| s.id()).collect();
    assert_eq!(ids, vec!["spend/ledger", "spend/policy"]);

    // Both are the round-7 shape by construction, and the sentence each one
    // carries is what a diagnostic prints when `outcome()` is `None`. A slot
    // that lost it would still report an id and still look fine.
    for slot in slots {
        match slot.missing() {
            MissingSemantics::IndistinguishableDefault { reads_as } => {
                assert!(
                    !reads_as.is_empty(),
                    "{}: an IndistinguishableDefault with nothing to say is the \
                     silence this round exists to remove",
                    slot.id()
                );
            }
            other => panic!(
                "{}: expected IndistinguishableDefault, got {other:?}",
                slot.id()
            ),
        }
    }
}
