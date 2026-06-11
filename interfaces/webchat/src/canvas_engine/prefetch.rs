use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

pub const HOVER_DEBOUNCE_MS: f64 = 150.0;
pub const CACHE_TTL_MS: f64 = 90_000.0;
pub const CACHE_CAPACITY: usize = 20;
/// Upper bound on concurrent `graph.neighbors` prefetch RPCs.
/// Hover-skimming 50 nodes must not spawn 50 inflight WebSocket calls.
pub const MAX_IN_FLIGHT: usize = 4;

/// Bounded LRU cache of payloads keyed by `String` id, with TTL.
/// Each entry carries its own fetched-at timestamp because the raw payload has no
/// such field (unlike `Neighborhood`).
pub struct PrefetchCache<T> {
    entries: VecDeque<(String, T, f64)>,
    capacity: usize,
    ttl_ms: f64,
}

impl<T> PrefetchCache<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: CACHE_CAPACITY,
            ttl_ms: CACHE_TTL_MS,
        }
    }

    pub fn put(&mut self, id: String, value: T, now_ms: f64) {
        self.entries.retain(|(k, _, _)| k != &id);
        self.entries.push_back((id, value, now_ms));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    #[must_use]
    pub fn get(&self, id: &str, now_ms: f64) -> Option<&T> {
        self.entries.iter().rev().find_map(|(k, v, fetched)| {
            if k == id && now_ms - fetched <= self.ttl_ms {
                Some(v)
            } else {
                None
            }
        })
    }

    #[must_use]
    pub fn has(&self, id: &str, now_ms: f64) -> bool {
        self.get(id, now_ms).is_some()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop the cache entry for `id`, if present. Used by the event-driven
    /// invalidation path (memory.note.changed / NoteDeleted) so the next
    /// navigation refetches instead of serving a stale neighborhood.
    pub fn invalidate(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(k, _, _)| k != id);
        before != self.entries.len()
    }

    /// Drop every cached entry. Cheaper than recreating the struct when the
    /// caller wants to keep the same capacity / TTL config.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl<T> Default for PrefetchCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Hover-debounce timer state. Caller calls `note_hover` on each pointer move.
pub struct HoverDebouncer {
    current_id: Option<String>,
    started_at_ms: f64,
}

impl Default for HoverDebouncer {
    fn default() -> Self {
        Self::new()
    }
}

impl HoverDebouncer {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            current_id: None,
            started_at_ms: 0.0,
        }
    }

    /// Returns Some(id) if hover threshold reached, else None.
    pub fn note_hover(&mut self, hovered: Option<&str>, now_ms: f64) -> Option<String> {
        match (hovered, &self.current_id) {
            (Some(id), Some(cur)) if id == cur => {
                if now_ms - self.started_at_ms >= HOVER_DEBOUNCE_MS {
                    let out = self.current_id.take();
                    self.started_at_ms = now_ms; // prevent immediate refire
                    return out;
                }
                None
            }
            (Some(id), _) => {
                self.current_id = Some(id.to_string());
                self.started_at_ms = now_ms;
                None
            }
            (None, _) => {
                self.current_id = None;
                None
            }
        }
    }
}

/// Bounded set of in-flight prefetch ids.
///
/// Two responsibilities, both load-bearing:
/// 1. **De-dup** — if a hover is still loading, a second hover on the same node
///    must not spawn a parallel RPC.
/// 2. **Concurrency cap** — at most `cap` (default `MAX_IN_FLIGHT = 4`) RPCs
///    may be outstanding at once. A skim across 50 nodes is rate-limited so
///    later hovers wait for earlier ones to settle instead of fanning out.
///
/// Lives in panel-land (single-threaded WASM); no synchronization needed beyond
/// the surrounding `RefCell`. Release happens via the RAII guard, so panics
/// inside the future still drop the slot.
pub struct InFlightSet {
    ids: HashSet<String>,
    cap: usize,
}

impl InFlightSet {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            ids: HashSet::new(),
            cap,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }
}

