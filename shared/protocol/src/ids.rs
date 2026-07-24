//! Process-local monotonic id generation.
//!
//! JSON-RPC wire ids and `IdentityContext.request_id` only need to be unique
//! within the process for request/response correlation and audit tagging —
//! neither is a secret. An `AtomicU64` counter serves this without pulling
//! `uuid` (→ `rand`) into the protocol crate (R3).

use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Return a fresh process-unique id, e.g. `"id-42"`. Monotonic; never repeats
/// within a process.
pub fn next_id() -> String {
    format!("id-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_monotonic() {
        let a = next_id();
        let b = next_id();
        assert_ne!(a, b);
        assert!(a.starts_with("id-"));
    }
}
