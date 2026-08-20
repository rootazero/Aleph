//! The critical section around one side session's read-copy-write.
//!
//! # What this covers that the store does not
//!
//! Seeding is three awaits — read the cursor, copy the delta, advance the
//! cursor — and nothing underneath makes them one step. `patch_session`'s
//! critical section is real but it covers the metadata **document**: it
//! guarantees no other writer clobbers the cursor key while it is being
//! rewritten. It says nothing about two seeders interleaving *around* it.
//!
//! Without this lock, two side questions on the same main session — the same
//! person asking from Panel and from Telegram, or a channel retrying a
//! delivery — both read cursor `C`, both copy `[C+1 ..= D]`, and both write
//! `D`. The delta lands in the side transcript twice, the resulting state
//! looks perfectly healthy, and nothing ever repeats or self-heals. That is
//! the doubling the seeding module exists to prevent, arriving through the one
//! path a cursor cannot see, with zero errors.
//!
//! # Why a process-local lock is a complete fix and not a partial one
//!
//! The daemon is a singleton: `utils::instance_lock` holds an exclusive
//! `flock` on `<data_dir>/aleph.lock`, and CLI write subcommands go through
//! IPC rather than opening the store themselves. A second process writing
//! these rows is already a diagnosed fault (`doctor`'s
//! `core/duplicate-instance`), not a case to be locked against here. Same
//! reasoning and same shape as
//! [`crate::gateway::session_store::file_backend`]'s `MetaLocks`, which
//! answers the identical question one layer down.
//!
//! # Why the permit is a type
//!
//! [`SeedPermit`]'s field is private to this module and [`acquire`] is its
//! only constructor, so a second entry point into seeding cannot claim to hold
//! the lock without taking it. The behavioural guard is
//! `concurrent_side_questions_do_not_double_the_delta`, which goes red if the
//! acquisition is removed — a source-level rule would only recognise the
//! shapes it had been taught.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};

use crate::session::service::SessionId;

/// Prune dead slots once the table reaches this size.
///
/// A slot whose `Arc` is gone has no live critical section, so dropping it
/// cannot split a lock in two. The bound only exists so a long-lived server
/// that serves many short-lived side threads does not accumulate one `Weak`
/// per key forever.
const PRUNE_AT: usize = 128;

/// Per-side-session seeding locks.
#[derive(Debug, Default)]
struct SeedGate {
    slots: std::sync::Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
}

impl SeedGate {
    /// Resolve the lock for `key`, creating it if no live holder exists.
    ///
    /// Upgrade-or-insert happens under the table's own mutex, so two tasks
    /// racing on the same key always end up with the same `Arc` — the check
    /// and the insert cannot interleave.
    fn slot(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(live) = slots.get(key).and_then(Weak::upgrade) {
            return live;
        }
        if slots.len() >= PRUNE_AT {
            slots.retain(|_, w| w.strong_count() > 0);
        }
        let fresh = Arc::new(tokio::sync::Mutex::new(()));
        slots.insert(key.to_string(), Arc::downgrade(&fresh));
        fresh
    }
}

fn gate() -> &'static SeedGate {
    static GATE: OnceLock<SeedGate> = OnceLock::new();
    GATE.get_or_init(SeedGate::default)
}

/// Exclusive right to seed one side session, held until dropped.
///
/// Constructible only by [`acquire`] — the field is private to this module —
/// so "I hold the seeding lock" cannot be asserted without holding it.
#[derive(Debug)]
pub(crate) struct SeedPermit {
    _inner: tokio::sync::OwnedMutexGuard<()>,
}

/// Take the seeding lock for `side`.
///
/// Keyed on the side session, not the main one: two different main sessions
/// derive different side keys and never touch the same transcript, so locking
/// them against each other would serialise unrelated work.
pub(crate) async fn acquire(side: &SessionId) -> SeedPermit {
    SeedPermit {
        _inner: gate().slot(&side.to_key_string()).lock_owned().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> SessionId {
        SessionId::Ephemeral {
            agent_id: "gate-test".to_string(),
            ephemeral_id: id.to_string(),
        }
    }

    /// The same key resolves to the same lock while a holder is alive — the
    /// property the whole module rests on. If `slot` handed out two different
    /// mutexes for one key, every guarantee above would be decoration.
    #[test]
    fn one_key_is_one_lock_while_a_holder_is_alive() {
        let g = SeedGate::default();
        let a = g.slot("agent:main:ephemeral:btw-1");
        let b = g.slot("agent:main:ephemeral:btw-1");
        assert!(Arc::ptr_eq(&a, &b));
        let c = g.slot("agent:main:ephemeral:btw-2");
        assert!(!Arc::ptr_eq(&a, &c));
    }

    /// Different side sessions do not serialise against each other.
    #[tokio::test]
    async fn distinct_side_sessions_do_not_block_one_another() {
        let held = acquire(&key("btw-a")).await;
        // Would deadlock if the gate were global rather than keyed.
        let other = acquire(&key("btw-b")).await;
        drop(other);
        drop(held);
    }

    /// The slot table does not grow without bound across many short-lived
    /// side threads.
    #[test]
    fn dead_slots_are_reclaimed() {
        let g = SeedGate::default();
        for i in 0..(PRUNE_AT * 3) {
            drop(g.slot(&format!("agent:main:ephemeral:btw-{i}")));
        }
        let count = g.slots.lock().unwrap_or_else(|e| e.into_inner()).len();
        assert!(count <= PRUNE_AT, "slot table kept {count} entries");
    }
}
