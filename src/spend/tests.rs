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

#[test]
fn total_for_sums_every_principal_in_the_period_and_ignores_other_periods() {
    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());
    let bob = Principal::User("u-bob".to_string());

    ledger.record(&alice, 1_000, Delta::Usd(1.0)).unwrap();
    ledger.record(&bob, 1_000, Delta::Usd(2.0)).unwrap();
    for _ in 0..3 {
        ledger.record(&bob, 1_000, Delta::Unpriced).unwrap();
    }
    for _ in 0..4 {
        ledger.record(&bob, 1_000, Delta::Partial(0.0)).unwrap();
    }
    // A different period must not contribute to the 1_000 total.
    ledger.record(&alice, 2_000, Delta::Usd(100.0)).unwrap();

    let total = ledger.total_for(1_000).unwrap();
    assert_eq!(total.usd, 3.0);
    assert_eq!(total.unpriced_calls, 3);
    assert_eq!(total.partial_calls, 4);
}

#[test]
fn sweep_before_removes_only_rows_strictly_older_than_the_cutoff() {
    let ledger = InMemorySpendLedger::default();
    let alice = Principal::User("u-alice".to_string());

    ledger.record(&alice, 1_000, Delta::Usd(1.0)).unwrap();
    ledger.record(&alice, 2_000, Delta::Usd(2.0)).unwrap();
    ledger.record(&alice, 3_000, Delta::Usd(3.0)).unwrap();

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
