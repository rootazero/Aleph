//! Public telecom run admission (public session isolation contract §5).
//!
//! Public runs never enter the owner busy lane, queue, or durable journal.
//! This registry is their whole admission state, in memory only:
//!
//! - at most one live run per telecom binding (`TelecomProvenance::binding_ref`);
//! - a global cap on concurrent public runs;
//! - the explicit "telecom public" spend principal
//!   ([`Principal::TelecomPublic`]) — never inferred from metadata, never the
//!   owner's budget.
//!
//! Spend admission has two modes, chosen per call by the live
//! `[policies.spend]` policy:
//!
//! - **formal ledger** (policy enabled): admission asks `spend::check` about
//!   `Principal::TelecomPublic` against the process ledger and policy
//!   (`telecom_public_usd`, falling back to `per_user_usd`, plus the machine
//!   `total_usd`). The ledger is fed by the public run's
//!   `MeteringProvider::with_principal(.., Principal::TelecomPublic)` (see
//!   `providers::metering::metered_public_provider`), not by this registry;
//! - **bounded fallback** (policy disabled — no ceiling configured, so the
//!   ledger is neither read nor written): a per-process token counter vs
//!   `token_cap`. This is a crude safety bound, NOT ledger accounting.
//!
//! Busy / over-cap callers receive a fixed notice; nothing is queued.

use std::collections::HashMap;

use crate::config::types::policies::SpendPolicy;
use crate::spend::{Principal, SpendLedger, Verdict};
use crate::sync_primitives::{Arc, AtomicU64, Mutex, OnceLock, Ordering};
use tokio_util::sync::CancellationToken;

use super::{TelecomProvenance, TelecomPublicContext};

/// The explicit spend principal of every public telecom run — a typed
/// variant, never derived from metadata or the ambient owner.
const PUBLIC_PRINCIPAL: Principal = Principal::TelecomPublic;

/// Fixed notice for a busy binding or a full global public lane.
pub const PUBLIC_BUSY_NOTICE: &str = "稍后再试 / Please try again later.";

/// Fixed notice when the public spend principal is over its cap.
pub const PUBLIC_OVER_SPEND_NOTICE: &str =
    "暂时无法回复，请稍后再试 / Unable to reply right now, please try again later.";

/// Default global concurrent public runs.
pub const DEFAULT_PUBLIC_GLOBAL_CAP: usize = 4;
/// Default token cap of the bounded fallback (used only while no spend policy
/// is enabled), per process lifetime.
pub const DEFAULT_PUBLIC_TOKEN_CAP: u64 = 200_000;

struct Entry {
    id: u64,
    caller_digest: super::public_context::CallerDigest,
    cancel: CancellationToken,
}

/// Where spend admission reads the public principal's spend from.
enum SpendGate {
    /// The process-wide `[policies.spend]` policy and ledger (production).
    Process,
    /// Bounded token counter only; never reads the process spend globals.
    /// Deterministic for tests that do not own the spend serial group.
    FallbackOnly,
    /// A caller-supplied policy and ledger (tests), instead of the globals.
    #[cfg(test)]
    Injected {
        policy: SpendPolicy,
        ledger: Arc<dyn SpendLedger>,
    },
}

impl SpendGate {
    /// The policy/ledger pair when formal ledger admission applies right
    /// now; `None` selects the bounded fallback.
    fn formal(&self) -> Option<(SpendPolicy, Arc<dyn SpendLedger>)> {
        let (policy, ledger) = match self {
            Self::Process => (
                crate::spend::current_policy(),
                crate::spend::global_ledger(),
            ),
            Self::FallbackOnly => return None,
            #[cfg(test)]
            Self::Injected { policy, ledger } => (policy.clone(), Arc::clone(ledger)),
        };
        policy.enabled().then_some((policy, ledger))
    }
}

/// In-memory admission state of public telecom runs.
pub struct PublicRunRegistry {
    active: Mutex<HashMap<String, Entry>>,
    next_id: AtomicU64,
    global_cap: usize,
    token_cap: u64,
    /// Bounded-fallback counter; moves only while no spend policy is enabled.
    tokens_spent: AtomicU64,
    gate: SpendGate,
}

