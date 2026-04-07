//! Post-compaction context recovery via constraint injection.
//!
//! After a compaction pass the LLM may lose track of the active task,
//! available tools, and user preferences that were only present in the
//! now-compacted messages.  [`ConstraintInjector`] collects those facts
//! from pluggable [`ConstraintSource`] implementations and formats them
//! into an XML block that can be prepended to the next assistant turn.

use std::sync::Arc;

use super::types::{CompactionResult, PostCompactCleanup};

// =============================================================================
// ConstraintCategory
// =============================================================================

/// Semantic category of a single constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintCategory {
    /// Current task description and progress state.
    ActiveTask,
    /// Currently available tool list.
    ActiveTools,
    /// High-frequency user preferences retrieved from memory.
    UserPreference,
    /// Recently read file content for post-compaction recovery.
    RecentFile,
}

// =============================================================================
// Constraint
// =============================================================================

/// A single recoverable constraint emitted by a [`ConstraintSource`].
#[derive(Debug, Clone)]
pub struct Constraint {
    /// Semantic category of this constraint.
    pub category: ConstraintCategory,
    /// Human-readable content to inject into context.
    pub content: String,
    /// Priority in `[0, 255]`; higher values sort first.
    pub priority: u8,
}

// =============================================================================
// ConstraintSource
// =============================================================================

/// A pluggable source that emits [`Constraint`]s to be injected after
/// compaction.
pub trait ConstraintSource: Send + Sync {
    /// Collect all constraints currently available from this source.
    fn collect_constraints(&self) -> Vec<Constraint>;
}

// =============================================================================
// ConstraintInjector
// =============================================================================

/// Collects constraints from multiple [`ConstraintSource`]s, formats them
/// into an XML block, and exposes the result to the agent loop via
/// [`take_injection`].
///
/// Implements [`PostCompactCleanup`] so it can be registered as a cleanup
/// hook and automatically fire after every compaction pass.
pub struct ConstraintInjector {
    sources: Vec<Arc<dyn ConstraintSource>>,
    last_injection: std::sync::Mutex<Option<String>>,
}

impl ConstraintInjector {
    /// Create a new injector backed by the given sources.
    pub fn new(sources: Vec<Arc<dyn ConstraintSource>>) -> Self {
        Self {
            sources,
            last_injection: std::sync::Mutex::new(None),
        }
    }

    /// Collect constraints from all sources and sort by priority descending.
    pub fn collect_all(&self) -> Vec<Constraint> {
        let mut all: Vec<Constraint> = self
            .sources
            .iter()
            .flat_map(|s| s.collect_constraints())
            .collect();
        // Stable descending sort: highest priority comes first.
        all.sort_by(|a, b| b.priority.cmp(&a.priority));
        all
    }

    /// Format a slice of constraints into an XML block suitable for context
    /// injection.  Returns an empty string when `constraints` is empty.
    pub fn format_injection(&self, constraints: &[Constraint]) -> String {
        if constraints.is_empty() {
            return String::new();
        }

        let task_items: Vec<&str> = constraints
            .iter()
            .filter(|c| c.category == ConstraintCategory::ActiveTask)
            .map(|c| c.content.as_str())
            .collect();

        let tool_items: Vec<&str> = constraints
            .iter()
            .filter(|c| c.category == ConstraintCategory::ActiveTools)
            .map(|c| c.content.as_str())
            .collect();

        let pref_items: Vec<&str> = constraints
            .iter()
            .filter(|c| c.category == ConstraintCategory::UserPreference)
            .map(|c| c.content.as_str())
            .collect();

        let file_items: Vec<&str> = constraints
            .iter()
            .filter(|c| c.category == ConstraintCategory::RecentFile)
            .map(|c| c.content.as_str())
            .collect();

        let mut body = String::new();
        body.push_str("## Active Constraints (auto-restored after compaction)\n");

        if !task_items.is_empty() {
            body.push_str("\n### Task Context\n");
            for item in &task_items {
                body.push_str(&format!("- {item}\n"));
            }
        }

        if !tool_items.is_empty() {
            body.push_str("\n### Active Tools\n");
            for item in &tool_items {
                body.push_str(item);
                body.push('\n');
            }
        }

        if !pref_items.is_empty() {
            body.push_str("\n### Key Preferences\n");
            for item in &pref_items {
                body.push_str(&format!("- {item}\n"));
            }
        }

        if !file_items.is_empty() {
            body.push_str("\n### Recently Read Files\n");
            for item in &file_items {
                body.push_str(item);
                body.push('\n');
            }
        }

        format!("<post-compaction-context>\n{body}</post-compaction-context>")
    }

