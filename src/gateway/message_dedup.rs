//! Message deduplication with text normalization

use std::collections::HashMap;
use std::time::Instant;

/// Normalize text for duplicate comparison
///
/// - Trims whitespace
/// - Collapses multiple spaces
/// - Converts to lowercase
/// - Removes common punctuation
pub fn normalize_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .replace(['。', '，', '！', '？', '；', '：'], "")
        .replace(['.', ',', '!', '?', ';', ':'], "")
}

/// Check if two texts are duplicates after normalization
pub fn is_text_duplicate(a: &str, b: &str) -> bool {
    normalize_text(a) == normalize_text(b)
}

/// Record of a sent message
#[derive(Debug, Clone)]
pub struct SentRecord {
    pub channel: String,
    pub user_id: Option<String>,
    pub text: String,
    pub sent_at: Instant,
}

const DEFAULT_MAX_TRACKED: usize = 10_000;

/// Tracks sent messages for deduplication
#[derive(Debug)]
pub struct SentMessageTracker {
    /// Original sent texts
    sent_texts: Vec<String>,
    /// Normalized texts for fast lookup, refcounted because distinct raw texts
    /// can map to the same normalized form (normalization is many-to-one).
    sent_normalized: HashMap<String, usize>,
    /// Full records with metadata
    records: Vec<SentRecord>,
    /// Maximum number of messages to track (oldest are evicted)
    max_tracked: usize,
}

impl Default for SentMessageTracker {
    fn default() -> Self {
        Self {
            sent_texts: Vec::new(),
            sent_normalized: HashMap::new(),
            records: Vec::new(),
            max_tracked: DEFAULT_MAX_TRACKED,
        }
    }
}

impl SentMessageTracker {
    /// Create a new tracker
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new tracker with a custom capacity limit
    pub fn with_capacity(max_tracked: usize) -> Self {
        Self {
            sent_texts: Vec::new(),
            sent_normalized: HashMap::new(),
            records: Vec::new(),
            max_tracked,
        }
    }

    /// Check if text would be a duplicate
    pub fn is_duplicate(&self, text: &str) -> bool {
        let normalized = normalize_text(text);
        self.sent_normalized.contains_key(&normalized)
    }

    /// Record a sent message
    pub fn record(&mut self, text: &str, channel: &str, user_id: Option<&str>) {
        let normalized = normalize_text(text);

        // Evict oldest if at capacity
        if self.sent_texts.len() >= self.max_tracked {
            let evicted = self.sent_texts.remove(0);
            let evicted_norm = normalize_text(&evicted);
            // Refcount: only drop the normalized entry once the last raw text
            // sharing this form is evicted, otherwise a still-tracked duplicate
            // would stop being detected.
            if let Some(count) = self.sent_normalized.get_mut(&evicted_norm) {
                *count -= 1;
                if *count == 0 {
                    self.sent_normalized.remove(&evicted_norm);
                }
            }
            self.records.remove(0);
        }

        self.sent_texts.push(text.to_string());
        *self.sent_normalized.entry(normalized).or_insert(0) += 1;
        self.records.push(SentRecord {
            channel: channel.to_string(),
            user_id: user_id.map(|s| s.to_string()),
            text: text.to_string(),
            sent_at: Instant::now(),
        });
    }

    /// Check if duplicate, if not record it
    ///
    /// Returns true if this is a new (non-duplicate) message
    pub fn check_and_record(&mut self, text: &str, channel: &str, user_id: Option<&str>) -> bool {
        if self.is_duplicate(text) {
            return false;
        }
        self.record(text, channel, user_id);
        true
    }

    /// Get all sent texts
    pub fn all_texts(&self) -> &[String] {
        &self.sent_texts
    }

    /// Get all records
    pub fn all_records(&self) -> &[SentRecord] {
        &self.records
    }

    /// Get count of sent messages
    pub fn count(&self) -> usize {
        self.sent_texts.len()
    }

    /// Reset tracker
    pub fn reset(&mut self) {
        self.sent_texts.clear();
        self.sent_normalized.clear();
        self.records.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_text() {
        assert_eq!(normalize_text("  Hello   World  "), "hello world");
        assert_eq!(normalize_text("Hello, World!"), "hello world");
        assert_eq!(normalize_text("你好，世界！"), "你好世界");
    }

    #[test]
    fn test_is_text_duplicate() {
        assert!(is_text_duplicate("Hello World", "hello world"));
        assert!(is_text_duplicate("Hello, World!", "Hello World"));
        assert!(!is_text_duplicate("Hello", "World"));
    }

    #[test]
    fn test_tracker_is_duplicate() {
        let mut tracker = SentMessageTracker::new();
        tracker.record("Hello World", "telegram", None);

        assert!(tracker.is_duplicate("Hello World"));
        assert!(tracker.is_duplicate("hello world"));
        assert!(tracker.is_duplicate("Hello, World!"));
        assert!(!tracker.is_duplicate("Goodbye"));
    }

    #[test]
    fn test_check_and_record() {
        let mut tracker = SentMessageTracker::new();

        // First time should succeed
        assert!(tracker.check_and_record("Hello", "telegram", None));
        assert_eq!(tracker.count(), 1);

        // Duplicate should fail
        assert!(!tracker.check_and_record("Hello", "telegram", None));
        assert_eq!(tracker.count(), 1);

        // Different message should succeed
        assert!(tracker.check_and_record("World", "telegram", None));
        assert_eq!(tracker.count(), 2);
    }

    #[test]
    fn test_reset() {
        let mut tracker = SentMessageTracker::new();
        tracker.record("Test", "channel", Some("user1"));

        tracker.reset();
        assert_eq!(tracker.count(), 0);
        assert!(!tracker.is_duplicate("Test"));
    }

    #[test]
    fn test_records_metadata() {
        let mut tracker = SentMessageTracker::new();
        tracker.record("Test message", "telegram", Some("user123"));

        let records = tracker.all_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].channel, "telegram");
        assert_eq!(records[0].user_id, Some("user123".to_string()));
        assert_eq!(records[0].text, "Test message");
    }
}
