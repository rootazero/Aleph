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
//!   constructor parameters because `MeteringProvider` has 8 production
//!   construction sites; threading a handle through them would wire some and
//!   miss others, and the missed ones would meter without a floor while
//!   every unit test stayed green;
//! - the two principal resolvers ([`ambient_principal`] /
//!   [`principal_from_metadata`]) that answer "who is this run's spend
//!   charged to" from the two places that question is askable.
//!
//! No call site reads any of this yet — that lands with `spend::check`
//! (admission) and the metering floor arm in later rounds.

use std::collections::HashMap;

use crate::sync_primitives::{Arc, Mutex};

pub mod period;

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
    /// The ledger's primary-key text. `"@unattributed"` cannot collide with a
    /// real id: `users.user_id` values are `u-`-prefixed.
    pub fn as_key(&self) -> &str {
        match self {
            Self::User(id) => id,
            Self::Unattributed => "@unattributed",
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
    pub period_start_ms: i64,
    pub period_end_ms: i64,
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
#[derive(Debug, Clone)]
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
    fn record(&self, principal: &Principal, period_start_ms: i64, delta: Delta) -> anyhow::Result<()>;
    fn spent_for(&self, principal: &Principal, period_start_ms: i64) -> anyhow::Result<Spent>;
    fn total_for(&self, period_start_ms: i64) -> anyhow::Result<Spent>;
    fn sweep_before(&self, period_start_ms: i64) -> anyhow::Result<usize>;
}

/// One ledger row: a principal's accumulated spend within one period.
#[derive(Default)]
struct Row {
    usd: f64,
    unpriced_calls: u64,
    partial_calls: u64,
}

/// In-process, non-durable [`SpendLedger`]. This is the default the global
/// handle lazily falls back to ([`global_ledger`]) when nothing has called
/// [`install_ledger`] yet — pre-boot code, embedded uses, and any unit test
/// that never wires the durable backend. Production installs
/// `spend::sqlite::SqliteSpendLedger` at boot instead, before the gateway
/// serves its first request.
#[derive(Default)]
pub struct InMemorySpendLedger {
    rows: Mutex<HashMap<(String, i64), Row>>,
}

impl SpendLedger for InMemorySpendLedger {
    fn record(&self, principal: &Principal, period_start_ms: i64, delta: Delta) -> anyhow::Result<()> {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let row = rows
            .entry((principal.as_key().to_string(), period_start_ms))
            .or_default();
        match delta {
            Delta::Usd(usd) => row.usd += usd,
            Delta::Partial(usd) => {
                row.usd += usd;
                row.partial_calls += 1;
            }
            Delta::Unpriced => row.unpriced_calls += 1,
        }
        Ok(())
    }

    fn spent_for(&self, principal: &Principal, period_start_ms: i64) -> anyhow::Result<Spent> {
        let rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let row = rows.get(&(principal.as_key().to_string(), period_start_ms));
        Ok(Spent {
            usd: row.map_or(0.0, |r| r.usd),
            unpriced_calls: row.map_or(0, |r| r.unpriced_calls),
            partial_calls: row.map_or(0, |r| r.partial_calls),
            period_start_ms,
            // The ledger has no notion of period length (Day vs Month) — it
            // only ever sees `period_start_ms`. The true reset instant is
            // computed by `spend::check` (which does know the configured
            // `SpendPeriod`) and folded into the `Spent` it returns; this
            // placeholder is never the value a consumer outside `check`
            // should read.
            period_end_ms: period_start_ms,
        })
    }

    fn total_for(&self, period_start_ms: i64) -> anyhow::Result<Spent> {
        let rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let mut usd = 0.0;
        let mut unpriced_calls = 0;
        let mut partial_calls = 0;
        for (key, row) in rows.iter() {
            if key.1 != period_start_ms {
                continue;
            }
            usd += row.usd;
            unpriced_calls += row.unpriced_calls;
            partial_calls += row.partial_calls;
        }
        Ok(Spent {
            usd,
            unpriced_calls,
            partial_calls,
            period_start_ms,
            period_end_ms: period_start_ms,
        })
    }

    fn sweep_before(&self, period_start_ms: i64) -> anyhow::Result<usize> {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let before = rows.len();
        rows.retain(|key, _| key.1 >= period_start_ms);
        Ok(before - rows.len())
    }
}

// ============================================================================
// Process-global handles
// ============================================================================
//
// Same shape as `thinker::prompt_builder::cache_monitor::global_cache_monitor`
// on the read side (lazy `OnceLock`, the admission/floor arms always have
// something to call) and as `tools::result_store::set_global_tool_result_store`
// / `providers::route_handle::global_route_handle` on the install side (boot
// wins the first install; a config-write path hot-applies afterward through
// its own interior mutability rather than re-installing).

static GLOBAL_LEDGER: std::sync::OnceLock<Arc<dyn SpendLedger>> = std::sync::OnceLock::new();

/// Install the process-wide ledger. Idempotent — a second call is silently
/// ignored so multiple boot paths cannot stomp each other (mirrors
/// `set_global_tool_result_store`). Boot calls this once, before the gateway
/// serves any request, with the durable `spend::sqlite` backend.
pub fn install_ledger(ledger: Arc<dyn SpendLedger>) {
    let _ = GLOBAL_LEDGER.set(ledger);
}

/// Read the process-wide ledger, lazily installing [`InMemorySpendLedger`] if
/// nothing has called [`install_ledger`] yet — mirrors
/// [`global_cache_monitor`](crate::thinker::prompt_builder::cache_monitor::global_cache_monitor):
/// the admission and floor arms must always have something to call.
pub fn global_ledger() -> Arc<dyn SpendLedger> {
    GLOBAL_LEDGER
        .get_or_init(|| Arc::new(InMemorySpendLedger::default()) as Arc<dyn SpendLedger>)
        .clone()
}

static GLOBAL_POLICY: std::sync::OnceLock<arc_swap::ArcSwap<crate::config::types::policies::SpendPolicy>> =
    std::sync::OnceLock::new();

/// Install the process-wide policy handle at boot, seeded from
/// `[policies.spend]`. Idempotent, like [`install_ledger`]: a second call is
/// silently ignored.
pub fn install_policy(policy: crate::config::types::policies::SpendPolicy) {
    let _ = GLOBAL_POLICY.set(arc_swap::ArcSwap::from_pointee(policy));
}

/// Hot-apply path for `[policies.spend]` (the live-reload round): store a new
/// policy into the already-installed handle. Returns `false` when no handle
/// has been installed yet, so the live-apply verdict can downgrade to
/// `Restart` honestly instead of reporting `Live` for a knob that silently
/// did nothing — mirrors `providers::route_handle::try_global_route_handle`'s
/// `None` arm.
pub fn update_policy(policy: crate::config::types::policies::SpendPolicy) -> bool {
    match GLOBAL_POLICY.get() {
        Some(cell) => {
            cell.store(Arc::new(policy));
            true
        }
        None => false,
    }
}

/// The effective policy right now. Mirrors `global_cache_monitor()`: always
/// returns something, so `spend::check` never has to special-case "nothing
/// installed yet" — an uninstalled handle reads as
/// `SpendPolicy::default()` (no ceiling on either axis, i.e. disabled),
/// which is the correct behavior for unit tests, pre-boot code, and embedded
/// uses that never touch `[policies.spend]`.
pub fn current_policy() -> crate::config::types::policies::SpendPolicy {
    GLOBAL_POLICY
        .get()
        .map_or_else(crate::config::types::policies::SpendPolicy::default, |cell| {
            (*cell.load_full()).clone()
        })
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
        .map(Principal::User)
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
    meta.get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
        .cloned()
        .or_else(|| crate::scope::scope_from_metadata(meta).map(|attr| attr.owner_user_id))
        .map(Principal::User)
        .unwrap_or(Principal::Unattributed)
}
