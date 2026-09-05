//! Per-principal and machine-total USD spend ceilings.
//!
//! See [`crate::config::types::policies::spend`] for the config shape
//! ([`crate::config::types::policies::SpendPolicy`] /
//! [`crate::config::types::policies::SpendPeriod`]) and [`period`] for the
//! local-calendar boundaries a period resolves to.
//!
//! This module additionally carries:
//!
//! - the ledger-facing types ([`Principal`], [`Spent`], [`Delta`], [`Limit`],
//!   [`Verdict`]) and the [`SpendLedger`] trait a durable backend implements
//!   (the in-process default is [`InMemorySpendLedger`]; the durable one
//!   lives in `spend::sqlite`, wired in at boot);
//! - the two process-global handles ([`global_ledger`] / [`install_ledger`]
//!   for the ledger, [`current_policy`] / [`install_policy`] /
//!   [`update_policy`] for the policy) — process-global rather than
//!   constructor parameters because `MeteringProvider` has 7 production
//!   construction sites (`context::compact::compactor`,
//!   `providers::moa::provider`, `agents::subagent_spawner::mod` ×2,
//!   `orchestrator::harness_bridge::runner_impl` ×3); threading a handle
//!   through them would wire some and miss others, and the missed ones would
//!   meter without a floor while every unit test stayed green;
//! - the two principal resolvers ([`ambient_principal`] /
//!   [`principal_from_metadata`]) that answer "who is this run's spend
//!   charged to" from the two places that question is askable;
//! - [`check`], the single admission/floor predicate: is a principal still
//!   inside its ceiling for the period containing "now". Both call sites
//!   this doc used to describe as "later tasks" now exist: the metering
//!   floor arm (`providers::metering::MeteringProvider::enforce_spend_ceiling`)
//!   and the run-admission arm
//!   (`gateway::execution_engine::run_loop::deny_if_over_spend`).

use std::collections::HashMap;

use crate::capability::{CapabilitySlot, MissingSemantics, MutableCapabilitySlot, SlotStatus};
use crate::config::types::policies::{SpendPeriod, SpendPolicy};
use crate::sync_primitives::{Arc, Mutex};

pub mod period;
pub mod sqlite;

#[cfg(test)]
mod tests;

// ============================================================================
// Core types
// ============================================================================

/// Who a dollar is charged to. Always a `users.user_id` or the reserved
/// sentinel — never an agent id (an agent is not a person and cannot hold a
/// budget), and never `u-owner` as a fallback (charging an unattributed run
/// to the machine owner is a silent misattribution, not a default).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Principal {
    User(String),
    Unattributed,
}

impl Principal {
    /// The reserved sentinel key — single-sourced so [`Self::as_key`] and
    /// [`Self::from_key`] can never drift into two different spellings of
    /// "unattributed".
    const UNATTRIBUTED_KEY: &'static str = "@unattributed";

    /// The safe construction path for a [`Self::User`] principal.
    ///
    /// Coerces every reserved sentinel onto [`Self::Unattributed`] so the
    /// `principal_id` row space and the `User(String)` row space can never
    /// collide:
    ///
    /// - the literal [`Self::UNATTRIBUTED_KEY`] (`"@unattributed"`), which
    ///   would otherwise write to the same ledger row as the actual
    ///   [`Self::Unattributed`] variant (see [`Self::as_key`] /
    ///   [`Self::from_key`]);
    /// - any other `@`-prefixed id, since the canonical `users.user_id`
    ///   shape is `u-`-prefixed and any `@`-prefix in production input
    ///   (request metadata, replay rows, test fixtures) is a mis-stamp
    ///   rather than a real user;
    /// - an empty id, which has no user to charge and would round-trip
    ///   through the ledger as a zero-length `principal_id` string the
    ///   SQLite `TEXT PRIMARY KEY` happily stores alongside a real user.
    ///
    /// The matching in `principal_from_metadata` (the untrusted-input site)
    /// uses this constructor verbatim; direct `Principal::User(x)` remains
    /// legal for tests and pattern-matched unwraps, where the input is by
    /// construction a valid `u-`-prefixed string.
    pub fn user(id: impl Into<String>) -> Self {
        let id = id.into();
        if id.is_empty() || id == Self::UNATTRIBUTED_KEY || id.starts_with('@') {
            Self::Unattributed
        } else {
            Self::User(id)
        }
    }

    /// The ledger's primary-key text. `"@unattributed"` cannot collide with a
    /// real id: `users.user_id` values are `u-`-prefixed, and any value
    /// that would collide is coerced to [`Self::Unattributed`] by
    /// [`Self::user`] before construction.
    pub fn as_key(&self) -> &str {
        match self {
            Self::User(id) => id,
            Self::Unattributed => Self::UNATTRIBUTED_KEY,
        }
    }

    /// The inverse of [`Self::as_key`] — reconstruct a principal from a
    /// ledger row's primary-key text. Both [`SpendLedger::principals_in`]
    /// implementations need this exact mapping to turn a stored key back
    /// into a `Principal`; a second, slightly different copy of the
    /// `"@unattributed"` check in the other backend is exactly the kind of
    /// drift this method exists to rule out.
    pub fn from_key(key: &str) -> Self {
        if key == Self::UNATTRIBUTED_KEY {
            Self::Unattributed
        } else {
            Self::User(key.to_string())
        }
    }
}