/// RAII handle to a slot inside `InFlightSet`. The slot is freed when the
/// guard is dropped — whether the spawned future completes, errors, or panics.
pub struct InFlightGuard {
    set: Rc<RefCell<InFlightSet>>,
    id: String,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.set.borrow_mut().ids.remove(&self.id);
    }
}

/// Try to register `id` as in-flight. Returns `None` if the cap is full or the
/// id is already being fetched (de-dup). The caller owns the guard for the
/// lifetime of the async work.
pub fn try_acquire_in_flight(set: &Rc<RefCell<InFlightSet>>, id: &str) -> Option<InFlightGuard> {
    let mut s = set.borrow_mut();
    if s.ids.contains(id) || s.ids.len() >= s.cap {
        return None;
    }
    s.ids.insert(id.to_string());
    drop(s);
    Some(InFlightGuard {
        set: Rc::clone(set),
        id: id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_engine::adapter::{GraphNeighborsResponse, NoteNodeDto};
    use std::collections::HashMap;

    fn raw_resp(id: &str) -> GraphNeighborsResponse {
        GraphNeighborsResponse {
            center: NoteNodeDto {
                id: id.to_string(),
                name: id.to_string(),
                path: format!("{id}.md"),
                category: "concept".to_string(),
                tags: vec![],
                link_count: 1,
            },
            nodes: vec![],
            edges: vec![],
            hop_depth: HashMap::new(),
        }
    }

    #[test]
    fn cache_put_then_get() {
        let mut c = PrefetchCache::new();
        c.put("a".to_string(), raw_resp("a"), 0.0);
        assert!(c.get("a", 100.0).is_some());
    }

    #[test]
    fn cache_expires_after_ttl() {
        let mut c = PrefetchCache::new();
        c.put("a".to_string(), raw_resp("a"), 0.0);
        assert!(c.get("a", CACHE_TTL_MS + 1.0).is_none());
    }

    #[test]
    fn cache_evicts_oldest_at_capacity() {
        let mut c = PrefetchCache::new();
        for i in 0..(CACHE_CAPACITY + 5) {
            c.put(format!("n{i}"), raw_resp(&format!("n{i}")), 0.0);
        }
        assert_eq!(c.len(), CACHE_CAPACITY);
        assert!(c.get("n0", 0.0).is_none());
        assert!(c.get(&format!("n{}", CACHE_CAPACITY + 4), 0.0).is_some());
    }

    #[test]
    fn cache_serves_any_threshold_for_same_id() {
        // The cache no longer discriminates by fold_threshold; a single put serves
        // any caller-side threshold via `to_neighborhood(raw, _, threshold)`.
        let mut c = PrefetchCache::new();
        c.put("a".to_string(), raw_resp("a"), 0.0);
        let v = c.get("a", 100.0).expect("present");
        assert_eq!(v.center.id, "a");
    }

    #[test]
    fn debounce_fires_after_threshold() {
        let mut d = HoverDebouncer::new();
        assert_eq!(d.note_hover(Some("x"), 0.0), None);
        assert_eq!(d.note_hover(Some("x"), 100.0), None);
        assert_eq!(d.note_hover(Some("x"), 151.0), Some("x".to_string()));
    }

    #[test]
    fn debounce_resets_on_target_change() {
        let mut d = HoverDebouncer::new();
        d.note_hover(Some("x"), 0.0);
        assert_eq!(d.note_hover(Some("y"), 100.0), None);
        assert_eq!(d.note_hover(Some("y"), 251.0), Some("y".to_string()));
    }

    #[test]
    fn cache_works_for_string_payloads() {
        let mut c: PrefetchCache<String> = PrefetchCache::new();
        c.put("a".into(), "hello".into(), 0.0);
        assert_eq!(c.get("a", 100.0).map(String::as_str), Some("hello"));
        assert!(c.has("a", 100.0));
    }

    #[test]
    fn cache_expires_by_ttl() {
        let mut c: PrefetchCache<u32> = PrefetchCache::new();
        c.put("k".into(), 42, 0.0);
        assert_eq!(c.get("k", CACHE_TTL_MS - 1.0), Some(&42));
        assert_eq!(c.get("k", CACHE_TTL_MS + 1.0), None);
    }

    #[test]
    fn ttl_is_ninety_seconds() {
        // Guards against accidental regression of the goal-mandated 90s window.
        assert_eq!(CACHE_TTL_MS, 90_000.0);
    }

    #[test]
    fn invalidate_drops_entry_and_returns_true() {
        let mut c: PrefetchCache<u32> = PrefetchCache::new();
        c.put("k".into(), 1, 0.0);
        assert!(c.has("k", 0.0));
        assert!(c.invalidate("k"));
        assert!(!c.has("k", 0.0));
    }

    #[test]
    fn invalidate_missing_id_returns_false() {
        let mut c: PrefetchCache<u32> = PrefetchCache::new();
        assert!(!c.invalidate("ghost"));
    }

    #[test]
    fn clear_drops_all_entries() {
        let mut c: PrefetchCache<u32> = PrefetchCache::new();
        c.put("a".into(), 1, 0.0);
        c.put("b".into(), 2, 0.0);
        assert_eq!(c.len(), 2);
        c.clear();
        assert_eq!(c.len(), 0);
    }

    // --- InFlightSet ---------------------------------------------------------

    #[test]
    fn in_flight_acquire_releases_on_drop() {
        let set = Rc::new(RefCell::new(InFlightSet::new(4)));
        {
            let g = try_acquire_in_flight(&set, "a").expect("first acquire");
            assert_eq!(set.borrow().len(), 1);
            assert!(set.borrow().contains("a"));
            drop(g);
        }
        assert_eq!(set.borrow().len(), 0);
        assert!(!set.borrow().contains("a"));
    }

    #[test]
    fn in_flight_dedup_same_id() {
        let set = Rc::new(RefCell::new(InFlightSet::new(4)));
        let _g = try_acquire_in_flight(&set, "a").expect("first");
        assert!(
            try_acquire_in_flight(&set, "a").is_none(),
            "second acquire of same id must dedup"
        );
        assert_eq!(set.borrow().len(), 1);
    }

    #[test]
    fn in_flight_caps_concurrency() {
        let set = Rc::new(RefCell::new(InFlightSet::new(MAX_IN_FLIGHT)));
        let mut guards = Vec::new();
        for i in 0..MAX_IN_FLIGHT {
            guards.push(
                try_acquire_in_flight(&set, &format!("n{i}"))
                    .unwrap_or_else(|| panic!("slot {i} should be free")),
            );
        }
        assert_eq!(set.borrow().len(), MAX_IN_FLIGHT);
        assert!(
            try_acquire_in_flight(&set, "overflow").is_none(),
            "acquire past cap must fail"
        );
        // Releasing one slot makes room.
        drop(guards.pop());
        assert!(try_acquire_in_flight(&set, "overflow").is_some());
    }

    #[test]
    fn in_flight_fifty_hover_storm_caps_at_four() {
        // Simulates the acceptance criterion: skimming 50 nodes must not exceed
        // MAX_IN_FLIGHT concurrent fetches. We don't release any guard so the
        // set stays saturated.
        let set = Rc::new(RefCell::new(InFlightSet::new(MAX_IN_FLIGHT)));
        let mut held = Vec::new();
        let mut admitted = 0usize;
        for i in 0..50 {
            if let Some(g) = try_acquire_in_flight(&set, &format!("n{i}")) {
                held.push(g);
                admitted += 1;
            }
        }
        assert_eq!(admitted, MAX_IN_FLIGHT);
        assert_eq!(set.borrow().len(), MAX_IN_FLIGHT);
    }

    #[test]
    fn debounce_rapid_target_switch_no_panic() {
        // Acceptance: rapidly hovering 50 distinct nodes must not panic. The
        // debouncer resets state on every target change and only fires after
        // the dwell threshold, so 50 distinct hovers within the threshold
        // window produce zero PrefetchNeighbor intents.
        let mut d = HoverDebouncer::new();
        let mut fired = 0usize;
        for i in 0..50 {
            let id = format!("n{i}");
            if d.note_hover(Some(&id), i as f64).is_some() {
                fired += 1;
            }
        }
        assert_eq!(
            fired, 0,
            "rapid distinct hovers should not fire any prefetch"
        );
    }
}
