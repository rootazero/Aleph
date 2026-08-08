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
