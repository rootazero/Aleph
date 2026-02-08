//! Meta-cognition types for behavioral anchors and self-reflection
//!
//! This module defines the core data structures for Aleph's meta-cognition layer,
//! enabling the system to learn from failures and optimize its behavior over time.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A learned behavioral rule that guides future decision-making
///
/// BehavioralAnchors are generated through reactive reflection (pain learning)
/// or proactive reflection (excellence learning) and are dynamically injected
/// into the system prompt when relevant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehavioralAnchor {
    /// Unique identifier (UUID)
    pub id: String,

    /// Human-readable instruction text
    pub rule_text: String,

    /// Tags for context-based retrieval (e.g., ["Python", "macOS", "shell"])
    pub trigger_tags: Vec<String>,

    /// Confidence score (0.0-1.0), increases with validation
    pub confidence: f32,

    /// When this anchor was created
    pub created_at: DateTime<Utc>,

    /// Last time this anchor was validated as helpful
    pub last_validated: DateTime<Utc>,

    /// Number of times this anchor helped achieve success
    pub validation_count: u32,

    /// Number of times this anchor was present during failure
    pub failure_count: u32,

    /// Origin of this behavioral anchor
    pub source: AnchorSource,

    /// When this anchor should be applied
    pub scope: AnchorScope,

    /// Priority level (higher = more important)
    pub priority: i32,

    /// IDs of conflicting behavioral anchors
    pub conflicts_with: Vec<String>,

    /// ID of the anchor this one supersedes (if any)
    pub supersedes: Option<String>,
}

/// Source of a behavioral anchor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnchorSource {
    /// Generated from immediate failure response
    ReactiveReflection {
        task_id: String,
        error_type: String,
    },

    /// Generated from proactive optimization during idle time
    ProactiveReflection {
        pattern_hash: String,
        optimization_type: String,
    },

    /// Generated from explicit user feedback
    UserFeedback { session_id: String },

    /// Manually created by developer or user
    ManualInjection { author: String },
}

/// Scope defining when an anchor should be applied
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnchorScope {
    /// Always apply this anchor
    Global,

    /// Apply when specific tags match the context
    Tagged { tags: Vec<String> },

    /// Apply when a predicate condition is met
    Conditional { predicate: String },
}

impl BehavioralAnchor {
    /// Create a new behavioral anchor
    pub fn new(
        id: String,
        rule_text: String,
        trigger_tags: Vec<String>,
        source: AnchorSource,
        scope: AnchorScope,
        priority: i32,
        initial_confidence: f32,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            rule_text,
            trigger_tags,
            confidence: initial_confidence.clamp(0.0, 1.0),
            created_at: now,
            last_validated: now,
            validation_count: 0,
            failure_count: 0,
            source,
            scope,
            priority,
            conflicts_with: Vec::new(),
            supersedes: None,
        }
    }

    /// Update confidence based on validation or failure
    ///
    /// Uses exponential moving average to smooth confidence updates:
    /// - Validation: confidence += (1.0 - confidence) * 0.1
    /// - Failure: confidence *= 0.9
    pub fn update_confidence(&mut self, is_validation: bool) {
        if is_validation {
            self.validation_count += 1;
            self.last_validated = Utc::now();
            // Exponential approach to 1.0
            self.confidence += (1.0 - self.confidence) * 0.1;
        } else {
            self.failure_count += 1;
            // Exponential decay
            self.confidence *= 0.9;
        }
        // Ensure confidence stays in valid range
        self.confidence = self.confidence.clamp(0.0, 1.0);
    }

    /// Calculate validation rate (validation_count / total_count)
    pub fn validation_rate(&self) -> f32 {
        let total = self.validation_count + self.failure_count;
        if total == 0 {
            0.0
        } else {
            self.validation_count as f32 / total as f32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_behavioral_anchor() {
        let anchor = BehavioralAnchor::new(
            "test-id".to_string(),
            "Always check Python version".to_string(),
            vec!["Python".to_string(), "macOS".to_string()],
            AnchorSource::ReactiveReflection {
                task_id: "task-123".to_string(),
                error_type: "VersionMismatch".to_string(),
            },
            AnchorScope::Tagged {
                tags: vec!["Python".to_string()],
            },
            100,
            0.8,
        );

        assert_eq!(anchor.id, "test-id");
        assert_eq!(anchor.rule_text, "Always check Python version");
        assert_eq!(anchor.trigger_tags, vec!["Python", "macOS"]);
        assert_eq!(anchor.confidence, 0.8);
        assert_eq!(anchor.priority, 100);
        assert_eq!(anchor.validation_count, 0);
        assert_eq!(anchor.failure_count, 0);
    }

    #[test]
    fn test_confidence_clamps_to_valid_range() {
        let anchor = BehavioralAnchor::new(
            "test-id".to_string(),
            "Test rule".to_string(),
            vec![],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            50,
            1.5, // Invalid: > 1.0
        );

        assert_eq!(anchor.confidence, 1.0); // Should be clamped
    }

    #[test]
    fn test_update_confidence_validation() {
        let mut anchor = BehavioralAnchor::new(
            "test-id".to_string(),
            "Test rule".to_string(),
            vec![],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            50,
            0.5,
        );

        let initial_confidence = anchor.confidence;
        anchor.update_confidence(true);

        assert_eq!(anchor.validation_count, 1);
        assert!(anchor.confidence > initial_confidence);
        assert!(anchor.confidence <= 1.0);
    }

    #[test]
    fn test_update_confidence_failure() {
        let mut anchor = BehavioralAnchor::new(
            "test-id".to_string(),
            "Test rule".to_string(),
            vec![],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            50,
            0.8,
        );

        let initial_confidence = anchor.confidence;
        anchor.update_confidence(false);

        assert_eq!(anchor.failure_count, 1);
        assert!(anchor.confidence < initial_confidence);
        assert_eq!(anchor.confidence, 0.8 * 0.9);
    }

    #[test]
    fn test_validation_rate() {
        let mut anchor = BehavioralAnchor::new(
            "test-id".to_string(),
            "Test rule".to_string(),
            vec![],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            50,
            0.5,
        );

        // No validations or failures yet
        assert_eq!(anchor.validation_rate(), 0.0);

        // Add some validations and failures
        anchor.update_confidence(true); // validation
        anchor.update_confidence(true); // validation
        anchor.update_confidence(false); // failure

        // 2 validations out of 3 total = 0.666...
        assert!((anchor.validation_rate() - 0.666).abs() < 0.01);
    }
}