/// Outcome of [`PublicRunRegistry::try_admit`].
pub enum PublicAdmission {
    Admitted(PublicRunPermit),
    /// A run for this binding is already live.
    Busy,
    /// The global public lane is full.
    OverGlobalCap,
    /// The telecom-public principal is over its ledger cap (or, with no spend
    /// policy enabled, the bounded fallback token cap).
    OverSpend,
}

impl PublicAdmission {
    /// Fixed caller-facing notice for a refusal; `None` when admitted.
    pub fn notice(&self) -> Option<&'static str> {
        match self {
            Self::Admitted(_) => None,
            Self::Busy | Self::OverGlobalCap => Some(PUBLIC_BUSY_NOTICE),
            Self::OverSpend => Some(PUBLIC_OVER_SPEND_NOTICE),
        }
    }
}

/// Live admission of one public run. Dropping it releases the binding slot.
pub struct PublicRunPermit {
    registry: Arc<PublicRunRegistry>,
    binding: String,
    id: u64,
    caller_digest: super::public_context::CallerDigest,
    provenance: TelecomProvenance,
    cancel: CancellationToken,
}

/// Crate-private witness minted only by validating a live admission. It cannot
/// admit a run or release its slot; the owned permit remains the sole RAII owner.
#[derive(Clone)]
pub(crate) struct PublicRunIdentity {
    registry: Arc<PublicRunRegistry>,
    binding: String,
    id: u64,
    caller_digest: super::public_context::CallerDigest,
    provenance: TelecomProvenance,
    cancel: CancellationToken,
}

impl PublicRunIdentity {
    pub(crate) fn is_current(&self) -> bool {
        if self.cancel.is_cancelled() {
            return false;
        }
        let active = self
            .registry
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        active.get(&self.binding).is_some_and(|entry| {
            entry.id == self.id
                && entry.caller_digest == self.caller_digest
                && !entry.cancel.is_cancelled()
        })
    }

    pub(crate) fn same_run(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.registry, &other.registry)
            && self.binding == other.binding
            && self.id == other.id
            && self.caller_digest == other.caller_digest
            && self.provenance == other.provenance
    }

    pub(crate) fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

impl PublicRunPermit {
    /// Seal unknown cost before this permit releases the binding reservation.
    pub(crate) fn exhaust_unaccounted_spend(&self) {
        self.registry.exhaust_unaccounted_spend();
    }

    /// Cancellation token of this run; fired by
    /// [`PublicRunRegistry::cancel_binding`].
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Validate exact registry allocation, full authenticated provenance and
    /// current Entry id/digest, not just equal metadata or an uncancelled token.
    pub(crate) fn validated_identity(
        &self,
        registry: &Arc<PublicRunRegistry>,
        ctx: &TelecomPublicContext,
    ) -> Option<PublicRunIdentity> {
        if !Arc::ptr_eq(&self.registry, registry)
            || self.binding != ctx.provenance().binding_ref()
            || self.caller_digest != *ctx.provenance().caller_digest()
            || self.provenance != *ctx.provenance()
        {
            return None;
        }
        let identity = PublicRunIdentity {
            registry: self.registry.clone(),
            binding: self.binding.clone(),
            id: self.id,
            caller_digest: self.caller_digest.clone(),
            provenance: self.provenance.clone(),
            cancel: self.cancel.clone(),
        };
        identity.is_current().then_some(identity)
    }
}

impl Drop for PublicRunPermit {
    fn drop(&mut self) {
        let mut active = self
            .registry
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if active.get(&self.binding).is_some_and(|e| e.id == self.id) {
            active.remove(&self.binding);
        }
    }
}

impl PublicRunRegistry {
    fn with_gate(global_cap: usize, token_cap: u64, gate: SpendGate) -> Arc<Self> {
        Arc::new(Self {
            active: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            global_cap,
            token_cap,
            tokens_spent: AtomicU64::new(0),
            gate,
        })
    }

    /// Registry that uses only the bounded token fallback and never reads the
    /// process spend ledger/policy. For unit tests and embedders without a
    /// ledger; production admission is [`Self::global`].
    pub fn new(global_cap: usize, token_cap: u64) -> Arc<Self> {
        Self::with_gate(global_cap, token_cap, SpendGate::FallbackOnly)
    }

