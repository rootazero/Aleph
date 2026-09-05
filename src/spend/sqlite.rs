//! The durable [`SpendLedger`] backend — a per-principal, per-period row in
//! the [`SecurityStore`]'s `spend_ledger` table (added by that store's v17
//! migration; see `gateway::security::store::SCHEMA_VERSION`), fronted by an
//! in-process write-through cache.
//!
//! # Why this wraps `Arc<SecurityStore>` and not its own connection
//!
//! Every other durable consumer in this codebase (`AgentKeystore`,
//! `NodeManageTool`, `SharedTokenManager`, …) is constructed from the one
//! `Arc<SecurityStore>` boot already opened, rather than opening a second
//! connection to the same file. This ledger follows that convention: it has
//! no `open(path)` of its own, only [`SqliteSpendLedger::new`], which takes
//! the shared handle. That is also what makes the UPSERT in [`record`]
//! actually single-writer — `SecurityStore::conn` is one `Mutex<Connection>`,
//! so two `record` calls (from the floor arm and the admission arm, the two
//! writers the plan calls for) serialize through the same lock the rest of
//! the store's tables already share.
//!
//! # The cache/DB write-ordering invariant
//!
//! [`record`] holds `store.conn`'s lock across *both* the UPSERT and the
//! cache write. That is not incidental: if the cache write happened after
//! releasing the connection lock, two threads racing to `record` the same
//! `(principal, period)` could have their cache writes land in the opposite
//! order from their DB commits — the later DB-committer's correct running
//! total could be overwritten by the earlier committer's now-stale one, even
//! though the table itself stayed correct throughout. Keeping the cache
//! write inside the connection's critical section makes the cache's write
//! order match the DB's commit order by construction. [`spent_for`] and
//! [`sweep_before`] follow the same lock order (`conn` before `cache`) for
//! the same reason and to avoid a lock-order deadlock with `record`.
//!
//! [`record`]: SqliteSpendLedger::record
//! [`spent_for`]: SqliteSpendLedger::spent_for
//! [`sweep_before`]: SqliteSpendLedger::sweep_before
//!
//! # Why `record` is a single UPSERT with `RETURNING`, not UPSERT-then-SELECT
//!
//! The two statements are folded into one so a SELECT failure after a
//! successful UPSERT can no longer propagate out of `record` as "the cost
//! is not reflected in the ledger" — because if the single statement
//! errored, the cost really is not. The error path that
//! `MeteringProvider::record_spend`'s log line fires on is now strictly
//! the truthful one. See that function's call site and the test
//! `record_upsert_succeeds_and_cache_populated_via_returning` for the
//! pinning.

use std::collections::HashMap;

use rusqlite::OptionalExtension;

use super::{Delta, Principal, SpendLedger, Spent};
use crate::gateway::security::SecurityStore;
use crate::sync_primitives::{Arc, Mutex};

#[cfg(test)]
mod tests;

/// The cached snapshot of one `(principal_id, period_start)` row. Mirrors
/// the table's non-key columns, minus `updated_at` — nothing reads that
/// column back through the ledger trait.
#[derive(Clone, Copy)]
struct CachedRow {
    usd: f64,
    unpriced_calls: u64,
    partial_calls: u64,
}

/// Durable [`SpendLedger`] backed by the [`SecurityStore`]'s `spend_ledger`
/// table, fronted by an in-process write-through cache keyed by
/// `(principal_id, period_start_ms)`.
pub struct SqliteSpendLedger {
    store: Arc<SecurityStore>,
    cache: Mutex<HashMap<(String, i64), CachedRow>>,
}

