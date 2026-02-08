//! Rule Learner - ML-based L2 Rule Generation
//!
//! This module implements machine learning-based rule generation for the L2 routing layer.
//! It learns from successful L3 executions and automatically generates L2 routing rules.
//!
//! # Architecture
//!
//! ```text
//! L3 Success → Pattern Extraction → Frequency Analysis → Rule Generation → L2 Integration
//! ```
//!
//! # Learning Strategy
//!
//! 1. **Pattern Extraction**: Extract common patterns from user inputs
//! 2. **Frequency Analysis**: Track how often patterns lead to specific actions
//! 3. **Confidence Scoring**: Calculate confidence based on success rate
//! 4. **Rule Generation**: Generate L2 rules when confidence threshold is met
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::engine::{RuleLearner, AtomicAction};
//!
//! let mut learner = RuleLearner::new();
//!
//! // Learn from successful L3 execution
//! learner.learn("search for TODO", AtomicAction::Search { /* ... */ });
//! learner.learn("search for TODO", AtomicAction::Search { /* ... */ });
//! learner.learn("search for TODO", AtomicAction::Search { /* ... */ });
//!
//! // Generate L2 rules when confidence is high enough
//! if let Some(rule) = learner.generate_rule("search for TODO") {
//!     // Add rule to ReflexLayer
//! }
//! ```

use super::{AtomicAction, KeywordRule, FeatureExtractor, FeatureVector, NaiveBayesClassifier, ActionClass};
use dashmap::DashMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info};

/// Minimum number of successful executions before generating a rule
const MIN_EXECUTIONS: usize = 3;

/// Minimum confidence score (0.0-1.0) before generating a rule
const MIN_CONFIDENCE: f64 = 0.8;

/// Pattern learning record
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PatternRecord {
    /// User input pattern
    pattern: String,
    /// Action that was executed
    action: AtomicAction,
    /// Number of times this pattern → action mapping was observed
    count: usize,
    /// Number of successful executions
    successes: usize,
    /// Number of failed executions
    failures: usize,
    /// Extracted features (for advanced learning)
    features: Option<FeatureVector>,
}

impl PatternRecord {
    /// Calculate confidence score (success rate)
    fn confidence(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.successes as f64 / self.count as f64
        }
    }

    /// Check if this record is ready for rule generation
    fn is_ready(&self) -> bool {
        self.count >= MIN_EXECUTIONS && self.confidence() >= MIN_CONFIDENCE
    }
}

/// Rule learner for ML-based L2 rule generation
pub struct RuleLearner {
    /// Pattern records indexed by normalized input
    records: DashMap<String, PatternRecord>,

    /// Feature extractor
    feature_extractor: FeatureExtractor,

    /// Naive Bayes classifier for action prediction
    classifier: Arc<RwLock<NaiveBayesClassifier>>,

    /// Statistics
    stats: Arc<RwLock<LearnerStats>>,
}

impl RuleLearner {
    /// Create a new rule learner
    pub fn new() -> Self {
        Self {
            records: DashMap::new(),
            feature_extractor: FeatureExtractor::new(),
            classifier: Arc::new(RwLock::new(NaiveBayesClassifier::new())),
            stats: Arc::new(RwLock::new(LearnerStats::default())),
        }
    }

    /// Learn from a successful L3 execution
    ///
    /// # Arguments
    ///
    /// * `input` - The user input that triggered the execution
    /// * `action` - The atomic action that was executed
    pub fn learn_success(&self, input: &str, action: AtomicAction) {
        let normalized = self.normalize_input(input);
        let features = self.feature_extractor.extract(input);

        // Train the classifier
        if let Some(action_class) = Self::action_to_class(&action) {
            self.classifier.write().unwrap().train(&features, action_class);
        }

        self.records
            .entry(normalized.clone())
            .and_modify(|record| {
                record.count += 1;
                record.successes += 1;
                record.features = Some(features.clone());
            })
            .or_insert_with(|| PatternRecord {
                pattern: normalized.clone(),
                action: action.clone(),
                count: 1,
                successes: 1,
                failures: 0,
                features: Some(features),
            });

        self.stats.write().unwrap().total_observations += 1;

        debug!(
            input = %input,
            normalized = %normalized,
            action = ?action,
            "Learned from successful L3 execution"
        );
    }

