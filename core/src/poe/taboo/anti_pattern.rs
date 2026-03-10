//! AntiPattern: persistent record of failed approaches for future avoidance.
//!
//! When a POE task exhausts its budget, the failure pattern is crystallized
//! into an AntiPattern that can be recalled for similar future tasks.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// A persistent record of a failed approach.
///
/// Stored after BudgetExhausted to prevent the agent from repeating
/// the same doomed strategy on similar tasks.
#[derive(Debug, Clone)]
pub struct AntiPattern {
    /// Task pattern this anti-pattern applies to
    pub pattern_id: String,

    /// Human-readable description of what went wrong
    pub description: String,

    /// Failure categories encountered during the attempts
    pub failure_tags: Vec<String>,

    /// Number of attempts made before giving up
    pub attempts_made: u8,

    /// Unix timestamp when this anti-pattern was created
    pub created_at: i64,

    /// Optional metadata for extensibility
    pub metadata: HashMap<String, String>,
}

impl AntiPattern {
    /// Create a new AntiPattern with the current timestamp.
    pub fn new(
        pattern_id: impl Into<String>,
        description: impl Into<String>,
        failure_tags: Vec<String>,
    ) -> Self {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        Self {
            pattern_id: pattern_id.into(),
            description: description.into(),
            failure_tags,
            attempts_made: 0,
            created_at,
            metadata: HashMap::new(),
        }
    }

    /// Set the number of attempts made.
    pub fn with_attempts(mut self, attempts: u8) -> Self {
        self.attempts_made = attempts;
        self
    }

    /// Generate an avoidance prompt for injection into the LLM context.
    pub fn to_avoidance_prompt(&self) -> String {
        let tags = if self.failure_tags.is_empty() {
            "none".to_string()
        } else {
            self.failure_tags.join(", ")
        };

        format!(
            "AVOID: Pattern [{}] failed after {} attempts. \
             Reason: {}. Failure categories: [{}]. \
             Do NOT repeat this approach.",
            self.pattern_id, self.attempts_made, self.description, tags,
        )
    }
}

/// In-memory store for anti-patterns.
///
/// Provides simple insert/query by pattern_id. A persistent backend
/// (e.g., LanceDB) can replace this for production use.
#[derive(Debug, Default)]
pub struct InMemoryAntiPatternStore {
    patterns: Vec<AntiPattern>,
}

impl InMemoryAntiPatternStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an anti-pattern into the store.
    pub fn insert(&mut self, pattern: AntiPattern) {
        self.patterns.push(pattern);
    }

    /// Find all anti-patterns matching the given pattern_id.
    pub fn find_by_pattern_id(&self, pattern_id: &str) -> Vec<&AntiPattern> {
        self.patterns
            .iter()
            .filter(|p| p.pattern_id == pattern_id)
            .collect()
    }

    /// Number of anti-patterns in the store.
    pub fn len(&self) -> usize {
        self.patterns.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_with_timestamp() {
        let ap = AntiPattern::new(
            "rust-compilation",
            "Repeated syntax errors in generated code",
            vec!["CompilationError".to_string()],
        );

        assert_eq!(ap.pattern_id, "rust-compilation");
        assert!(ap.created_at > 0);
        assert_eq!(ap.attempts_made, 0);
    }

    #[test]
    fn avoidance_prompt_format() {
        let ap = AntiPattern::new(
            "db-migration",
            "Schema conflicts on concurrent migrations",
            vec!["SchemaConflict".to_string(), "LockTimeout".to_string()],
        )
        .with_attempts(5);

        let prompt = ap.to_avoidance_prompt();
        assert!(prompt.starts_with("AVOID:"));
        assert!(prompt.contains("[db-migration]"));
        assert!(prompt.contains("5 attempts"));
        assert!(prompt.contains("SchemaConflict, LockTimeout"));
        assert!(prompt.contains("Do NOT repeat"));
    }

    #[test]
    fn store_insert_and_retrieve() {
        let mut store = InMemoryAntiPatternStore::new();
        assert!(store.is_empty());

        store.insert(AntiPattern::new("p1", "desc1", vec!["tag1".into()]));
        store.insert(AntiPattern::new("p1", "desc2", vec!["tag2".into()]));
        store.insert(AntiPattern::new("p2", "desc3", vec!["tag3".into()]));

        assert_eq!(store.len(), 3);
        assert_eq!(store.find_by_pattern_id("p1").len(), 2);
        assert_eq!(store.find_by_pattern_id("p2").len(), 1);
    }

    #[test]
    fn empty_query_returns_empty() {
        let store = InMemoryAntiPatternStore::new();
        assert!(store.find_by_pattern_id("nonexistent").is_empty());
    }
}