impl SqliteSpendLedger {
    /// The one production constructor. `store` is the same `Arc<SecurityStore>`
    /// boot opens once and hands to every other durable consumer — see the
    /// module doc for why this does not open its own connection.
    #[must_use]
    pub fn new(store: Arc<SecurityStore>) -> Self {
        Self {
            store,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// A standalone ledger over a private, non-durable `SecurityStore::in_memory()`,
    /// for tests that want `SqliteSpendLedger`'s SQL/cache logic without
    /// wiring a real, file-backed store.
    ///
    /// `#[cfg(test)]` on purpose, and that is the enforcement: production has
    /// exactly one `spend_ledger` table (inside the one boot-opened
    /// `SecurityStore`), and a ledger built over its own private store would
    /// be a second, disconnected writer with its own empty cache believing it
    /// owns that same row space — the shape `ProcessRegistry` shipped (see
    /// its `journaled` field and `#[cfg(test)] fn new()`), where several
    /// independently-constructed instances each started their own
    /// bookkeeping from zero while a single durable store existed on the
    /// side. Keeping this constructor unreachable outside tests turns "there
    /// is exactly one spend ledger" from a convention into a compile error.
    #[cfg(test)]
    fn for_test() -> Self {
        Self::new(Arc::new(
            SecurityStore::in_memory().expect("in-memory security store"),
        ))
    }
}

impl SpendLedger for SqliteSpendLedger {
    fn record(
        &self,
        principal: &Principal,
        period_start_ms: i64,
        delta: Delta,
    ) -> anyhow::Result<()> {
        // `Delta` is exhaustively matched, no wildcard arm — see the type's
        // doc: a call site adding a fourth variant must decide how it moves
        // these three columns, not silently fall through as a no-op.
        let (delta_usd, delta_unpriced, delta_partial): (f64, i64, i64) = match delta {
            Delta::Usd(usd) => (usd, 0, 0),
            Delta::Partial(usd) => (usd, 0, 1),
            Delta::Unpriced => (0.0, 1, 0),
        };
        // NaN / ±inf in the USD column silently disables the spend ceiling
        // (IEEE 754 NaN comparisons are false, so `spent >= limit` is false
        // forever once a corrupt row lands). Mirror the InMemory backend's
        // guard: coerce to 0.0 + bump unpriced_calls so the 'this call had
        // a price it couldn't represent' signal is loud and the ceiling
        // continues to evaluate against a real number.
        let (delta_usd, delta_unpriced, delta_partial): (f64, i64, i64) =
            if !delta_usd.is_finite() {
                (0.0, delta_unpriced.saturating_add(1), delta_partial)
            } else {
                (delta_usd, delta_unpriced, delta_partial)
            };
        let key = principal.as_key().to_string();
        let updated_at = chrono::Utc::now().timestamp_millis();

        let conn = self.store.conn.lock().unwrap_or_else(|e| e.into_inner());
        // Single round-trip UPSERT that returns the row's post-write
        // values. Folding the read-back into the UPSERT closes the failure
        // window the two-statement shape had: if the SELECT ever failed
        // after a successful UPSERT, `record` returned an error to
        // `MeteringProvider::record_spend`, whose log line then claimed
        // "this call's cost is not reflected in the spend ledger" — but
        // the cost WAS reflected, the read just lost the race. With
        // RETURNING, any failure here is a real UPSERT failure (cost is
        // NOT in the ledger), and the log becomes truthful.
        //
        // Requires SQLite >= 3.35 for RETURNING in an UPSERT. `Cargo.toml`
        // pulls rusqlite with `bundled`, and the existing `GROUP_CONCAT(x
        // ORDER BY y)` site in `agents/swarm/tasks/store/crud.rs` already
        // assumes >= 3.44, so the bundled SQLite covers us here.
        let (usd, unpriced_calls, partial_calls): (f64, i64, i64) = conn.query_row(
            "INSERT INTO spend_ledger \
             (principal_id, period_start, usd, unpriced_calls, partial_calls, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(principal_id, period_start) DO UPDATE SET \
                 usd = spend_ledger.usd + excluded.usd, \
                 unpriced_calls = spend_ledger.unpriced_calls + excluded.unpriced_calls, \
                 partial_calls = spend_ledger.partial_calls + excluded.partial_calls, \
                 updated_at = excluded.updated_at \
             RETURNING usd, unpriced_calls, partial_calls",
            rusqlite::params![
                key,
                period_start_ms,
                delta_usd,
                delta_unpriced,
                delta_partial,
                updated_at
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

        // See the module doc: this write must land while `conn` is still
        // locked, so its ordering relative to a racing thread's cache write
        // matches the DB commit ordering the connection lock already
        // enforces.
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(
            (key, period_start_ms),
            CachedRow {
                usd,
                unpriced_calls: unpriced_calls as u64,
                partial_calls: partial_calls as u64,
            },
        );
        drop(cache);
        drop(conn);
        Ok(())
    }

    fn spent_for(&self, principal: &Principal, period_start_ms: i64) -> anyhow::Result<Spent> {
        let key = principal.as_key().to_string();

        if let Some(row) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(key.clone(), period_start_ms))
        {
            return Ok(Spent {
                usd: row.usd,
                unpriced_calls: row.unpriced_calls,
                partial_calls: row.partial_calls,
                period_start_ms,
                // See `Spent::period_end_ms`'s doc: only `check` can
                // compute this — the ledger does not know `SpendPeriod`.
                period_end_ms: None,
            });
        }

        // Cache miss: read through. Same `conn`-before-`cache`, one critical
        // section shape as `record` — see the module doc.
        let conn = self.store.conn.lock().unwrap_or_else(|e| e.into_inner());
        let found: Option<(f64, i64, i64)> = conn
            .query_row(
                "SELECT usd, unpriced_calls, partial_calls FROM spend_ledger \
                 WHERE principal_id = ?1 AND period_start = ?2",
                rusqlite::params![key, period_start_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (usd, unpriced_calls, partial_calls) = found.unwrap_or((0.0, 0, 0));

        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(
            (key, period_start_ms),
            CachedRow {
                usd,
                unpriced_calls: unpriced_calls as u64,
                partial_calls: partial_calls as u64,
            },
        );
        drop(cache);
        drop(conn);

        Ok(Spent {
            usd,
            unpriced_calls: unpriced_calls as u64,
            partial_calls: partial_calls as u64,
            period_start_ms,
            // See `Spent::period_end_ms`'s doc.
            period_end_ms: None,
        })
    }

    fn total_for(
        &self,
        window_start_ms: i64,
        coarse_ancestor_start_ms: i64,
    ) -> anyhow::Result<Spent> {
        // Deliberately not cached, and deliberately not a stored `@org` row
        // — see the module this trait lives in
        // (`crate::spend::SpendLedger`) and the plan: a stored aggregate is
        // a second source of truth for a number these rows already answer,
        // and the two drift the first time a write lands on one and not the
        // other. `SUM()` over zero matching rows is `NULL`, hence the
        // `Option` columns.
        //
        // The WHERE clause is the trait's fail-closed window rule (spend
        // I-1), identical to the in-memory backend's: every row keyed
        // inside the current window (`period_start >= window_start_ms`,
        // which also catches rows recorded under a finer old policy after
        // a hot `SpendPeriod` switch, e.g. Day → Month), plus the row
        // keyed at the start of the coarsest period containing the window
        // (`period_start = coarse_ancestor_start_ms`, which catches a
        // coarser old policy, e.g. Month → Day). See the trait method's
        // doc for why the deliberate over-count is the chosen direction.
        let conn = self.store.conn.lock().unwrap_or_else(|e| e.into_inner());
        let (usd, unpriced_calls, partial_calls): (Option<f64>, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT SUM(usd), SUM(unpriced_calls), SUM(partial_calls) \
             FROM spend_ledger WHERE period_start >= ?1 OR period_start = ?2",
                rusqlite::params![window_start_ms, coarse_ancestor_start_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;

        Ok(Spent {
            usd: usd.unwrap_or(0.0),
            unpriced_calls: unpriced_calls.unwrap_or(0) as u64,
            partial_calls: partial_calls.unwrap_or(0) as u64,
            period_start_ms: window_start_ms,
            // See `Spent::period_end_ms`'s doc.
            period_end_ms: None,
        })
    }

    fn sweep_before(&self, period_start_ms: i64) -> anyhow::Result<usize> {
        let conn = self.store.conn.lock().unwrap_or_else(|e| e.into_inner());
        let removed = conn.execute(
            "DELETE FROM spend_ledger WHERE period_start < ?1",
            rusqlite::params![period_start_ms],
        )?;

        // The cache must agree with the delete, not just the table — an
        // entry left behind for a swept period would go on answering
        // `spent_for` from a row that no longer durably exists.
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|(_, period), _| *period >= period_start_ms);
        drop(cache);
        drop(conn);

        Ok(removed)
    }

    fn principals_in(&self, period_start_ms: i64) -> anyhow::Result<Vec<(Principal, Spent)>> {
        // Deliberately reads the table, not the cache — same reason as
        // `total_for`: the write-through cache only holds rows this process
        // has touched via `record`/`spent_for` since boot, so a cache-only
        // enumeration would silently omit every row written before this
        // process started or by another process. `ORDER BY` in SQL rather
        // than sorting in Rust for the same reason `total_for` sums in SQL:
        // one round trip, and the ordering the trait method's doc requires
        // (`usd` descending, then key ascending) is exactly what the
        // database is already good at.
        let conn = self.store.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT principal_id, usd, unpriced_calls, partial_calls FROM spend_ledger \
             WHERE period_start = ?1 ORDER BY usd DESC, principal_id ASC",
        )?;
        let rows: Vec<(String, f64, i64, i64)> = stmt
            .query_map(rusqlite::params![period_start_ms], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<_>, rusqlite::Error>>()?;
        drop(stmt);
        drop(conn);

        Ok(rows
            .into_iter()
            .map(|(principal_id, usd, unpriced_calls, partial_calls)| {
                (
                    Principal::from_key(&principal_id),
                    Spent {
                        usd,
                        unpriced_calls: unpriced_calls as u64,
                        partial_calls: partial_calls as u64,
                        period_start_ms,
                        // See `Spent::period_end_ms`'s doc.
                        period_end_ms: None,
                    },
                )
            })
            .collect())
    }
}
