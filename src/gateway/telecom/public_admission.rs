//! Public telecom run admission (public session isolation contract §5).
//!
//! Public runs never enter the owner busy lane, queue, or durable journal.
//! This registry is their whole admission state, in memory only:
//!
//! - at most one live run per telecom binding (`TelecomProvenance::binding_ref`);
//! - a global cap on concurrent public runs;
//! - an explicit "telecom public" spend principal with its own token cap —
//!   never inferred from metadata, never the owner's budget.
//!
//! Busy / over-cap callers receive a fixed notice; nothing is queued.

use std::collections::HashMap;

use crate::sync_primitives::{Arc, AtomicU64, Mutex, OnceLock, Ordering};
use tokio_util::sync::CancellationToken;

use super::TelecomPublicContext;

/// Explicit spend principal of every public telecom run. Kept distinct from
/// any owner / `Principal::User` id so public spend is never charged to, or
/// admitted against, the owner's budget.
pub const TELECOM_PUBLIC_PRINCIPAL: &str = "@telecom-public";

/// Fixed notice for a busy binding or a full global public lane.
pub const PUBLIC_BUSY_NOTICE: &str = "稍后再试 / Please try again later.";

/// Fixed notice when the public spend principal is over its cap.
pub const PUBLIC_OVER_SPEND_NOTICE: &str =
    "暂时无法回复，请稍后再试 / Unable to reply right now, please try again later.";

/// Default global concurrent public runs.
pub const DEFAULT_PUBLIC_GLOBAL_CAP: usize = 4;
/// Default token cap of the public spend principal per process lifetime.
pub const DEFAULT_PUBLIC_TOKEN_CAP: u64 = 200_000;

struct Entry {
    id: u64,
    cancel: CancellationToken,
}

/// In-memory admission state of public telecom runs.
pub struct PublicRunRegistry {
    active: Mutex<HashMap<String, Entry>>,
    next_id: AtomicU64,
    global_cap: usize,
    token_cap: u64,
    tokens_spent: AtomicU64,
}

/// Outcome of [`PublicRunRegistry::try_admit`].
pub enum PublicAdmission {
    Admitted(PublicRunPermit),
    /// A run for this binding is already live.
    Busy,
    /// The global public lane is full.
    OverGlobalCap,
    /// The telecom-public spend principal is over its cap.
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
    cancel: CancellationToken,
}

impl PublicRunPermit {
    /// Cancellation token of this run; fired by
    /// [`PublicRunRegistry::cancel_binding`].
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
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
    pub fn new(global_cap: usize, token_cap: u64) -> Arc<Self> {
        Arc::new(Self {
            active: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            global_cap,
            token_cap,
            tokens_spent: AtomicU64::new(0),
        })
    }

    /// Process-wide registry with default caps.
    pub fn global() -> &'static Arc<Self> {
        static GLOBAL: OnceLock<Arc<PublicRunRegistry>> = OnceLock::new();
        GLOBAL.get_or_init(|| Self::new(DEFAULT_PUBLIC_GLOBAL_CAP, DEFAULT_PUBLIC_TOKEN_CAP))
    }

    /// Admit one public run for `ctx`'s binding, or refuse with a reason.
    /// Never queues.
    pub fn try_admit(self: &Arc<Self>, ctx: &TelecomPublicContext) -> PublicAdmission {
        if self.tokens_spent.load(Ordering::Acquire) >= self.token_cap {
            return PublicAdmission::OverSpend;
        }
        let binding = ctx.provenance().binding_ref().to_string();
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
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
                cancel: cancel.clone(),
            },
        );
        PublicAdmission::Admitted(PublicRunPermit {
            registry: Arc::clone(self),
            binding,
            id,
            cancel,
        })
    }

    /// `/stop`-equivalent: cancel the live public run of `binding_ref`.
    /// Returns whether a run was found.
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

    /// Charge `tokens` to the telecom-public principal.
    pub fn record_spend(&self, tokens: u64) {
        self.tokens_spent.fetch_add(tokens, Ordering::AcqRel);
    }

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
    fn principal_is_not_an_owner_id() {
        assert!(TELECOM_PUBLIC_PRINCIPAL.starts_with('@'));
    }
}