    /// Learn from a failed L3 execution
    ///
    /// # Arguments
    ///
    /// * `input` - The user input that triggered the execution
    /// * `action` - The atomic action that was attempted
    pub fn learn_failure(&self, input: &str, action: AtomicAction) {
        let normalized = self.normalize_input(input);
        let features = self.feature_extractor.extract(input);

        self.records
            .entry(normalized.clone())
            .and_modify(|record| {
                record.count += 1;
                record.failures += 1;
                record.features = Some(features.clone());
            })
            .or_insert_with(|| PatternRecord {
                pattern: normalized.clone(),
                action: action.clone(),
                count: 1,
                successes: 0,
                failures: 1,
                features: Some(features),
            });

        self.stats.write().unwrap().total_observations += 1;

        debug!(
            input = %input,
            normalized = %normalized,
            action = ?action,
            "Learned from failed L3 execution"
        );
    }

    /// Generate L2 routing rules from learned patterns
    ///
    /// Returns a list of keyword rules that can be added to the ReflexLayer.
    pub fn generate_rules(&self) -> Vec<KeywordRule> {
        let mut rules = Vec::new();

        for entry in self.records.iter() {
            let record = entry.value();

            if record.is_ready() {
                if let Some(rule) = self.create_rule(record) {
                    rules.push(rule);
                    info!(
                        pattern = %record.pattern,
                        confidence = %record.confidence(),
                        count = record.count,
                        "Generated L2 rule from learned pattern"
                    );
                }
            }
        }

        self.stats.write().unwrap().rules_generated += rules.len();

        rules
    }

    /// Normalize user input for pattern matching
    ///
    /// This removes case sensitivity and extra whitespace.
    fn normalize_input(&self, input: &str) -> String {
        input.trim().to_lowercase()
    }

    /// Create a keyword rule from a pattern record
    fn create_rule(&self, record: &PatternRecord) -> Option<KeywordRule> {
        // Extract pattern from the input
        // For now, we use simple regex patterns
        // TODO: Implement more sophisticated pattern extraction

        let pattern_str = format!("(?i)^{}$", regex::escape(&record.pattern));
        let pattern = Regex::new(&pattern_str).ok()?;

        // Determine action type and priority based on the atomic action
        let (action_type, priority) = match &record.action {
            AtomicAction::Read { .. } => (super::ActionType::Read, 80),
            AtomicAction::Write { .. } => (super::ActionType::Write, 80),
            AtomicAction::Bash { .. } => (super::ActionType::Bash, 85),
            AtomicAction::Search { .. } => (super::ActionType::Search, 75),
            AtomicAction::Replace { .. } => (super::ActionType::Replace, 75),
            AtomicAction::Move { .. } => (super::ActionType::Move, 75),
            _ => return None, // Edit is too complex for L2 routing
        };

        // Create a simple extractor that captures the entire input
        let extractor = Box::new(SimpleExtractor {
            action: record.action.clone(),
        });

        Some(KeywordRule {
            pattern,
            priority,
            action_type,
            extractor,
        })
    }

    /// Get learning statistics
    pub fn stats(&self) -> LearnerStats {
        self.stats.read().unwrap().clone()
    }

    /// Clear all learned patterns
    pub fn clear(&self) {
        self.records.clear();
        info!("Cleared all learned patterns");
    }

    /// Get number of learned patterns
    pub fn pattern_count(&self) -> usize {
        self.records.len()
    }

    /// Predict action for a given input using the classifier
    ///
    /// Returns the predicted action class and confidence score.
    pub fn predict(&self, input: &str) -> Option<(ActionClass, f64)> {
        let features = self.feature_extractor.extract(input);
        self.classifier.read().unwrap().predict(&features)
    }

    /// Convert AtomicAction to ActionClass for classifier training
    fn action_to_class(action: &AtomicAction) -> Option<ActionClass> {
        match action {
            AtomicAction::Read { .. } => Some(ActionClass::Read),
            AtomicAction::Write { .. } => Some(ActionClass::Write),
            AtomicAction::Edit { .. } => Some(ActionClass::Edit),
            AtomicAction::Bash { .. } => Some(ActionClass::Bash),
            AtomicAction::Search { .. } => Some(ActionClass::Search),
            AtomicAction::Replace { .. } => Some(ActionClass::Replace),
            AtomicAction::Move { .. } => Some(ActionClass::Move),
        }
    }
}

