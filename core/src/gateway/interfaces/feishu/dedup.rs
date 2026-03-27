use std::collections::VecDeque;
use std::time::{Duration, Instant};

const DEFAULT_CAPACITY: usize = 5000;
const DEFAULT_TTL: Duration = Duration::from_secs(86400); // 24 hours

pub struct MessageDedup {
    seen: VecDeque<(String, Instant)>,
    capacity: usize,
    ttl: Duration,
}

impl MessageDedup {
    pub fn new() -> Self {
        Self {
            seen: VecDeque::with_capacity(DEFAULT_CAPACITY),
            capacity: DEFAULT_CAPACITY,
            ttl: DEFAULT_TTL,
        }
    }

    /// Returns true if the message_id was already seen (duplicate).
    pub fn is_duplicate(&mut self, message_id: &str) -> bool {
        let now = Instant::now();

        // Drain expired entries from front
        while let Some((_, ts)) = self.seen.front() {
            if now.duration_since(*ts) > self.ttl {
                self.seen.pop_front();
            } else {
                break;
            }
        }

        // Check for existing
        if self.seen.iter().any(|(id, _)| id == message_id) {
            return true;
        }

        // Evict oldest if at capacity
        if self.seen.len() >= self.capacity {
            self.seen.pop_front();
        }

        self.seen.push_back((message_id.to_string(), now));
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_duplicate() {
        let mut dedup = MessageDedup::new();
        assert!(!dedup.is_duplicate("msg1"));
    }

    #[test]
    fn test_is_duplicate() {
        let mut dedup = MessageDedup::new();
        assert!(!dedup.is_duplicate("msg1"));
        assert!(dedup.is_duplicate("msg1"));
    }

    #[test]
    fn test_different_ids_not_duplicate() {
        let mut dedup = MessageDedup::new();
        assert!(!dedup.is_duplicate("msg1"));
        assert!(!dedup.is_duplicate("msg2"));
    }

    #[test]
    fn test_capacity_eviction() {
        let mut dedup = MessageDedup {
            seen: VecDeque::new(),
            capacity: 3,
            ttl: DEFAULT_TTL,
        };
        assert!(!dedup.is_duplicate("a"));
        assert!(!dedup.is_duplicate("b"));
        assert!(!dedup.is_duplicate("c"));
        assert!(!dedup.is_duplicate("d")); // "a" evicted
        assert!(!dedup.is_duplicate("a")); // "a" is new again
    }
}