/// What has been spent in the window that is open right now.
#[derive(Debug, Clone)]
pub struct Spent {
    pub usd: f64,
    /// `CostStatus::Unknown` calls — real spend that carries no price.
    pub unpriced_calls: u64,
    /// `CostStatus::PartialMissingPrice` calls — `usd` is a lower bound.
    pub partial_calls: u64,
    /// Start of the period this figure covers — an echo of the key the
    /// caller queried by, not a value the ledger derives.
    pub period_start_ms: i64,
    /// End of the period this figure covers (the reset instant).
    ///
    /// Always `None` out of every [`SpendLedger`] method: the ledger is
    /// keyed by `period_start_ms` alone and has no notion of period
    /// *length* (`Day` vs `Month`), so it cannot compute this value — only
    /// [`check`], which knows the configured
    /// [`SpendPeriod`](crate::config::types::policies::SpendPeriod), can.
    /// Before this was `Option`, both backends filled it with
    /// `period_start_ms` as a placeholder documented "the real value is
    /// computed later by `check`" — a value that type-checks, reads as a
    /// plausible instant, and is simply false on every direct ledger read.
    /// A raw `ledger.spent_for(..)` call (as a future read surface over the
    /// ledger will make) can no longer produce a plausible-but-wrong reset
    /// time: it gets `None` and must resolve the window itself, the same
    /// way `check` does.
    pub period_end_ms: Option<i64>,
}

/// One increment, keyed to the `CostStatus` that produced it. A caller can
/// only ever move one of the three ledger dimensions per call — there is no
/// constructor that sets a dollar figure *and* leaves the call uncounted, or
/// that counts a call as both partial and unpriced — so "an unpriced call
/// never becomes zero dollars" is a property of the type, not a rule every
/// call site has to remember to uphold. There is deliberately no `Default`:
/// an unpriced call recorded as "nothing happened" is exactly the failure
/// this type exists to make unrepresentable.
#[derive(Debug, Clone, Copy)]
pub enum Delta {
    /// A priced call: `CostStatus::Complete`.
    Usd(f64),
    /// `CostStatus::PartialMissingPrice` — the figure is a lower bound, so
    /// the dollars accumulate AND the call is counted as partial.
    Partial(f64),
    /// `CostStatus::Unknown` — real spend carrying no price. Counts, moves
    /// no dollars, and therefore can never contribute to a denial.
    Unpriced,
}

/// Which ceiling was hit. Shape, not role predicate — see spec §4.8.
///
/// `Copy` + `PartialEq` (not `Eq` — `f64` has no total order), unlike its
/// siblings [`Spent`]/[`Verdict`]: every field is an `f64`, and
/// `ExecutionError::SpendExhausted` / `i18n::ReceiptKind`
/// (`gateway::execution_engine`/`gateway::i18n`) both carry one and need to
/// stay `Copy`/`PartialEq` themselves — `ReceiptKind` already derives both
/// and is matched on by value at every `receipt_kind()` call site, so a
/// non-`Copy` field here would force every one of those sites to
/// restructure around a reference instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Limit {
    /// The caller's own ceiling: both numbers are his own spend, so both are
    /// safe to tell him.
    PerUser { spent: f64, limit: f64 },
    /// The machine-wide ceiling. Deliberately **fieldless**: `user_receipt`
    /// takes no actor and `caller_identity` is dead inside a spawned run, so
    /// there is no point at which "may this person see the machine total?"
    /// could be answered. Machine numbers live on the admin-gated read face.
    Total,
}

/// The admission/floor verdict: allowed with the window's current spend, or
/// denied naming which ceiling was hit and what had already been spent.
///
/// `spent` is always `principal`'s own figures — on `Allowed`, and on
/// **both** `Denied` shapes, including `Denied { limit: Limit::Total, .. }`.
/// It never carries the machine total, even when the machine total is the
/// axis that fired. This is load-bearing, not a style choice: `Limit::Total`
/// is deliberately fieldless *specifically* so the machine-wide figure has
/// no surface to reach a caller who may not be authorized to see it (see
/// its doc). If `spent` substituted the machine total in for the `Total`
/// arm, that number would ride back in through this sibling field —
/// defeating the entire reason `Limit::Total` carries none of its own.
#[derive(Debug, Clone)]
pub enum Verdict {
    Allowed(Spent),
    Denied { limit: Limit, spent: Spent },
}

// ============================================================================
// The ledger trait and its in-process default
// ============================================================================

/// Durable-or-not storage for per-principal, per-period spend. The floor arm
/// (every LLM call) and the admission arm (every run) both read and write
/// through this; the process-global handle below is what lets a durable
/// backend (`spend::sqlite`, wired at boot) stand in for the in-process
/// default without threading a handle through every construction site.
pub trait SpendLedger: Send + Sync {
    fn record(
        &self,
        principal: &Principal,
        period_start_ms: i64,
        delta: Delta,
    ) -> anyhow::Result<()>;
    fn spent_for(&self, principal: &Principal, period_start_ms: i64) -> anyhow::Result<Spent>;

