//! TabooBuffer: sliding window micro-taboo detection.
//!
//! Detects when the same failure pattern repeats N times consecutively,
//! generating a taboo warning prompt to force strategy changes.

use std::collections::VecDeque;

use crate::poe::Verdict;

/// A verdict annotated with a semantic failure tag and reason.
#[derive(Debug, Clone)]
pub struct TaggedVerdict {
    /// The original verdict from evaluation
    pub verdict: Verdict,

    /// Failure category (e.g., "PermissionDenied", "CompilationError")
    pub semantic_tag: String,

    /// Human-readable failure reason for prompt injection
    pub failure_reason: String,
}

impl TaggedVerdict {
    /// Create a new tagged verdict.
    pub fn new(
        verdict: Verdict,
        semantic_tag: impl Into<String>,
        failure_reason: impl Into<String>,
    ) -> Self {
        Self {
            verdict,
            semantic_tag: semantic_tag.into(),
            failure_reason: failure_reason.into(),
        }
    }
}

/// Sliding window buffer that detects consecutive same-tag failures.
///
/// When the same `semantic_tag` appears `repetition_threshold` times
/// consecutively at the tail of the window, a taboo warning is generated.
#[derive(Debug)]
pub struct TabooBuffer {
    /// Sliding window of tagged verdicts
    window: VecDeque<TaggedVerdict>,

    /// Number of consecutive same-tag failures required to trigger
    repetition_threshold: usize,

    /// Maximum entries in the window
    window_size: usize,
}

impl TabooBuffer {
    /// Create a new TabooBuffer with the given repetition threshold.
    ///
    /// Window size defaults to `threshold * 2`.
    pub fn new(repetition_threshold: usize) -> Self {
        Self {
            window: VecDeque::new(),
            repetition_threshold,
            window_size: repetition_threshold * 2,
        }
    }

    /// Create a new TabooBuffer with explicit window size.
    pub fn with_window(repetition_threshold: usize, window_size: usize) -> Self {
        Self {
            window: VecDeque::new(),
            repetition_threshold,
            window_size,
        }
    }

    /// Record a tagged verdict, maintaining window size.
    pub fn record(&mut self, tagged_verdict: TaggedVerdict) {
        self.window.push_back(tagged_verdict);
        while self.window.len() > self.window_size {
            self.window.pop_front();
        }
    }

    /// Check for micro-taboo: consecutive same-tag failures at the tail.
    ///
    /// Returns a taboo warning prompt string if the threshold is reached.
    pub fn check_micro_taboo(&self) -> Option<String> {
        if self.window.len() < self.repetition_threshold {
            return None;
        }

        // Count consecutive same-tag entries from the end
        let last = self.window.back()?;
        let tag = &last.semantic_tag;

        let consecutive_count = self
            .window
            .iter()
            .rev()
            .take_while(|tv| tv.semantic_tag == *tag)
            .count();

        if consecutive_count >= self.repetition_threshold {
            // Collect recent failure reasons
            let reasons: Vec<&str> = self
                .window
                .iter()
                .rev()
                .take(consecutive_count)
                .map(|tv| tv.failure_reason.as_str())
                .collect();

            Some(format!(
                "TABOO WARNING: You have failed {} consecutive times with the same root cause: \
                 [{}]. Recent errors: {}. This approach is FORBIDDEN. You MUST try a completely \
                 different strategy.",
                consecutive_count,
                tag,
                reasons.join("; "),
            ))
        } else {
            None
        }
    }

    /// Clear the buffer.
    pub fn clear(&mut self) {
        self.window.clear();
    }

    /// Number of entries in the buffer.
    pub fn len(&self) -> usize {
        self.window.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.window.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::Verdict;

    fn make_tagged(tag: &str, reason: &str) -> TaggedVerdict {
        TaggedVerdict::new(Verdict::failure(reason), tag, reason)
    }

    #[test]
    fn empty_buffer_no_trigger() {
        let buf = TabooBuffer::new(3);
        assert!(buf.check_micro_taboo().is_none());
        assert!(buf.is_empty());
    }

    #[test]
    fn below_threshold_no_trigger() {
        let mut buf = TabooBuffer::new(3);
        buf.record(make_tagged("CompilationError", "syntax error line 10"));
        buf.record(make_tagged("CompilationError", "syntax error line 20"));
        assert!(buf.check_micro_taboo().is_none());
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn threshold_reached_triggers_with_correct_message() {
        let mut buf = TabooBuffer::new(3);
        buf.record(make_tagged("CompilationError", "error 1"));
        buf.record(make_tagged("CompilationError", "error 2"));
        buf.record(make_tagged("CompilationError", "error 3"));

        let warning = buf.check_micro_taboo();
        assert!(warning.is_some());
        let msg = warning.unwrap();
        assert!(msg.contains("3 consecutive times"));
        assert!(msg.contains("[CompilationError]"));
        assert!(msg.contains("FORBIDDEN"));
        assert!(msg.contains("error 1"));
        assert!(msg.contains("error 3"));
    }

    #[test]
    fn mixed_tags_no_trigger() {
        let mut buf = TabooBuffer::new(3);
        buf.record(make_tagged("CompilationError", "err 1"));
        buf.record(make_tagged("PermissionDenied", "err 2"));
        buf.record(make_tagged("CompilationError", "err 3"));
        assert!(buf.check_micro_taboo().is_none());
    }

    #[test]
    fn sliding_window_shifts_trigger_disappears() {
        let mut buf = TabooBuffer::new(3);
        buf.record(make_tagged("CompilationError", "err 1"));
        buf.record(make_tagged("CompilationError", "err 2"));
        buf.record(make_tagged("CompilationError", "err 3"));
        assert!(buf.check_micro_taboo().is_some());

        // Push a different tag — consecutive run breaks
        buf.record(make_tagged("NetworkError", "timeout"));
        assert!(buf.check_micro_taboo().is_none());
    }

    #[test]
    fn custom_window_size_respected() {
        let mut buf = TabooBuffer::with_window(2, 3);
        buf.record(make_tagged("A", "a1"));
        buf.record(make_tagged("A", "a2"));
        buf.record(make_tagged("A", "a3"));
        buf.record(make_tagged("A", "a4"));

        // Window size is 3, so only 3 entries remain
        assert_eq!(buf.len(), 3);
        assert!(buf.check_micro_taboo().is_some());
    }

    #[test]
    fn clear_resets_buffer() {
        let mut buf = TabooBuffer::new(2);
        buf.record(make_tagged("X", "x1"));
        buf.record(make_tagged("X", "x2"));
        assert!(buf.check_micro_taboo().is_some());

        buf.clear();
        assert!(buf.is_empty());
        assert!(buf.check_micro_taboo().is_none());
    }
}
