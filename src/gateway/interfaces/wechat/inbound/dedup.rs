use std::collections::VecDeque;
use std::time::{Duration, Instant};

const DEDUP_CAPACITY: usize = 1000;
const DEFAULT_TTL_SECS: u64 = 300;

struct DedupEntry {
    message_id: String,
    timestamp: Instant,
}

pub struct MessageDedup {
    seen: VecDeque<DedupEntry>,
    capacity: usize,
    ttl: Duration,
}

impl Default for MessageDedup {
    fn default() -> Self {
        Self {
            seen: VecDeque::with_capacity(DEDUP_CAPACITY),
            capacity: DEDUP_CAPACITY,
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
        }
    }
}

impl MessageDedup {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_ttl(ttl_secs: u64) -> Self {
        Self {
            seen: VecDeque::with_capacity(DEDUP_CAPACITY),
            capacity: DEDUP_CAPACITY,
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    fn evict_expired(&mut self) {
        let now = Instant::now();
        while let Some(entry) = self.seen.front() {
            if now.duration_since(entry.timestamp) > self.ttl {
                self.seen.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn is_duplicate(&mut self, message_id: &str) -> bool {
        self.evict_expired();
        if self.seen.iter().any(|e| e.message_id == message_id) {
            return true;
        }
        if self.seen.len() >= self.capacity {
            self.seen.pop_front();
        }
        self.seen.push_back(DedupEntry {
            message_id: message_id.to_string(),
            timestamp: Instant::now(),
        });
        false
    }

    pub fn remove(&mut self, message_id: &str) {
        self.seen.retain(|entry| entry.message_id != message_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedup_new_message() {
        let mut dedup = MessageDedup::new();
        assert!(!dedup.is_duplicate("msg_001"));
    }

    #[test]
    fn test_dedup_duplicate_message() {
        let mut dedup = MessageDedup::new();
        assert!(!dedup.is_duplicate("msg_001"));
        assert!(dedup.is_duplicate("msg_001"));
    }

    #[test]
    fn test_dedup_capacity_eviction() {
        let mut dedup = MessageDedup {
            seen: VecDeque::new(),
            capacity: 3,
            ttl: Duration::from_secs(300),
        };
        assert!(!dedup.is_duplicate("a"));
        assert!(!dedup.is_duplicate("b"));
        assert!(!dedup.is_duplicate("c"));
        assert!(!dedup.is_duplicate("d"));
        assert!(!dedup.is_duplicate("a"));
    }

    #[test]
    fn test_dedup_ttl_eviction() {
        let mut dedup = MessageDedup::with_ttl(1);
        assert!(!dedup.is_duplicate("msg_001"));
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!dedup.is_duplicate("msg_001"));
    }
}
