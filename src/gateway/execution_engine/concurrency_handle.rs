//! Process-global handle to the live `ConcurrencyLimiter`, mirroring
//! `providers::route_handle`. The limiter is built once inside the
//! `ExecutionEngine`; a `[execution]` config patch alone never reaches it.
//! This handle lets `self_config` hot-apply new run caps on the next
//! admission — no daemon restart (the task's hot-state requirement).
//!
//! Holds a `Weak` so a torn-down engine doesn't keep the limiter alive;
//! `reconfigure_global` is a no-op returning `false` when nothing is live.

use std::sync::{OnceLock, Weak};

use super::concurrency::ConcurrencyLimiter;
use crate::sync_primitives::Arc;

static HANDLE: OnceLock<Weak<ConcurrencyLimiter>> = OnceLock::new();

/// Register the engine's limiter once (idempotent). Called from
/// `ExecutionEngine::new`.
pub(super) fn install_global(limiter: &Arc<ConcurrencyLimiter>) {
    let _ = HANDLE.set(Arc::downgrade(limiter));
}

/// Live-resize the global run caps. Returns `false` if no engine is installed
/// or it has been dropped (the caller reports "no live limiter").
pub fn reconfigure_global(global_cap: usize, per_agent_cap: usize) -> bool {
    match HANDLE.get().and_then(Weak::upgrade) {
        Some(limiter) => {
            limiter.reconfigure(global_cap, per_agent_cap);
            true
        }
        None => false,
    }
}
