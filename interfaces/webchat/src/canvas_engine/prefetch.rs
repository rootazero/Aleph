use crate::canvas_engine::adapter::GraphNeighborsResponse;
use std::collections::VecDeque;

pub const HOVER_DEBOUNCE_MS: f64 = 150.0;
pub const CACHE_TTL_MS: f64 = 60_000.0;
pub const CACHE_CAPACITY: usize = 20;

/// Bounded LRU cache of raw `GraphNeighborsResponse` payloads, keyed by center id.
/// Each entry carries its own fetched-at timestamp because the raw payload has no
/// such field (unlike `Neighborhood`).
pub struct PrefetchCache {
    entries: VecDeque<(String, GraphNeighborsResponse, f64)>,
    capacity: usize,
    ttl_ms: f64,
}

impl PrefetchCache {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: CACHE_CAPACITY,
            ttl_ms: CACHE_TTL_MS,
        }
    }

    pub fn put(&mut self, id: String, raw: GraphNeighborsResponse, now_ms: f64) {
        self.entries.retain(|(k, _, _)| k != &id);
        self.entries.push_back((id, raw, now_ms));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub fn get(&self, id: &str, now_ms: f64) -> Option<&GraphNeighborsResponse> {
        self.entries.iter().rev().find_map(|(k, v, fetched)| {
            if k == id && now_ms - fetched <= self.ttl_ms {
                Some(v)
            } else {
                None
            }
        })
    }

    pub fn has(&self, id: &str, now_ms: f64) -> bool {
        self.get(id, now_ms).is_some()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Hover-debounce timer state. Caller calls `note_hover` on each pointer move.
pub struct HoverDebouncer {
    current_id: Option<String>,
    started_at_ms: f64,
}

impl HoverDebouncer {
    pub fn new() -> Self {
        Self { current_id: None, started_at_ms: 0.0 }
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
}
