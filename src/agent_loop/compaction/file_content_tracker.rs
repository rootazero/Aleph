//! FileContentTracker — LRU cache of recently read file contents for
//! post-compaction context recovery.
//!
//! After a compaction pass the LLM may lose the contents of files it was
//! actively editing.  [`FileContentTracker`] records up to
//! [`MAX_TRACKED_FILES`] recent file reads and exposes them via the
//! [`ConstraintSource`] trait so [`ConstraintInjector`] can re-inject them
//! into the next assistant turn.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::constraint_injector::{Constraint, ConstraintCategory, ConstraintSource};

// =============================================================================
// Constants
// =============================================================================

/// Maximum number of distinct files retained in the LRU cache.
const MAX_TRACKED_FILES: usize = 5;

/// Maximum number of characters included in the injected preview.
const MAX_PREVIEW_CHARS: usize = 5000;

// =============================================================================
// FileReadRecord
// =============================================================================

/// Internal record of a single file read.
#[derive(Debug, Clone)]
struct FileReadRecord {
    /// Absolute or relative path of the file.
    path: String,
    /// Truncated preview of the file contents.
    preview: String,
    /// Total number of lines in the original content.
    line_count: usize,
}

// =============================================================================
// FileContentTracker
// =============================================================================

/// Tracks recently read files and emits them as [`Constraint`]s after
/// compaction.
///
/// Thread-safe: all mutation goes through an internal `Mutex`.
pub struct FileContentTracker {
    recent_reads: Mutex<VecDeque<FileReadRecord>>,
}

impl FileContentTracker {
    /// Create a new, empty tracker.
    pub fn new() -> Self {
        Self {
            recent_reads: Mutex::new(VecDeque::with_capacity(MAX_TRACKED_FILES)),
        }
    }

    /// Record a file read.
    ///
    /// If the same `path` was already tracked, the old entry is removed and
    /// replaced with the new content (deduplication — newest wins).  When the
    /// cache exceeds [`MAX_TRACKED_FILES`] entries, the oldest entry is evicted.
    pub fn record_read(&self, path: &str, content: &str) {
        let line_count = content.lines().count();
        let preview = truncate_preview(content, MAX_PREVIEW_CHARS);
        let record = FileReadRecord {
            path: path.to_string(),
            preview,
            line_count,
        };

        let mut deque = self.recent_reads.lock().unwrap_or_else(|e| e.into_inner());

        // Deduplicate: remove an existing entry for the same path.
        if let Some(pos) = deque.iter().position(|r| r.path == path) {
            deque.remove(pos);
        }

        // Evict oldest if at capacity.
        if deque.len() >= MAX_TRACKED_FILES {
            deque.pop_front();
        }

        deque.push_back(record);
    }
}

impl Default for FileContentTracker {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// ConstraintSource impl
// =============================================================================

impl ConstraintSource for FileContentTracker {
    fn collect_constraints(&self) -> Vec<Constraint> {
        let deque = self.recent_reads.lock().unwrap_or_else(|e| e.into_inner());

        deque
            .iter()
            .map(|r| {
                let content = format!(
                    "**{}** ({} lines)\n```\n{}\n```",
                    r.path, r.line_count, r.preview
                );
                Constraint {
                    category: ConstraintCategory::RecentFile,
                    content,
                    priority: 60,
                }
            })
            .collect()
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Truncate `content` to at most `max_chars` characters, cutting at a newline
/// boundary where possible and appending `...[truncated]` when the content was
/// shortened.
fn truncate_preview(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }

    let byte_limit = content
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(content.len());

    let cut = content[..byte_limit]
        .rfind('\n')
        .map(|pos| pos + 1)
        .unwrap_or(byte_limit);

    let truncated = content.get(..cut).unwrap_or(&content[..byte_limit]);
    format!("{}...[truncated]", truncated)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_collect() {
        let tracker = FileContentTracker::new();
        tracker.record_read("/src/main.rs", "fn main() {\n    println!(\"hello\");\n}\n");

        let constraints = tracker.collect_constraints();
        assert_eq!(constraints.len(), 1);

        let c = &constraints[0];
        assert_eq!(c.category, ConstraintCategory::RecentFile);
        assert_eq!(c.priority, 60);
        assert!(c.content.contains("**/src/main.rs**"));
        assert!(c.content.contains("3 lines"));
        assert!(c.content.contains("fn main()"));
    }

    #[test]
    fn deduplicates_by_path() {
        let tracker = FileContentTracker::new();
        tracker.record_read("/src/lib.rs", "// version 1\n");
        tracker.record_read("/src/lib.rs", "// version 2\n");

        let constraints = tracker.collect_constraints();
        assert_eq!(
            constraints.len(),
            1,
            "duplicate path should produce only one entry"
        );
        assert!(
            constraints[0].content.contains("version 2"),
            "content should be the latest"
        );
    }

    #[test]
    fn evicts_oldest_beyond_capacity() {
        let tracker = FileContentTracker::new();
        for i in 0..7 {
            tracker.record_read(&format!("/file_{i}.rs"), &format!("content {i}\n"));
        }

        let constraints = tracker.collect_constraints();
        assert_eq!(
            constraints.len(),
            MAX_TRACKED_FILES,
            "should retain exactly MAX_TRACKED_FILES entries"
        );

        // The two oldest (/file_0.rs, /file_1.rs) should have been evicted.
        let paths: Vec<&str> = constraints
            .iter()
            .map(|c| {
                // Extract path from "**<path>** (...)"
                c.content.split("**").nth(1).unwrap_or("")
            })
            .collect();

        assert!(
            !paths.contains(&"/file_0.rs"),
            "oldest entry should be evicted"
        );
        assert!(
            !paths.contains(&"/file_1.rs"),
            "second oldest entry should be evicted"
        );
        assert!(
            paths.contains(&"/file_6.rs"),
            "newest entry should be retained"
        );
    }

    #[test]
    fn truncates_large_preview() {
        let large_content: String = "x".repeat(10_000);
        let tracker = FileContentTracker::new();
        tracker.record_read("/big.rs", &large_content);

        let constraints = tracker.collect_constraints();
        assert_eq!(constraints.len(), 1);
        assert!(
            constraints[0].content.contains("...[truncated]"),
            "large file content should be truncated"
        );
        // The total constraint content should not balloon far beyond MAX_PREVIEW_CHARS.
        assert!(
            constraints[0].content.len() < MAX_PREVIEW_CHARS + 200,
            "truncated preview should not exceed MAX_PREVIEW_CHARS significantly"
        );
    }
}