impl Default for RuleLearner {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple parameter extractor that uses the learned action directly
struct SimpleExtractor {
    action: AtomicAction,
}

impl super::ParamExtractor for SimpleExtractor {
    fn extract(&self, _input: &str) -> Option<HashMap<String, serde_json::Value>> {
        // For learned patterns, we already know the exact action
        // So we just return empty params and let the build_action use the stored action
        Some(HashMap::new())
    }
}

/// Learning statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearnerStats {
    /// Total number of observations (successes + failures)
    pub total_observations: usize,

    /// Number of rules generated
    pub rules_generated: usize,
}

impl LearnerStats {
    /// Get average observations per rule
    pub fn avg_observations_per_rule(&self) -> f64 {
        if self.rules_generated == 0 {
            0.0
        } else {
            self.total_observations as f64 / self.rules_generated as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{SearchPattern, SearchScope};

    #[test]
    fn test_rule_learner_basic() {
        let learner = RuleLearner::new();

        let action = AtomicAction::Bash {
            command: "git status".to_string(),
            cwd: None,
        };

        // Learn from multiple successful executions
        learner.learn_success("git status", action.clone());
        learner.learn_success("git status", action.clone());
        learner.learn_success("git status", action.clone());

        // Should have one pattern
        assert_eq!(learner.pattern_count(), 1);

        // Generate rules
        let rules = learner.generate_rules();
        assert_eq!(rules.len(), 1);

        // Check stats
        let stats = learner.stats();
        assert_eq!(stats.total_observations, 3);
        assert_eq!(stats.rules_generated, 1);
    }

    #[test]
    fn test_rule_learner_confidence() {
        let learner = RuleLearner::new();

        let action = AtomicAction::Bash {
            command: "test".to_string(),
            cwd: None,
        };

        // Learn with mixed success/failure
        learner.learn_success("test command", action.clone());
        learner.learn_success("test command", action.clone());
        learner.learn_failure("test command", action.clone());

        // Confidence is 2/3 = 0.666, below MIN_CONFIDENCE (0.8)
        let rules = learner.generate_rules();
        assert_eq!(rules.len(), 0); // Should not generate rule

        // Add more successes
        learner.learn_success("test command", action.clone());
        learner.learn_success("test command", action.clone());

        // Now confidence is 4/5 = 0.8, meets MIN_CONFIDENCE
        let rules = learner.generate_rules();
        assert_eq!(rules.len(), 1); // Should generate rule
    }

    #[test]
    fn test_rule_learner_normalization() {
        let learner = RuleLearner::new();

        let action = AtomicAction::Bash {
            command: "ls".to_string(),
            cwd: None,
        };

        // Learn with different cases and whitespace
        learner.learn_success("  LS  ", action.clone());
        learner.learn_success("ls", action.clone());
        learner.learn_success("Ls", action.clone());

        // Should normalize to same pattern
        assert_eq!(learner.pattern_count(), 1);
    }

    #[test]
    fn test_rule_learner_clear() {
        let learner = RuleLearner::new();

        let action = AtomicAction::Bash {
            command: "test".to_string(),
            cwd: None,
        };

        learner.learn_success("test", action);
        assert_eq!(learner.pattern_count(), 1);

        learner.clear();
        assert_eq!(learner.pattern_count(), 0);
    }

    #[test]
    fn test_classifier_integration() {
        let learner = RuleLearner::new();

        // Train with search actions
        let search_action = AtomicAction::Search {
            pattern: SearchPattern::Regex {
                pattern: "TODO".to_string(),
            },
            scope: SearchScope::Workspace,
            filters: vec![],
        };

        learner.learn_success("search for TODO", search_action.clone());
        learner.learn_success("find TODO in file", search_action.clone());
        learner.learn_success("look for TODO", search_action);

        // Train with bash actions
        let bash_action = AtomicAction::Bash {
            command: "git status".to_string(),
            cwd: None,
        };

        learner.learn_success("run git status", bash_action.clone());
        learner.learn_success("execute git status", bash_action.clone());
        learner.learn_success("git status", bash_action);

        // Predict action for similar inputs
        let prediction = learner.predict("search for FIXME");
        assert!(prediction.is_some());
        let (action_class, confidence) = prediction.unwrap();
        assert_eq!(action_class, ActionClass::Search);
        assert!(confidence > 0.0);

        let prediction = learner.predict("run git diff");
        assert!(prediction.is_some());
        let (action_class, confidence) = prediction.unwrap();
        assert_eq!(action_class, ActionClass::Bash);
        assert!(confidence > 0.0);
    }
}
