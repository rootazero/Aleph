//! GUID dedup for the webhook∩catch-up-poll overlap. Same algorithm as
//! feishu_inbound::dedup (intentional copy — channels stay decoupled).
use std::collections::VecDeque;
use std::time::{Duration, Instant};

struct Entry {
    id: String,
    at: Instant,
}

pub struct BbDedup {
    seen: VecDeque<Entry>,
    cap: usize,
    ttl: Duration,
}

impl Default for BbDedup {
    fn default() -> Self {
        Self::new()
    }
}

impl BbDedup {
    #[must_use]
    pub fn new() -> Self {
        Self {
            seen: VecDeque::with_capacity(1000),
            cap: 1000,
            ttl: Duration::from_secs(600),
        }
    }

    fn evict(&mut self) {
        let now = Instant::now();
        while let Some(e) = self.seen.front() {
            if now.duration_since(e.at) > self.ttl {
                self.seen.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn is_duplicate(&mut self, id: &str) -> bool {
        self.evict();
        if self.seen.iter().any(|e| e.id == id) {
            return true;
        }
        if self.seen.len() >= self.cap {
            self.seen.pop_front();
        }
        self.seen.push_back(Entry { id: id.to_string(), at: Instant::now() });
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_blocks_second_sight() {
        let mut d = BbDedup::new();
        assert!(!d.is_duplicate("g1"));
        assert!(d.is_duplicate("g1"));
    }
}