    /// The machine-wide spend that counts against the total ceiling for the
    /// window `[window_start_ms, now]`.
    ///
    /// # Window rule — deliberately wider than an exact key match
    ///
    /// A row counts toward the total iff either:
    ///
    /// - its `period_start_ms >= window_start_ms` — the row was recorded
    ///   under the current period or under a *finer* period whose boundary
    ///   lies inside the current window. This arm is an exact match in
    ///   steady state, and it is what keeps a `Day → Month` policy switch
    ///   from zeroing the total: the day-keyed rows already recorded this
    ///   month keep counting toward the month ceiling;
    /// - its `period_start_ms == coarse_ancestor_start_ms` — the row was
    ///   recorded under a *coarser* period whose window still contains the
    ///   current one. Callers pass the start of the longest period kind
    ///   ([`SpendPeriod::Month`]) containing the current window
    ///   (`period::period_start_ms(now, SpendPeriod::Month)`); when the
    ///   configured period is already `Month` it equals `window_start_ms`
    ///   and this arm is a no-op. This is what keeps a `Month → Day`
    ///   switch from zeroing the total: the month-keyed row keeps counting
    ///   toward today's ceiling.
    ///
    /// # Fail-closed, on purpose
    ///
    /// Both arms can over-count, and that is the chosen direction. A
    /// month-keyed row covers the whole month's spend, not just the day
    /// window it is summed into after a `Month → Day` switch; and under a
    /// steady `Day` policy a row recorded on the 1st of a month is keyed
    /// exactly at the month boundary — indistinguishable from a
    /// month-keyed row — so it counts toward every daily total for the
    /// rest of that month. The alternative direction (under-counting) is
    /// the audit's spend I-1: after a hot `SpendPeriod` change the exact
    /// key match found no rows, the total read zero, and the machine
    /// ceiling silently stopped firing for the rest of the window. The
    /// machine total is the ceiling of last resort: it must fire a little
    /// early before it may ever silently stop. Distinguishing the two row
    /// kinds would require storing the recording period per row — a schema
    /// change this fix deliberately avoids; the over-count is bounded (at
    /// most one coarse period's spend) and ages out as the rows themselves
    /// roll out of the window or are swept.
    ///
    /// [`Self::spent_for`] and [`Self::principals_in`] deliberately keep
    /// exact-key semantics: the per-user ceiling is the axis a principal
    /// can move himself, and the admin read face is a report of the window
    /// that is open now, not a brake — only the machine total gets the
    /// fail-closed window. Closing the analogous per-user hole is a known,
    /// scoped-out follow-up.
    fn total_for(
        &self,
        window_start_ms: i64,
        coarse_ancestor_start_ms: i64,
    ) -> anyhow::Result<Spent>;
    fn sweep_before(&self, period_start_ms: i64) -> anyhow::Result<usize>;

    /// Every principal with a row in `period_start_ms`, `usd` descending
    /// then key ascending — the read face `gateway::handlers::spend` needs
    /// to answer "who has spent, in the window that is open now", which
    /// none of the point-lookup methods above can answer (`spent_for`
    /// requires already knowing the principal; `total_for` only sums).
    ///
    /// Deliberately unbounded (no `LIMIT`, no page size): bounded by
    /// principals-with-spend in one window, and a silently truncated spend
    /// report is worse than a large one. Deliberately ordered rather than
    /// left to backend iteration order: an unordered enumeration would make
    /// the CLI table reshuffle between two calls on unchanged data, which a
    /// reader interprets as the data changing.
    ///
    /// Every returned [`Spent::period_end_ms`] is `None`, exactly as it is
    /// out of [`Self::spent_for`] / [`Self::total_for`]: this trait has no
    /// notion of period *length*, only period *start* — only a caller who
    /// knows the configured [`crate::config::types::policies::SpendPeriod`]
    /// (the handler) can resolve the end instant.
    fn principals_in(&self, period_start_ms: i64) -> anyhow::Result<Vec<(Principal, Spent)>>;
}

/// One ledger row: a principal's accumulated spend within one period.
#[derive(Default)]
struct Row {
    usd: f64,
    unpriced_calls: u64,
    partial_calls: u64,
}

/// The two maps [`InMemorySpendLedger`] keeps mutually consistent under
/// one lock — see the struct's doc for why a single lock, and for the
/// window rule the index exists to serve.
#[derive(Default)]
struct LedgerState {
    /// The ledger rows themselves, keyed by `(principal key, period_start_ms)`.
    rows: HashMap<(String, i64), Row>,
    /// `period_start_ms` → the principal keys holding a row at that period
    /// start. This index is what makes [`SpendLedger::total_for`] and
    /// [`SpendLedger::principals_in`] cheaper than a full scan of `rows`
    /// (spend I-3: both ran on the hot `check_with` path and walked every
    /// retained row of every period and principal): `total_for` walks the
    /// index's keys — a set bounded by the retention horizon plus any
    /// strays a hot `SpendPeriod` switch left behind — and `principals_in`
    /// reads exactly one entry. Every mutation site of `rows` (`record`,
    /// `sweep_before`) maintains this map in the same critical section, so
    /// the two can never be observed mid-drift; the invariant is pinned by
    /// `tests::the_by_period_index_never_drifts_from_the_rows_across_random_ops`.
    by_period: HashMap<i64, Vec<String>>,
}