    /// Take and clear the last generated injection string.
    ///
    /// Returns `None` if no compaction has fired yet or if the injection was
    /// already consumed.
    pub fn take_injection(&self) -> Option<String> {
        self.last_injection
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

// =============================================================================
// PostCompactCleanup impl
// =============================================================================

impl PostCompactCleanup for ConstraintInjector {
    fn cleanup_order(&self) -> u32 {
        50
    }

    fn on_compact_complete(&self, _result: &CompactionResult) {
        let constraints = self.collect_all();
        let injection = self.format_injection(&constraints);
        if !injection.is_empty() {
            *self
                .last_injection
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(injection);
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    struct MockSource {
        constraints: Vec<Constraint>,
    }

    impl MockSource {
        fn new(constraints: Vec<Constraint>) -> Arc<Self> {
            Arc::new(Self { constraints })
        }
    }

    impl ConstraintSource for MockSource {
        fn collect_constraints(&self) -> Vec<Constraint> {
            self.constraints.clone()
        }
    }

    // -------------------------------------------------------------------------
    // Tests
    // -------------------------------------------------------------------------

    #[test]
    fn constraint_injection_format() {
        let source = MockSource::new(vec![
            Constraint {
                category: ConstraintCategory::ActiveTask,
                content: "Refactor the compaction module".to_string(),
                priority: 100,
            },
            Constraint {
                category: ConstraintCategory::ActiveTools,
                content: "read_file, write_file".to_string(),
                priority: 80,
            },
        ]);

        let injector = ConstraintInjector::new(vec![source]);
        let constraints = injector.collect_all();
        let output = injector.format_injection(&constraints);

        assert!(output.contains("<post-compaction-context>"));
        assert!(output.contains("</post-compaction-context>"));
        assert!(output.contains("### Task Context"));
        assert!(output.contains("Refactor the compaction module"));
        assert!(output.contains("### Active Tools"));
        assert!(output.contains("read_file, write_file"));
    }

    #[test]
    fn constraints_sorted_by_priority_descending() {
        let source = MockSource::new(vec![
            Constraint {
                category: ConstraintCategory::UserPreference,
                content: "prefer concise answers".to_string(),
                priority: 10,
            },
            Constraint {
                category: ConstraintCategory::ActiveTask,
                content: "implement trait".to_string(),
                priority: 200,
            },
            Constraint {
                category: ConstraintCategory::ActiveTools,
                content: "bash".to_string(),
                priority: 100,
            },
        ]);

        let injector = ConstraintInjector::new(vec![source]);
        let sorted = injector.collect_all();

        assert_eq!(sorted[0].priority, 200);
        assert_eq!(sorted[1].priority, 100);
        assert_eq!(sorted[2].priority, 10);
    }

    #[test]
    fn collects_from_multiple_sources() {
        let source_a = MockSource::new(vec![Constraint {
            category: ConstraintCategory::ActiveTask,
            content: "task from source A".to_string(),
            priority: 90,
        }]);

        let source_b = MockSource::new(vec![Constraint {
            category: ConstraintCategory::UserPreference,
            content: "preference from source B".to_string(),
            priority: 50,
        }]);

        let injector = ConstraintInjector::new(vec![source_a, source_b]);
        let all = injector.collect_all();

        assert_eq!(all.len(), 2);
        // Verify both sources contributed.
        assert!(all.iter().any(|c| c.content == "task from source A"));
        assert!(all.iter().any(|c| c.content == "preference from source B"));
    }
}