    /// Process-wide registry with default caps and formal ledger admission
    /// whenever `[policies.spend]` is enabled.
    pub fn global() -> &'static Arc<Self> {
        static GLOBAL: OnceLock<Arc<PublicRunRegistry>> = OnceLock::new();
        GLOBAL.get_or_init(|| {
            Self::with_gate(
                DEFAULT_PUBLIC_GLOBAL_CAP,
                DEFAULT_PUBLIC_TOKEN_CAP,
                SpendGate::Process,
            )
        })
    }

    /// Whether the public principal is over its spend cap right now: the
    /// formal ledger verdict when a policy is enabled, else the bounded
    /// token fallback. Never touches owner metadata or any owner principal.
    fn over_spend(&self) -> bool {
        match self.gate.formal() {
            Some((policy, ledger)) => matches!(
                crate::spend::check_with(
                    &PUBLIC_PRINCIPAL,
                    chrono::Utc::now().timestamp_millis(),
                    &policy,
                    &*ledger,
                ),
                Verdict::Denied { .. }
            ),
            None => self.tokens_spent.load(Ordering::Acquire) >= self.token_cap,
        }
    }

    /// Recheck the same public spend gate between utterances while a voice
    /// call retains its single permit. Does not acquire another admission slot
    /// or introduce a second accounting policy.
    pub(crate) fn recheck_spend(&self) -> bool {
        !self.over_spend()
    }

    /// Admit one public run for `ctx`'s binding, or refuse with a reason.
    /// Never queues.
    pub fn try_admit(self: &Arc<Self>, ctx: &TelecomPublicContext) -> PublicAdmission {
        if self.over_spend() {
            return PublicAdmission::OverSpend;
        }
        let binding = ctx.provenance().binding_ref().to_string();
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        // The fast check may predate an unknown-cost seal while this caller
        // waits for the reservation lock. Recheck at allocation time.
        if self.over_spend() {
            return PublicAdmission::OverSpend;
        }
        if active.contains_key(&binding) {
            return PublicAdmission::Busy;
        }
        if active.len() >= self.global_cap {
            return PublicAdmission::OverGlobalCap;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = CancellationToken::new();
        active.insert(
            binding.clone(),
            Entry {
                id,
                caller_digest: ctx.provenance().caller_digest().clone(),
                cancel: cancel.clone(),
            },
        );
        PublicAdmission::Admitted(PublicRunPermit {
            registry: Arc::clone(self),
            binding,
            id,
            caller_digest: ctx.provenance().caller_digest().clone(),
            provenance: ctx.provenance().clone(),
            cancel,
        })
    }

    /// Caller-originated `/stop`: only the admitted caller can cancel the
    /// active run of this binding. A shared service number is not authority.
    pub fn cancel_caller(&self, ctx: &TelecomPublicContext) -> bool {
        let active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        match active.get(ctx.provenance().binding_ref()) {
            Some(e) if e.caller_digest == *ctx.provenance().caller_digest() => {
                e.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    /// Trusted control-plane off-switch: cancel the live public run of a
    /// binding without a caller context. Do not call this from inbound SMS.
    pub fn cancel_binding(&self, binding_ref: &str) -> bool {
        let active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        match active.get(binding_ref) {
            Some(e) => {
                e.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn active_count(&self) -> usize {
        self.active.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Count `tokens` toward the bounded fallback cap. A no-op while formal
    /// ledger admission applies: the metered provider already recorded the
    /// run's real cost under [`Principal::TelecomPublic`], and counting it
    /// again here would be a second, disagreeing ledger.
    pub fn record_spend(&self, tokens: u64) {
        if self.gate.formal().is_none() {
            self.tokens_spent.fetch_add(tokens, Ordering::AcqRel);
        }
    }

    /// A failed provider run can consume tokens without returning trustworthy
    /// usage. With no formal ledger, exhaust the fallback rather than treating
    /// unknown cost as zero; the next run must use a configured spend policy or
    /// wait for restart. Formal metering remains the source of truth when on.
    pub fn exhaust_unaccounted_spend(&self) {
        // Serialize fallback sealing with reservation allocation. An existing
        // permit is retained by its owner until after this lock is released.
        let _active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if self.gate.formal().is_none() {
            self.tokens_spent
                .fetch_max(self.token_cap, Ordering::AcqRel);
        }
    }

    /// Tokens counted by the bounded fallback (stays 0 under formal ledger
    /// admission).
    pub fn tokens_spent(&self) -> u64 {
        self.tokens_spent.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::telecom::public_context::test_support::sample_public_context;

    #[test]
    fn second_run_on_same_binding_is_busy_and_released_on_drop() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let ctx = sample_public_context("bind-a");
        let first = reg.try_admit(&ctx);
        assert!(matches!(first, PublicAdmission::Admitted(_)));
        let second = reg.try_admit(&ctx);
        assert!(matches!(second, PublicAdmission::Busy));
        assert_eq!(second.notice(), Some(PUBLIC_BUSY_NOTICE));
        drop(first);
        assert_eq!(reg.active_count(), 0);
        assert!(matches!(reg.try_admit(&ctx), PublicAdmission::Admitted(_)));
    }

    #[test]
    fn same_run_includes_the_full_provenance_witness() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let ctx = sample_public_context("bind-a");
        let PublicAdmission::Admitted(permit) = reg.try_admit(&ctx) else {
            panic!("admit");
        };
        let identity = permit.validated_identity(&reg, &ctx).unwrap();
        let mut other = identity.clone();
        assert!(identity.same_run(&other));
        // Keep registry/id/binding/digest unchanged to isolate the witness.
        other.provenance = sample_public_context("bind-b").provenance().clone();
        assert!(!identity.same_run(&other));
    }

    #[test]
    fn global_cap_refuses_other_bindings() {
        let reg = PublicRunRegistry::new(1, 1_000);
        let _a = reg.try_admit(&sample_public_context("bind-a"));
        let b = reg.try_admit(&sample_public_context("bind-b"));
        assert!(matches!(b, PublicAdmission::OverGlobalCap));
        assert_eq!(b.notice(), Some(PUBLIC_BUSY_NOTICE));
    }

    #[test]
    fn spend_cap_refuses_with_fixed_notice() {
        let reg = PublicRunRegistry::new(4, 100);
        reg.record_spend(100);
        let a = reg.try_admit(&sample_public_context("bind-a"));
        assert!(matches!(a, PublicAdmission::OverSpend));
        assert_eq!(a.notice(), Some(PUBLIC_OVER_SPEND_NOTICE));
    }

    #[test]
    fn unknown_provider_cost_exhausts_only_the_fallback() {
        let reg = PublicRunRegistry::new(4, 100);
        reg.exhaust_unaccounted_spend();
        assert_eq!(reg.tokens_spent(), 100);
        assert!(matches!(
            reg.try_admit(&sample_public_context("another")),
            PublicAdmission::OverSpend
        ));
        let (formal, _) = registry_with(enabled_policy(Some(1.0), None));
        formal.exhaust_unaccounted_spend();
        assert_eq!(formal.tokens_spent(), 0);
    }

    #[test]
    fn caller_stop_cannot_cancel_another_caller_on_the_same_binding() {
        use super::super::public_context::test_support::sample_public_context_for_peer;
        let reg = PublicRunRegistry::new(4, 1_000);
        let caller_a = sample_public_context_for_peer("bind-a", "+15550000001");
        let caller_b = sample_public_context_for_peer("bind-a", "+15550000002");
        let PublicAdmission::Admitted(permit) = reg.try_admit(&caller_a) else {
            panic!("admit");
        };
        assert!(!reg.cancel_caller(&caller_b));
        assert!(!permit.cancel_token().is_cancelled());
        assert!(reg.cancel_caller(&caller_a));
        assert!(permit.cancel_token().is_cancelled());
    }

    #[test]
    fn cancel_binding_fires_the_permit_token() {
        let reg = PublicRunRegistry::new(4, 1_000);
        let PublicAdmission::Admitted(permit) = reg.try_admit(&sample_public_context("bind-a"))
        else {
            panic!("admit");
        };
        assert!(!reg.cancel_binding("bind-other"));
        assert!(reg.cancel_binding("bind-a"));
        assert!(permit.cancel_token().is_cancelled());
    }

    #[test]
    fn admission_rechecks_spend_after_waiting_for_reservation_lock() {
        use std::sync::mpsc;
        struct ReadNotice {
            ledger: crate::spend::InMemorySpendLedger,
            first_read: Mutex<Option<mpsc::Sender<()>>>,
        }
        impl SpendLedger for ReadNotice {
            fn record(&self, p: &Principal, t: i64, d: crate::spend::Delta) -> anyhow::Result<()> {
                self.ledger.record(p, t, d)
            }
            fn spent_for(&self, p: &Principal, t: i64) -> anyhow::Result<crate::spend::Spent> {
                let snapshot = self.ledger.spent_for(p, t)?;
                if let Some(sender) = self.first_read.lock().unwrap().take() {
                    sender.send(()).unwrap();
                }
                Ok(snapshot)
            }
            fn total_for(&self, s: i64, e: i64) -> anyhow::Result<crate::spend::Spent> {
                self.ledger.total_for(s, e)
            }
            fn sweep_before(&self, t: i64) -> anyhow::Result<usize> {
                self.ledger.sweep_before(t)
            }
            fn principals_in(
                &self,
                t: i64,
            ) -> anyhow::Result<Vec<(Principal, crate::spend::Spent)>> {
                self.ledger.principals_in(t)
            }
        }
        let (sender, read) = mpsc::channel();
        let policy = enabled_policy(Some(1.0), None);
        let ledger: Arc<dyn SpendLedger> = Arc::new(ReadNotice {
            ledger: crate::spend::InMemorySpendLedger::default(),
            first_read: Mutex::new(Some(sender)),
        });
        let reg = PublicRunRegistry::with_gate(
            4,
            1000,
            SpendGate::Injected {
                policy: policy.clone(),
                ledger: ledger.clone(),
            },
        );
        let reservation_lock = reg.active.lock().unwrap();
        let pending = reg.clone();
        let thread =
            std::thread::spawn(move || pending.try_admit(&sample_public_context("bind-a")));
        read.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        // The caller already read a zero-spend snapshot, but cannot allocate.
        record_usd(&ledger, &policy, &Principal::TelecomPublic, 1.0);
        drop(reservation_lock);
        assert!(matches!(thread.join().unwrap(), PublicAdmission::OverSpend));
        assert_eq!(reg.active_count(), 0);
    }

    fn enabled_policy(telecom_public_usd: Option<f64>, per_user_usd: Option<f64>) -> SpendPolicy {
        SpendPolicy {
            telecom_public_usd,
            per_user_usd,
            ..SpendPolicy::default()
        }
    }

    fn registry_with(policy: SpendPolicy) -> (Arc<PublicRunRegistry>, Arc<dyn SpendLedger>) {
        let ledger: Arc<dyn SpendLedger> = Arc::new(crate::spend::InMemorySpendLedger::default());
        let reg = PublicRunRegistry::with_gate(
            4,
            u64::MAX,
            SpendGate::Injected {
                policy,
                ledger: Arc::clone(&ledger),
            },
        );
        (reg, ledger)
    }

    fn record_usd(ledger: &Arc<dyn SpendLedger>, policy: &SpendPolicy, who: &Principal, usd: f64) {
        let start = crate::spend::period::period_start_ms(
            chrono::Utc::now().timestamp_millis(),
            policy.period,
        );
        ledger
            .record(who, start, crate::spend::Delta::Usd(usd))
            .expect("record");
    }

    /// Formal admission: the public principal's own ledger row, against
    /// `telecom_public_usd`, refuses with the fixed notice — with no owner
    /// principal anywhere in play (no scope, no metadata).
    #[test]
    fn formal_public_cap_refuses_without_any_owner_principal() {
        let policy = enabled_policy(Some(1.0), Some(1_000.0));
        let (reg, ledger) = registry_with(policy.clone());
        let ctx = sample_public_context("bind-a");
        assert!(matches!(reg.try_admit(&ctx), PublicAdmission::Admitted(_)));

        record_usd(&ledger, &policy, &Principal::TelecomPublic, 1.0);
        let refused = reg.try_admit(&ctx);
        assert!(matches!(refused, PublicAdmission::OverSpend));
        assert_eq!(refused.notice(), Some(PUBLIC_OVER_SPEND_NOTICE));
    }

    /// The public cap reads the public row only: a blown owner/user row does
    /// not refuse public callers, and a blown public row does not move
    /// anyone else's verdict.
    #[test]
    fn formal_public_cap_is_independent_of_owner_rows() {
        let policy = enabled_policy(Some(1.0), Some(1.0));
        let (reg, ledger) = registry_with(policy.clone());
        record_usd(&ledger, &policy, &Principal::User("u-owner".into()), 50.0);
        record_usd(&ledger, &policy, &Principal::Unattributed, 50.0);
        assert!(matches!(
            reg.try_admit(&sample_public_context("bind-a")),
            PublicAdmission::Admitted(_)
        ));
        record_usd(&ledger, &policy, &Principal::TelecomPublic, 2.0);
        let now = chrono::Utc::now().timestamp_millis();
        assert!(matches!(
            crate::spend::check_with(&Principal::TelecomPublic, now, &policy, &*ledger),
            Verdict::Denied { .. }
        ));
        assert!(matches!(
            crate::spend::check_with(&Principal::Unattributed, now, &policy, &*ledger),
            Verdict::Denied { .. }
        ));
    }

    /// With `telecom_public_usd` unset the public line falls back to
    /// `per_user_usd` — public spend is never unconstrained by an enabled
    /// per-principal ceiling.
    #[test]
    fn unset_public_cap_falls_back_to_per_user_ceiling() {
        let policy = enabled_policy(None, Some(1.0));
        let (reg, ledger) = registry_with(policy.clone());
        record_usd(&ledger, &policy, &Principal::TelecomPublic, 1.0);
        assert!(matches!(
            reg.try_admit(&sample_public_context("bind-a")),
            PublicAdmission::OverSpend
        ));
    }

    /// Formal mode: `record_spend` does not feed the fallback counter (the
    /// metered provider owns the ledger write), and the fallback token cap
    /// is not consulted.
    #[test]
    fn formal_mode_does_not_double_count_in_memory() {
        let policy = enabled_policy(Some(100.0), None);
        let ledger: Arc<dyn SpendLedger> = Arc::new(crate::spend::InMemorySpendLedger::default());
        let reg = PublicRunRegistry::with_gate(4, 1, SpendGate::Injected { policy, ledger });
        reg.record_spend(10_000);
        assert_eq!(reg.tokens_spent(), 0);
        assert!(matches!(
            reg.try_admit(&sample_public_context("bind-a")),
            PublicAdmission::Admitted(_)
        ));
    }

    /// Disabled policy: the ledger is never read (a ledger that panics on
    /// any call proves it) and the bounded fallback still refuses.
    #[test]
    fn disabled_policy_uses_bounded_fallback_and_never_touches_ledger() {
        struct PanicLedger;
        impl SpendLedger for PanicLedger {
            fn record(&self, _: &Principal, _: i64, _: crate::spend::Delta) -> anyhow::Result<()> {
                panic!("ledger touched");
            }
            fn spent_for(&self, _: &Principal, _: i64) -> anyhow::Result<crate::spend::Spent> {
                panic!("ledger touched");
            }
            fn total_for(&self, _: i64, _: i64) -> anyhow::Result<crate::spend::Spent> {
                panic!("ledger touched");
            }
            fn sweep_before(&self, _: i64) -> anyhow::Result<usize> {
                panic!("ledger touched");
            }
            fn principals_in(
                &self,
                _: i64,
            ) -> anyhow::Result<Vec<(Principal, crate::spend::Spent)>> {
                panic!("ledger touched");
            }
        }
        let reg = PublicRunRegistry::with_gate(
            4,
            100,
            SpendGate::Injected {
                policy: SpendPolicy::default(),
                ledger: Arc::new(PanicLedger),
            },
        );
        assert!(matches!(
            reg.try_admit(&sample_public_context("bind-a")),
            PublicAdmission::Admitted(_)
        ));
        reg.record_spend(100);
        assert_eq!(reg.tokens_spent(), 100);
        assert!(matches!(
            reg.try_admit(&sample_public_context("bind-b")),
            PublicAdmission::OverSpend
        ));
    }
}