/// In-process, non-durable [`SpendLedger`]. This is the default the global
/// handle lazily falls back to ([`global_ledger`]) when nothing has called
/// [`install_ledger`] yet — pre-boot code, embedded uses, and any unit test
/// that never wires the durable backend. Production installs
/// `spend::sqlite::SqliteSpendLedger` at boot instead, before the gateway
/// serves its first request.
///
/// Both maps live in a single [`LedgerState`] under one `Mutex`, not two
/// `Mutex<HashMap>` fields: a row insert and its index insert must be one
/// critical section, or a concurrent `total_for`/`principals_in` could
/// observe one map updated and not the other — and two locks would also
/// have to agree on an acquisition order forever after.
#[derive(Default)]
pub struct InMemorySpendLedger {
    state: Mutex<LedgerState>,
}

impl SpendLedger for InMemorySpendLedger {
    fn record(
        &self,
        principal: &Principal,
        period_start_ms: i64,
        delta: Delta,
    ) -> anyhow::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let key = (principal.as_key().to_string(), period_start_ms);
        let is_new_row = !state.rows.contains_key(&key);
        let row = state.rows.entry(key).or_default();
        match delta {
            Delta::Usd(usd) => {
                if !usd.is_finite() {
                    // NaN / ±inf in the USD column silently disables the
                    // spend ceiling: `ceiling_blown(NaN, anything)` is
                    // always false (IEEE 754 NaN comparisons), so once a
                    // single corrupt price lands in the row the principal
                    // is allowed to continue calling forever. Coerce to
                    // 0.0 + bump unpriced_calls so the 'this call had a
                    // price it couldn't represent' signal is loud and the
                    // ceiling continues to evaluate against a real number.
                    row.unpriced_calls += 1;
                } else {
                    row.usd += usd;
                }
            }
            Delta::Partial(usd) => {
                if !usd.is_finite() {
                    row.unpriced_calls += 1;
                } else {
                    row.usd += usd;
                    row.partial_calls += 1;
                }
            }
            Delta::Unpriced => row.unpriced_calls += 1,
        }
        if is_new_row {
            // First time this principal appears in this period: index it.
            // Same critical section as the row insert — see `LedgerState`'s
            // doc for why the two maps must never be observed mid-drift.
            state
                .by_period
                .entry(period_start_ms)
                .or_default()
                .push(principal.as_key().to_string());
        }
        Ok(())
    }

    fn spent_for(&self, principal: &Principal, period_start_ms: i64) -> anyhow::Result<Spent> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let row = state
            .rows
            .get(&(principal.as_key().to_string(), period_start_ms));
        Ok(Spent {
            usd: row.map_or(0.0, |r| r.usd),
            unpriced_calls: row.map_or(0, |r| r.unpriced_calls),
            partial_calls: row.map_or(0, |r| r.partial_calls),
            period_start_ms,
            // See `Spent::period_end_ms`'s doc: the ledger does not know
            // `SpendPeriod`, so it cannot compute this — only `check` can.
            period_end_ms: None,
        })
    }

    fn total_for(
        &self,
        window_start_ms: i64,
        coarse_ancestor_start_ms: i64,
    ) -> anyhow::Result<Spent> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut usd = 0.0;
        let mut unpriced_calls = 0;
        let mut partial_calls = 0;
        // Walk the period index, not the row map: the number of distinct
        // period starts is bounded by the retention horizon (plus strays a
        // hot `SpendPeriod` switch left behind), so this is O(distinct
        // period starts + matching rows) rather than O(every retained row)
        // on the hot `check_with` path — spend I-3. The window rule itself
        // (`>= window_start` OR `== coarse_ancestor_start`) is specified on
        // the trait method; both backends implement it identically.
        for (period, principal_keys) in state.by_period.iter() {
            if *period < window_start_ms && *period != coarse_ancestor_start_ms {
                continue;
            }
            for principal_key in principal_keys {
                let Some(row) = state.rows.get(&(principal_key.clone(), *period)) else {
                    // The index is maintained in the same critical section
                    // as every row mutation, so a miss here is a broken
                    // invariant, not a race — loud in debug builds, skipped
                    // (never panicked on) in release, matching the ledger's
                    // fail-open read posture.
                    debug_assert!(
                        false,
                        "by_period index names ({principal_key}, {period}) but rows has no such key"
                    );
                    continue;
                };
                usd += row.usd;
                unpriced_calls += row.unpriced_calls;
                partial_calls += row.partial_calls;
            }
        }
        Ok(Spent {
            usd,
            unpriced_calls,
            partial_calls,
            period_start_ms: window_start_ms,
            // See `Spent::period_end_ms`'s doc.
            period_end_ms: None,
        })
    }

    fn sweep_before(&self, period_start_ms: i64) -> anyhow::Result<usize> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let before = state.rows.len();
        state.rows.retain(|key, _| key.1 >= period_start_ms);
        // The same cutoff on the index, in the same critical section: an
        // index entry left behind for a swept period would keep answering
        // `principals_in`/`total_for` for rows that no longer exist.
        state
            .by_period
            .retain(|period, _| *period >= period_start_ms);
        Ok(before - state.rows.len())
    }

    fn principals_in(&self, period_start_ms: i64) -> anyhow::Result<Vec<(Principal, Spent)>> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // One index lookup instead of a full `rows` scan — spend I-3. Every
        // key the index names for this period has a row by construction;
        // the `filter_map` is the same defensive skip `total_for` uses.
        let mut out: Vec<(Principal, Spent)> = state
            .by_period
            .get(&period_start_ms)
            .map(|principal_keys| {
                principal_keys
                    .iter()
                    .filter_map(|key| {
                        let row = state.rows.get(&(key.clone(), period_start_ms))?;
                        Some((
                            Principal::from_key(key),
                            Spent {
                                usd: row.usd,
                                unpriced_calls: row.unpriced_calls,
                                partial_calls: row.partial_calls,
                                period_start_ms,
                                // See `Spent::period_end_ms`'s doc.
                                period_end_ms: None,
                            },
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // `HashMap` index values are in insertion order, which is not the
        // contracted order — see the trait method's doc for why this must
        // be sorted explicitly rather than relying on iteration order.
        out.sort_by(|(a_principal, a_spent), (b_principal, b_spent)| {
            b_spent
                .usd
                .partial_cmp(&a_spent.usd)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a_principal.as_key().cmp(b_principal.as_key()))
        });
        Ok(out)
    }
}

// ============================================================================
// Process-global handles
// ============================================================================
//
// Both are `crate::capability` slots rather than bare `OnceLock`s, and they are
// the anchor that module argues from. §5.22 round-7 recorded the failure right
// here: `spend.query` reporting `configured: false` is a true statement about a
// box with no ceiling AND a true statement about a box that configured one
// whose handle boot never installed, and nothing a caller can reach tells the
// two apart. The slot's `outcome()` is what separates them; the
// `MissingSemantics` below is the sentence a diagnostic shows when it is
// `None`.
//
// The read side is unchanged. `global_ledger` / `current_policy` still always
// return something, so the admission and floor arms never special-case
// "nothing installed yet" (mirroring
// `thinker::prompt_builder::cache_monitor::global_cache_monitor`), and the
// install side is still first-writer-wins with a hot-apply path of its own
// (mirroring `tools::result_store::set_global_tool_result_store` /
// `providers::route_handle::global_route_handle`).

static GLOBAL_LEDGER: CapabilitySlot<Arc<dyn SpendLedger>> = CapabilitySlot::new(
    "spend/ledger",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "an in-memory ledger that resets every restart",
    },
);

/// The lazy default [`global_ledger`] falls back to — kept OUT of the slot on
/// purpose.
///
/// Latching it into `GLOBAL_LEDGER` is the obvious rewrite of the old
/// `get_or_init`, and it would make this handle's own `MissingSemantics` false:
/// `install` stamps `Outcome::Installed`, so the first pre-boot read would
/// teach every later diagnostic that boot installed a ledger boot never
/// touched. That is the "confident lie … worse than today's silence" the
/// `capability` module doc exists to prevent, and it is worse here than a bare
/// `OnceLock` was. A separate `LazyLock` keeps the identity that matters (every
/// caller shares one in-memory ledger, so spend recorded through it
/// accumulates) while leaving `GLOBAL_LEDGER.outcome()` honestly `None`.
///
/// ⚠️ One deliberate behavioural delta, and it is the only one **this file's**
/// migration makes. The round makes exactly one other, in
/// [`crate::config::defaults_override`], and it is the same latch on a
/// different handle — that file says so too. (Both sentences used to read "the
/// only one", which at the level a later reader reads them is a
/// contradiction.) Here: the old `GLOBAL_LEDGER.get_or_init` LATCHED the fallback into the
/// handle, so any read before boot made the subsequent `install_ledger` a
/// silent no-op. Now boot's install wins. In production nothing reads before
/// `start/mod.rs` installs the durable backend, so the window is theoretical
/// either way — but of the two answers, "the durable ledger boot installed is
/// the one in use" is the one this module is trying to make true.
static LEDGER_FALLBACK: std::sync::LazyLock<Arc<dyn SpendLedger>> =
    std::sync::LazyLock::new(|| Arc::new(InMemorySpendLedger::default()) as Arc<dyn SpendLedger>);

/// Install the process-wide ledger. Idempotent — a second call is silently
/// ignored so multiple boot paths cannot stomp each other (mirrors
/// `set_global_tool_result_store`). Boot calls this once, before the gateway
/// serves any request, with the durable `spend::sqlite` backend.
pub fn install_ledger(ledger: Arc<dyn SpendLedger>) {
    let _ = GLOBAL_LEDGER.install(ledger);
}

/// Read the process-wide ledger, falling back to the shared
/// [`InMemorySpendLedger`] when nothing has called [`install_ledger`] yet —
/// mirrors
/// [`global_cache_monitor`](crate::thinker::prompt_builder::cache_monitor::global_cache_monitor):
/// the admission and floor arms must always have something to call.
pub fn global_ledger() -> Arc<dyn SpendLedger> {
    match GLOBAL_LEDGER.get() {
        Some(ledger) => ledger.clone(),
        // Not `unwrap_or(&LEDGER_FALLBACK)`: that forces the `LazyLock` on
        // every call, including the installed path this arm never runs on.
        None => LEDGER_FALLBACK.clone(),
    }
}

static GLOBAL_POLICY: MutableCapabilitySlot<SpendPolicy> = MutableCapabilitySlot::new(
    "spend/policy",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "SpendPolicy::default() — enabled() == false, i.e. no ceiling",
    },
);

/// Install the process-wide policy handle at boot, seeded from
/// `[policies.spend]`. Idempotent, like [`install_ledger`]: a second call is
/// silently ignored.
pub fn install_policy(policy: crate::config::types::policies::SpendPolicy) {
    let _ = GLOBAL_POLICY.install(policy);
}

/// Hot-apply path for `[policies.spend]` (the live-reload round): store a new
/// policy into the already-installed handle. Returns `false` when no handle
/// has been installed yet, so the live-apply verdict can downgrade to
/// `Restart` honestly instead of reporting `Live` for a knob that silently
/// did nothing — mirrors `providers::route_handle::try_global_route_handle`'s
/// `None` arm.
///
/// The `false` branch is not exercisable through this global. `install_policy`
/// is deliberately idempotent and `cargo test --lib` runs every unit test in
/// this crate in one process, so any test anywhere that installs a policy makes
/// this handle `Some` for the rest of the binary's life, in an order this crate
/// does not control. It used to be reached through an injectable
/// `update_policy_into(handle, policy)`; the slot type carries that seam now —
/// `capability::tests::update_before_install_returns_false_and_changes_nothing`
/// pins the same contract against a slot the test owns.
///
/// `#[must_use]` for the same reason [`MutableCapabilitySlot::update`] carries
/// it one level down: a caller who writes `update_policy(p);` and drops the
/// bool reports a successful hot-apply for a store that never happened. The
/// argument for keeping the slot type at all was "the type enforces it rather
/// than each call site remembering" — that applies here too, and a wrapper that
/// forwards a `#[must_use]` value without being `#[must_use]` itself is a hole
/// in exactly that argument.
#[must_use]
pub fn update_policy(policy: crate::config::types::policies::SpendPolicy) -> bool {
    GLOBAL_POLICY.update(policy)
}

/// The effective policy right now. Mirrors `global_cache_monitor()`: always
/// returns something, so `spend::check` never has to special-case "nothing
/// installed yet" — an uninstalled handle reads as
/// `SpendPolicy::default()` (no ceiling on either axis, i.e. disabled),
/// which is the correct behavior for unit tests, pre-boot code, and embedded
/// uses that never touch `[policies.spend]`.
///
/// ⚠️ That default is precisely the round-7 indistinguishable read: it is the
/// right value to return and it says nothing about whether boot got here. Ask
/// [`global_policy_slot`]`().outcome()` for that — never this function.
pub fn current_policy() -> crate::config::types::policies::SpendPolicy {
    GLOBAL_POLICY
        .load()
        .map_or_else(crate::config::types::policies::SpendPolicy::default, |p| {
            (**p).clone()
        })
}

/// The two handles above, type-erased for the roster.
///
/// `pub(crate) fn` rather than `pub static`: the roster built in
/// [`crate::capability::ALL_SLOTS`] needs one `&dyn SlotStatus` per handle, and
/// making 45 statics public to assemble it would widen the crate's surface by
/// 45 items to answer one question. Erasing at the accessor also means a
/// roster consumer gets exactly `id` / `missing` / `outcome` and can never
/// reach the value — a diagnostic has no business reading the ledger.
///
/// `const fn`, not merely `fn`: `ALL_SLOTS` is a `static` array of trait
/// objects, and a `static` initializer must be a constant expression, so every
/// accessor it calls has to be callable at compile time. The body is nothing
/// but an unsizing coercion of an already-`'static` reference, so `const` adds
/// no restriction the body did not already meet.
///
/// These carried `#[allow(dead_code)]` from the day they were written until
/// `ALL_SLOTS` landed and gave them their first production caller — see that
/// static's doc, and `census::every_installed_global_is_a_capability_slot` /
/// `census::every_declared_slot_is_in_the_roster` for the guards that close
/// the round these two handles anchor.
pub(crate) const fn global_ledger_slot() -> &'static dyn SlotStatus {
    &GLOBAL_LEDGER
}

