use crate::canvas_engine::types::Neighborhood;
use std::collections::VecDeque;

pub const HOVER_DEBOUNCE_MS: f64 = 150.0;
pub const CACHE_TTL_MS: f64 = 60_000.0;
pub const CACHE_CAPACITY: usize = 20;

pub struct PrefetchCache {
    entries: VecDeque<((String, usize), Neighborhood)>,
    capacity: usize,
    ttl_ms: f64,
}

impl PrefetchCache {
    pub fn new() -> Self {
        Self { entries: VecDeque::new(), capacity: CACHE_CAPACITY, ttl_ms: CACHE_TTL_MS }
    }

    pub fn put(&mut self, id: String, threshold: usize, nbhd: Neighborhood) {
        let key = (id, threshold);
        self.entries.retain(|(k, _)| k != &key);
        self.entries.push_back((key, nbhd));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub fn get(&self, id: &str, threshold: usize, now_ms: f64) -> Option<&Neighborhood> {
        self.entries.iter().rev().find_map(|((k_id, k_thresh), v)| {
            if k_id == id
                && *k_thresh == threshold
                && now_ms - v.fetched_at_ms <= self.ttl_ms
            {
                Some(v)
            } else {
                None
            }
        })
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
    use crate::canvas_engine::types::*;
    use std::collections::HashMap;

    fn nbhd(id: &str, fetched_at: f64) -> Neighborhood {
        Neighborhood {
            center: CanvasNode {
                id: id.to_string(),
                name: id.to_string(),
                category: "concept".to_string(),
                color: Color::new(0, 0, 0),
                radius: 30.0,
                position: Vec2::new(0.0, 0.0),
                velocity: Vec2::new(0.0, 0.0),
                z: 0.0,
                hop: 0,
                pinned: false,
                decay_score: 1.0,
                edge_count: 1,
            },
            one_hop: vec![],
            two_hop: vec![],
            orphans: vec![],
            clusters: vec![],
            edges: vec![],
            target_positions: HashMap::new(),
            fetched_at_ms: fetched_at,
        }
    }

    #[test]
    fn cache_put_then_get() {
        let mut c = PrefetchCache::new();
        c.put("a".to_string(), 12, nbhd("a", 0.0));
        assert!(c.get("a", 12, 100.0).is_some());
    }

    #[test]
    fn cache_expires_after_ttl() {
        let mut c = PrefetchCache::new();
        c.put("a".to_string(), 12, nbhd("a", 0.0));
        assert!(c.get("a", 12, CACHE_TTL_MS + 1.0).is_none());
    }

    #[test]
    fn cache_evicts_oldest_at_capacity() {
        let mut c = PrefetchCache::new();
        for i in 0..(CACHE_CAPACITY + 5) {
            c.put(format!("n{i}"), 12, nbhd(&format!("n{i}"), 0.0));
        }
        assert_eq!(c.len(), CACHE_CAPACITY);
        assert!(c.get("n0", 12, 0.0).is_none());
        assert!(c.get(&format!("n{}", CACHE_CAPACITY + 4), 12, 0.0).is_some());
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
    fn cache_miss_when_threshold_differs() {
        let mut c = PrefetchCache::new();
        c.put("a".to_string(), 12, nbhd("a", 0.0));
        assert!(c.get("a", 12, 100.0).is_some(), "same threshold hits");
        assert!(c.get("a", 6, 100.0).is_none(), "different threshold misses");
    }
}
