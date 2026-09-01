//! Bounded insertion-order (FIFO) cache hygiene, shared by the long-lived
//! `String`-keyed indexes several subsystems keep in memory.
//!
//! Lives here rather than in any one of them because its callers sit on both
//! sides of a dependency edge: `gateway::event_visibility` (run→session,
//! session→ownership, team→owner) and `teams::broadcast` (fan-out
//! `run_id → team_id`). It was written once in the gateway and reached into
//! from `teams`, which is the wrong direction (P1: a domain module must not
//! depend on the interface layer for a generic helper). The caps themselves
//! stay with their owners — only the eviction rule is shared.

use std::collections::{HashMap, VecDeque};

/// Insert into a bounded insertion-order cache, evicting the oldest key once
/// `cap` is exceeded.
///
/// `order` and `map` are two halves of one structure and must only ever be
/// mutated together through this function; four hand-copied versions of a
/// `while len > cap` loop is how one of them ends up unbounded after a
/// refactor nobody applied everywhere.
pub(crate) fn remember<V>(
    order: &mut VecDeque<String>,
    map: &mut HashMap<String, V>,
    key: String,
    value: V,
    cap: usize,
) {
    // `cap=0` is technically a "hard bound" — the cache ends up empty — but
    // every insert is immediately evicted and the value is silently lost, so
    // it reads as a working cache that just never holds anything. Catch the
    // programmer error in tests; in release builds, fall through (the
    // existing behavior) so an existing cap=0 call site cannot panic in prod.
    debug_assert!(
        cap > 0,
        "fifo_cache cap must be > 0; cap=0 silently swallows every insert"
    );
    if cap == 0 {
        return;
    }
    if !map.contains_key(&key) {
        order.push_back(key.clone());
    }
    map.insert(key, value);
    while map.len() > cap {
        let Some(oldest) = order.pop_front() else {
            break;
        };
        map.remove(&oldest);
    }
}

/// Drop a single key from the insertion-order cache, keeping `order` and
/// `map` consistent so a future `remember` of the same key starts a fresh
/// insertion slot rather than double-booking the eviction queue. Idempotent:
/// forgetting an absent key is a no-op.
///
/// Used by `gateway::event_visibility::{forget_session, forget_team}` to
/// invalidate cached ownership / scope pairs after the source row mutates
/// — without it the cache keeps serving pre-mutation data until FIFO
/// eviction, which is exactly the bug the forget arm closes.
pub(crate) fn forget<V>(order: &mut VecDeque<String>, map: &mut HashMap<String, V>, key: &str) {
    if map.remove(key).is_some() {
        // The same key may have been re-inserted multiple times while live
        // (`remember` deduplicates), so only ONE slot in the queue belongs
        // to it. Remove the first match and leave any later duplicates
        // alone — they are unreachable from `map.remove` and harmless.
        if let Some(pos) = order.iter().position(|k| k == key) {
            order.remove(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_the_oldest_key_once_the_cap_is_exceeded() {
        let mut order = VecDeque::new();
        let mut map = HashMap::new();
        for i in 0..5 {
            remember(&mut order, &mut map, format!("k{i}"), i, 3);
        }
        assert_eq!(map.len(), 3, "the cap is a hard bound");
        assert!(!map.contains_key("k0"), "the oldest key is evicted first");
        assert!(!map.contains_key("k1"));
        assert_eq!(map.get("k4"), Some(&4), "the newest key survives");
    }

    #[test]
    fn re_inserting_a_live_key_does_not_double_book_its_eviction_slot() {
        let mut order = VecDeque::new();
        let mut map = HashMap::new();
        remember(&mut order, &mut map, "a".to_string(), 1, 2);
        remember(&mut order, &mut map, "a".to_string(), 2, 2);
        assert_eq!(order.len(), 1, "one live key, one eviction slot");
        assert_eq!(map.get("a"), Some(&2), "the value is overwritten");
    }
}