/// See [`global_ledger_slot`].
pub(crate) const fn global_policy_slot() -> &'static dyn SlotStatus {
    &GLOBAL_POLICY
}

// ============================================================================
// The two principal resolvers
// ============================================================================

/// Floor arm — called from inside the run's task-local nest (everything
/// under `run_loop::with_request_scope`).
///
/// Reads [`crate::scope::current_room_author`] directly, **not**
/// [`crate::scope::ambient_room_author`]: the latter filters through
/// `room_author`, which returns `None` for any non-`ScopeId::Project` scope —
/// that is the room transcript byline, not a general actor accessor, and
/// using it here would charge nearly every install's spend to
/// `@unattributed` with no test going red. See [`principal_from_metadata`]
/// for why the two are unconditionally equivalent.
#[must_use]
pub fn ambient_principal() -> Principal {
    crate::scope::current_room_author()
        .or_else(crate::scope::ambient_owner)
        .map(Principal::user)
        .unwrap_or(Principal::Unattributed)
}

/// Admission arm — called before the run's task-local nest exists, off the
/// request's own metadata map.
///
/// Reads the same two facts [`ambient_principal`] does, in the same order:
/// `AUTHOR_USER_KEY` (this turn's speaker), then the scope owner — resolved
/// through [`crate::scope::scope_from_metadata`], **not** a bare
/// `meta.get(OWNER_META_KEY)`. That routing is what makes the two resolvers
/// equivalent unconditionally rather than by convention: `ambient_principal`'s
/// own fallback, [`crate::scope::ambient_owner`], derives the owner from
/// `current_scope()`, and `with_request_scope` seeds `current_scope()` with
/// `scope_from_metadata(&request.metadata)` — the same function, with the
/// same all-or-nothing requirement on `OWNER_META_KEY` **and**
/// `SCOPE_META_KEY` both being present and parseable. A bare
/// `meta.get(OWNER_META_KEY)` would resolve `Principal::User` on metadata
/// carrying an owner key but no (or an unparseable) scope key, while the
/// floor arm resolves `Unattributed` on that exact input — see
/// `run_loop::tests::spend_principal_resolvers_agree_on_an_owner_key_with_no_scope_key`
/// for that case pinned, alongside
/// `run_loop::tests::run_loop_seeds_scope_from_request_metadata` and this
/// module's own equivalence guards.
///
/// Neither resolver may call `visibility::ambient_actor()` or
/// `turn_context::current_agent_id()`: `ambient_actor`'s third fallback arm
/// is an agent id, and an agent is not a person and cannot hold a budget.
#[must_use]
pub fn principal_from_metadata(meta: &HashMap<String, String>) -> Principal {
    // Routes both the author key and the scope-owner fallback through
    // `Principal::user`, so any `"@unattributed"` (or other `@`-prefixed)
    // string an upstream caller stamps into `AUTHOR_USER_KEY` collapses to
    // `Unattributed` instead of writing to the same `principal_id` row
    // space as the sentinel — silent misattribution rather than a panic.
    meta.get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
        .cloned()
        .or_else(|| crate::scope::scope_from_metadata(meta).map(|attr| attr.owner_user_id))
        .map(Principal::user)
        .unwrap_or(Principal::Unattributed)
}

