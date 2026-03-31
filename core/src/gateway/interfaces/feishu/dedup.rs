use std::collections::VecDeque;

const DEDUP_CAPACITY: usize = 1000;

/// Ring-buffer message deduplication.
pub struct MessageDedup {
    seen: VecDeque<String>,
    capacity: usize,
}

impl MessageDedup {
    pub fn new() -> Self {
        Self {
            seen: VecDeque::with_capacity(DEDUP_CAPACITY),
            capacity: DEDUP_CAPACITY,
        }
    }

    /// Returns `true` if the message_id is a duplicate (already seen).
    /// If new, records it and returns `false`.
    pub fn is_duplicate(&mut self, message_id: &str) -> bool {
        if self.seen.iter().any(|id| id == message_id) {
            return true;
        }
        if self.seen.len() >= self.capacity {
            self.seen.pop_front();
        }
        self.seen.push_back(message_id.to_string());
        false
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
        };
        assert!(!dedup.is_duplicate("a"));
        assert!(!dedup.is_duplicate("b"));
        assert!(!dedup.is_duplicate("c"));
        // "a" should be evicted after this
        assert!(!dedup.is_duplicate("d"));
        assert!(!dedup.is_duplicate("a")); // no longer seen
    }
}
