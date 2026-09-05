# src/spend review (raw agent output)

## Summary
- Files scanned: src/spend/{mod.rs, period.rs, sqlite.rs, tests.rs, sqlite/tests.rs}
- Critical: 0, Important: 3, Minor: 6
- Health: green

## Strengths (selected)
- Defensive locking discipline in SqliteSpendLedger
- record is a single-statement UPSERT+RETURNING
- Principal::user is the single chokepoint for untrusted input
- Two-function split pattern (check / check_with) consistently applied
- Period math is calendar-correct under DST and month rollover
- Fail-open design on ledger read errors is explicitly load-bearing

## Critical findings
None.

## Important findings

### I-1 `total_for` becomes zero after a hot policy change of `SpendPeriod`
- File: src/spend/sqlite.rs:181-203 + src/spend/mod.rs:683-695
- Problem: total_for is keyed by period_start_ms. If policy changes from Period::Day to Period::Month, the WHERE clause period_start = ?1 matches nothing → 0. The machine ceiling silently stops firing for the entire rest of the current month.
- Suggested fix: change total_for to derive the window as [retention_cutoff_ms, now] rather than period_start = X.

### I-2 `f64` NaN propagation in `usd` accumulation silently lets spend bypass the ceiling
- File: src/spend/mod.rs:289-305 (InMemorySpendLedger) and src/spend/sqlite.rs:124-167 (UPSERT math) and src/spend/mod.rs:691-693 (ceiling comparison)
- Problem: record performs row.usd += usd with no is_finite() guard. NaN persists in the ledger. ceiling_blown(spent_usd, limit_usd) is spent_usd >= limit_usd; IEEE 754 says NaN comparison is always false, so check returns Allowed.
- Suggested fix: in record, check delta_usd.is_finite() and either reject or coerce NaN/±inf to 0.0 and increment unpriced_calls.

### I-3 `InMemorySpendLedger::total_for` / `principals_in` are O(N) over all retained periods
- File: src/spend/mod.rs:323-350 (total_for) and src/spend/mod.rs:368-408 (principals_in)
- Problem: storage is HashMap<(String, i64), Row>. Both methods iterate the entire map and filter. With 10k principals and 3 periods retained, 30k entries scanned per check_with call.
- Suggested fix: keep a parallel HashMap<i64, Vec<String>> index by period_start_ms.

## Minor findings
### M-1 Poisoned-mutex recovery is silent across the entire module
- File: src/spend/mod.rs:282, 311, 326, 354, 375 and src/spend/sqlite.rs:139, 180, 207
- Note: every lock().unwrap_or_else(|e| e.into_inner()) recovers from poisoning silently.

### M-2 `as u64` cast on `i64` SQLite sums silently wraps on negative input
- File: src/spend/sqlite.rs:152, 175, 195-200, 260
- Note: same pattern as src/resilience/database/traces.rs:293,309 already flagged in 2026-09-05 audit.

### M-3 `Period::Day` `succ_opt().expect(...)` panics on `NaiveDate::MAX` input
- File: src/spend/period.rs:127-131

### M-4 Two principal resolvers duplicate logic that could drift
- File: src/spend/mod.rs:547-585

### M-5 `current_room_author` lookup path is not exercised by a unit test in this module
- File: src/spend/mod.rs:553-557

### M-6 `update_policy`'s doc says the false branch is "not exercisable through this global" but it actually is
- File: src/spend/mod.rs:478-499 (doc)

## Cross-cutting observations
- Module-wide unwrap_or_else(|e| e.into_inner()) for poisoned-mutex recovery
- Defensive Option-and-fallback pattern in period.rs is the baseline
- Two-function split pattern consistently applied
- capability slot migration is complete and uniform
