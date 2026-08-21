//! Tests for `spend::mod` — the core types, the in-process ledger default,
//! and `principal_from_metadata`. `ambient_principal`'s equivalence with
//! `principal_from_metadata` (G13) lives in
//! `gateway::execution_engine::run_loop::tests`, next to
//! `with_request_scope`, because it needs that function's `pub(super)`
//! visibility — see this round's task-3 brief for why widening that
//! visibility instead was rejected. G8–G10 (the `spend::check` guards) land
//! here in a later task.

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
    let mut meta = HashMap::new();
    meta.insert(
        crate::scope::OWNER_META_KEY.to_string(),
        "u-room-owner".to_string(),
    );

    assert_eq!(
        principal_from_metadata(&meta),
        Principal::User("u-room-owner".to_string())
    );
}

#[test]
fn principal_from_metadata_is_unattributed_when_neither_key_is_present() {
    assert_eq!(principal_from_metadata(&HashMap::new()), Principal::Unattributed);
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

    ledger
        .record(&alice, 1_000, Delta { usd: 1.5, unpriced: 0, partial: 0 })
        .unwrap();
    ledger
        .record(&alice, 1_000, Delta { usd: 2.25, unpriced: 1, partial: 2 })
        .unwrap();

    let spent = ledger.spent_for(&alice, 1_000).unwrap();
    assert_eq!(spent.usd, 3.75);
    assert_eq!(spent.unpriced_calls, 1);
    assert_eq!(spent.partial_calls, 2);
}

#[test]
fn record_keeps_distinct_periods_separate() {
    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());

    ledger
        .record(&alice, 1_000, Delta { usd: 5.0, ..Default::default() })
        .unwrap();
    ledger
        .record(&alice, 2_000, Delta { usd: 9.0, ..Default::default() })
        .unwrap();

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

#[test]
fn total_for_sums_every_principal_in_the_period_and_ignores_other_periods() {
    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());
    let bob = Principal::User("u-bob".to_string());

    ledger
        .record(&alice, 1_000, Delta { usd: 1.0, ..Default::default() })
        .unwrap();
    ledger
        .record(&bob, 1_000, Delta { usd: 2.0, unpriced: 3, partial: 4 })
        .unwrap();
    // A different period must not contribute to the 1_000 total.
    ledger
        .record(&alice, 2_000, Delta { usd: 100.0, ..Default::default() })
        .unwrap();

    let total = ledger.total_for(1_000).unwrap();
    assert_eq!(total.usd, 3.0);
    assert_eq!(total.unpriced_calls, 3);
    assert_eq!(total.partial_calls, 4);
}

#[test]
fn sweep_before_removes_only_rows_strictly_older_than_the_cutoff() {
    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());

    ledger
        .record(&alice, 1_000, Delta { usd: 1.0, ..Default::default() })
        .unwrap();
    ledger
        .record(&alice, 2_000, Delta { usd: 2.0, ..Default::default() })
        .unwrap();
    ledger
        .record(&alice, 3_000, Delta { usd: 3.0, ..Default::default() })
        .unwrap();

    let removed = ledger.sweep_before(2_000).unwrap();
    assert_eq!(removed, 1, "only the 1_000 row is strictly before the cutoff");

    // The cutoff period itself and everything after it survive.
    assert_eq!(ledger.spent_for(&alice, 1_000).unwrap().usd, 0.0);
    assert_eq!(ledger.spent_for(&alice, 2_000).unwrap().usd, 2.0);
    assert_eq!(ledger.spent_for(&alice, 3_000).unwrap().usd, 3.0);
}

// ============================================================================
// G15 — src/spend/ never calls the agent-shaped actor resolvers
// ============================================================================

/// Production `.rs` files directly under `src/spend/`, paired with their
/// contents. Excludes this test file: the guard's own rule text is code
/// (not comments), so the `contains` check would flag itself forever.
fn spend_sources() -> Vec<(std::path::PathBuf, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/spend");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "rs") {
            // Skip the guard's own test file: its rule text is code (not comments), so
            // the `contains` check would flag itself forever without this exclusion.
            if path.file_name().is_some_and(|n| n == "tests.rs") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push((path, text));
            }
        }
    }
    out
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