// ============================================================================
// check — the single admission/floor predicate
// ============================================================================

/// Is `principal` still inside its spend ceiling for the period containing
/// `now_ms`? The single predicate both the metering floor arm (every LLM
/// call) and the admission arm (every run) call — see the plan for why a
/// second, open-coded copy of this logic in either arm is exactly the bug
/// this function exists to rule out.
///
/// Reads the process-wide policy and ledger ([`current_policy`],
/// [`global_ledger`]) and delegates to [`check_with`], which takes both
/// explicitly and carries the actual logic — see that function's doc for
/// why the split exists.
#[must_use]
pub fn check(principal: &Principal, now_ms: i64) -> Verdict {
    let policy = current_policy();
    check_with(principal, now_ms, &policy, &*global_ledger())
}

/// The logic [`check`] delegates to, with `policy` and `ledger` taken as
/// parameters instead of read from the process globals.
///
/// This split exists for testability, not layering for its own sake:
/// [`install_ledger`] and [`install_policy`] populate `OnceLock`s that are
/// set once for the life of the process, and `cargo test --lib` runs every
/// unit test in this crate in one process. G8 needs a `SpendLedger` whose
/// every method panics, to turn "the disabled policy never touches the
/// ledger" from a claim into an assertion; G9 and G10 each need their own
/// freshly-seeded ledger. None of that is expressible by racing to install
/// a single process-wide `OnceLock` from three different tests — the first
/// `install_ledger` call anywhere in this binary would win, and the other
/// two would silently get its ledger instead of their own. Taking `policy`
/// and `ledger` as plain parameters sidesteps the whole hazard: every test
/// builds its own, with no shared mutable process state to race on.
///
/// `pub(crate)` rather than private: `providers::metering`'s G4 test needs
/// the exact same hazard-free construction — a freshly-built ledger, an
/// enormous run of `Delta::Unpriced` writes, then one `Verdict` read — and
/// this is the single predicate both the floor arm and the admission arm
/// call, so a second copy of it in `providers::metering` for testing alone
/// would be exactly the "two answers" bug this function exists to prevent.
pub(crate) fn check_with(
    principal: &Principal,
    now_ms: i64,
    policy: &SpendPolicy,
    ledger: &dyn SpendLedger,
) -> Verdict {
    let period_start_ms = period::period_start_ms(now_ms, policy.period);
    let period_end_ms = period::period_end_ms(now_ms, policy.period);

    if !policy.enabled() {
        // Neither ceiling is configured: return without calling a single
        // `SpendLedger` method — no query, no cache fill, no row. A
        // single-user box with `[policies.spend]` absent must be
        // byte-identical, on every request, to one without this feature at
        // all. See G8.
        return Verdict::Allowed(zero_spent(period_start_ms, period_end_ms));
    }

    // `principal`'s own spend rides every returned `Verdict` — `Allowed`
    // and both `Denied` shapes — as `spent`, because that field is always
    // the caller's own number, read unconditionally once the policy is
    // enabled. Never the machine total: see `Limit::Total`'s doc on why the
    // machine total must never ride alongside it, in this field or any
    // other the caller can reach.
    let spent = resolve_read(
        ledger.spent_for(principal, period_start_ms),
        period_start_ms,
        period_end_ms,
        |error| {
            tracing::error!(
                %error,
                principal = principal.as_key(),
                period_start_ms,
                "spend::check: SpendLedger::spent_for failed; treating this principal's spend as \
                 zero for this check rather than turning a ledger read failure into a denial for \
                 every request",
            );
        },
    );

    // Total first: it is the ceiling `principal` cannot move by asking
    // someone else to raise their own line, so it is the one named when
    // both are blown. Only read it when the axis is actually configured —
    // an install with `total_usd` unset must not pay for a query whose
    // answer can never matter.
    if let Some(total_limit) = policy.total_usd {
        // Fail-closed window for the machine total (spend I-1): rows
        // recorded before a hot `SpendPeriod` switch are keyed by the OLD
        // policy's boundaries, so an exact `period_start_ms` match reads
        // zero for the rest of the current window and the total ceiling
        // silently stops firing. `total_for` therefore sums every row keyed
        // inside the current window (covers a finer old period, e.g.
        // Day → Month) plus the row keyed at the start of the coarsest
        // period containing `now` (covers a coarser old period, e.g.
        // Month → Day). The deliberate over-count that rule accepts — and
        // why over-counting is the right direction for the ceiling of last
        // resort — is on the trait method's doc.
        let coarse_ancestor_start_ms = period::period_start_ms(now_ms, SpendPeriod::Month);
        let total = resolve_read(
            ledger.total_for(period_start_ms, coarse_ancestor_start_ms),
            period_start_ms,
            period_end_ms,
            |error| {
                tracing::error!(
                    %error,
                    period_start_ms,
                    "spend::check: SpendLedger::total_for failed; treating the machine total as \
                     zero for this check rather than turning a ledger read failure into a denial \
                     for every request",
                );
            },
        );
        if ceiling_blown(total.usd, total_limit) {
            return Verdict::Denied {
                limit: Limit::Total,
                spent,
            };
        }
    }

    if let Some(per_user_limit) = policy.per_user_usd {
        if ceiling_blown(spent.usd, per_user_limit) {
            return Verdict::Denied {
                limit: Limit::PerUser {
                    spent: spent.usd,
                    limit: per_user_limit,
                },
                spent,
            };
        }
    }

    Verdict::Allowed(spent)
}

/// The one place the ceiling comparison is written. `>=`, not `>`: a
/// principal exactly at the ceiling is denied, not let through for one more
/// call — see G10.
fn ceiling_blown(spent_usd: f64, limit_usd: f64) -> bool {
    spent_usd >= limit_usd
}

/// A zero-spend `Spent` for `period_start_ms`/`period_end_ms` — the
/// disabled-policy fast path (no ledger involved at all) and the fail-open
/// fallback in [`resolve_read`] (a ledger read that errored) both need
/// exactly this value.
fn zero_spent(period_start_ms: i64, period_end_ms: i64) -> Spent {
    Spent {
        usd: 0.0,
        unpriced_calls: 0,
        partial_calls: 0,
        period_start_ms,
        period_end_ms: Some(period_end_ms),
    }
}

/// Fold one `SpendLedger` read into a `Spent` carrying the real reset
/// instant (every `SpendLedger` method hands back `period_end_ms: None` —
/// see that field's doc — `check` is where it gets resolved).
///
/// Fails open on a ledger error: logs loudly via `on_error` and returns a
/// zero `Spent` rather than propagating the error out of `check` as a
/// denial. A spend ceiling is a governance feature, not a safety wall —
/// letting a transient ledger read failure deny *every* request for as long
/// as the failure lasts would turn a database hiccup into a full outage,
/// which is a worse failure mode than a bounded, loudly-logged window of
/// unmetered spend (P7: graceful degradation over a hard failure).
///
/// **This is a deliberate exception to "a gate keyed on state must treat
/// `Err` as refusal, not passage"**, not an oversight of it. That criterion
/// is for *authorization* gates, where an unreadable state is a security
/// unknown and passing it is a hole. `spend::check` is a cost ceiling, not
/// an authorization boundary — the plan already documents it as "a ceiling,
/// not a hard cap" (the check-before/record-after ordering permits a
/// bounded overshoot by design). Failing closed here would not preserve
/// safety; it would convert a transient SQLite read error into every LLM
/// call in the process being refused, which is exactly the "a door with no
/// handle is not a gate, it is a wall" shape — and the realistic operator
/// response to a wall is to disable the feature outright, the worst
/// available outcome. Pinned by
/// `tests::ledger_read_error_fails_open_and_is_logged_not_denied`.
fn resolve_read(
    result: anyhow::Result<Spent>,
    period_start_ms: i64,
    period_end_ms: i64,
    on_error: impl FnOnce(&anyhow::Error),
) -> Spent {
    match result {
        Ok(mut spent) => {
            spent.period_end_ms = Some(period_end_ms);
            spent
        }
        Err(error) => {
            on_error(&error);
            zero_spent(period_start_ms, period_end_ms)
        }
    }
}
