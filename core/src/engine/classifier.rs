//! Naive Bayes Classifier for ML-based Rule Learning
//!
//! This module implements a lightweight Naive Bayes classifier for predicting
//! which AtomicAction should be executed based on extracted features.
//!
//! # Algorithm
//!
//! Naive Bayes uses Bayes' theorem with the "naive" assumption that features
//! are independent:
//!
//! ```text
//! P(Action|Features) = P(Features|Action) * P(Action) / P(Features)
//! ```
//!
//! We use the log-space to avoid numerical underflow:
//!
//! ```text
//! log P(Action|Features) = log P(Action) + Σ log P(Feature_i|Action)
//! ```

use super::feature_extractor::{Entity, FeatureVector, Intent};
use super::AtomicAction;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

/// Naive Bayes classifier for action prediction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NaiveBayesClassifier {
    /// Prior probabilities: P(Action)
    priors: HashMap<ActionClass, f64>,

    /// Feature likelihoods: P(Feature|Action)
    likelihoods: HashMap<ActionClass, FeatureLikelihoods>,

    /// Total number of training samples
    total_samples: usize,

    /// Smoothing parameter (Laplace smoothing)
    alpha: f64,
}

/// Action class for classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActionClass {
    Read,
    Write,
    Edit,
    Bash,
    Search,
    Replace,
    Move,
}

impl ActionClass {
    /// Convert from AtomicAction
    pub fn from_action(action: &AtomicAction) -> Self {
        match action {
            AtomicAction::Read { .. } => ActionClass::Read,
            AtomicAction::Write { .. } => ActionClass::Write,
            AtomicAction::Edit { .. } => ActionClass::Edit,
            AtomicAction::Bash { .. } => ActionClass::Bash,
            AtomicAction::Search { .. } => ActionClass::Search,
            AtomicAction::Replace { .. } => ActionClass::Replace,
            AtomicAction::Move { .. } => ActionClass::Move,
        }
    }
}

/// Feature likelihoods for a specific action class
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FeatureLikelihoods {
    /// Intent likelihoods: P(Intent|Action)
    intent_counts: HashMap<Intent, usize>,

    /// Keyword likelihoods: P(Keyword|Action)
    keyword_counts: HashMap<String, usize>,

    /// Entity type likelihoods: P(EntityType|Action)
    entity_type_counts: HashMap<EntityType, usize>,

    /// Total samples for this action class
    total_samples: usize,
}

impl FeatureLikelihoods {
    fn new() -> Self {
        Self {
            intent_counts: HashMap::new(),
            keyword_counts: HashMap::new(),
            entity_type_counts: HashMap::new(),
            total_samples: 0,
        }
    }
}

/// Entity type for classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum EntityType {
    FilePath,
    Command,
    Pattern,
    Directory,
}

impl EntityType {
    fn from_entity(entity: &Entity) -> Self {
        match entity {
            Entity::FilePath(_) => EntityType::FilePath,
            Entity::Command(_) => EntityType::Command,
            Entity::Pattern(_) => EntityType::Pattern,
            Entity::Directory(_) => EntityType::Directory,
        }
    }
}

impl NaiveBayesClassifier {
    /// Create a new Naive Bayes classifier
    pub fn new() -> Self {
        Self {
            priors: HashMap::new(),
            likelihoods: HashMap::new(),
            total_samples: 0,
            alpha: 1.0, // Laplace smoothing
        }
    }

    /// Train the classifier with a new sample (incremental learning)
    ///
    /// # Arguments
    ///
    /// * `features` - Extracted features from user input
    /// * `action_class` - The action class that was executed
    pub fn train(&mut self, features: &FeatureVector, action_class: ActionClass) {

        // Update total samples
        self.total_samples += 1;

        // Update prior: P(Action)
        *self.priors.entry(action_class).or_insert(0.0) += 1.0;

        // Update likelihoods: P(Feature|Action)
        let likelihoods = self
            .likelihoods
            .entry(action_class)
            .or_insert_with(FeatureLikelihoods::new);

        likelihoods.total_samples += 1;

        // Update intent likelihood
        *likelihoods
            .intent_counts
            .entry(features.intent)
            .or_insert(0) += 1;

        // Update keyword likelihoods
        for keyword in &features.keywords {
            *likelihoods
                .keyword_counts
                .entry(keyword.clone())
                .or_insert(0) += 1;
        }

        // Update entity type likelihoods
        for entity in &features.entities {
            let entity_type = EntityType::from_entity(entity);
            *likelihoods
                .entity_type_counts
                .entry(entity_type)
                .or_insert(0) += 1;
        }

        debug!(
            action_class = ?action_class,
            total_samples = self.total_samples,
            "Trained classifier with new sample"
        );
    }

    /// Predict the most likely action class for given features
    ///
    /// Returns the predicted action class and its probability.
    pub fn predict(&self, features: &FeatureVector) -> Option<(ActionClass, f64)> {
        if self.total_samples == 0 {
            return None;
        }

        let mut best_class = None;
        let mut best_log_prob = f64::NEG_INFINITY;

        for (action_class, prior_count) in &self.priors {
            // Calculate log P(Action)
            let log_prior = (prior_count / self.total_samples as f64).ln();

            // Calculate log P(Features|Action)
            let log_likelihood = self.calculate_log_likelihood(features, action_class);

            // Calculate log P(Action|Features) ∝ log P(Action) + log P(Features|Action)
            let log_posterior = log_prior + log_likelihood;

            if log_posterior > best_log_prob {
                best_log_prob = log_posterior;
                best_class = Some(*action_class);
            }
        }

        best_class.map(|class| (class, best_log_prob.exp()))
    }

    /// Calculate log likelihood: log P(Features|Action)
    fn calculate_log_likelihood(&self, features: &FeatureVector, action_class: &ActionClass) -> f64 {
        let likelihoods = match self.likelihoods.get(action_class) {
            Some(l) => l,
            None => return f64::NEG_INFINITY,
        };

        let mut log_likelihood = 0.0;

        // Intent likelihood
        let intent_count = likelihoods
            .intent_counts
            .get(&features.intent)
            .copied()
            .unwrap_or(0);
        let intent_prob = (intent_count as f64 + self.alpha)
            / (likelihoods.total_samples as f64 + self.alpha * 6.0); // 6 intent types
        log_likelihood += intent_prob.ln();

        // Keyword likelihoods
        for keyword in &features.keywords {
            let keyword_count = likelihoods
                .keyword_counts
                .get(keyword)
                .copied()
                .unwrap_or(0);
            let keyword_prob = (keyword_count as f64 + self.alpha)
                / (likelihoods.total_samples as f64 + self.alpha * 1000.0); // Vocabulary size estimate
            log_likelihood += keyword_prob.ln();
        }

        // Entity type likelihoods
        for entity in &features.entities {
            let entity_type = EntityType::from_entity(entity);
            let entity_count = likelihoods
                .entity_type_counts
                .get(&entity_type)
                .copied()
                .unwrap_or(0);
            let entity_prob = (entity_count as f64 + self.alpha)
                / (likelihoods.total_samples as f64 + self.alpha * 4.0); // 4 entity types
            log_likelihood += entity_prob.ln();
        }

        log_likelihood
    }

    /// Get the number of training samples
    pub fn sample_count(&self) -> usize {
        self.total_samples
    }

    /// Clear all training data
    pub fn clear(&mut self) {
        self.priors.clear();
        self.likelihoods.clear();
        self.total_samples = 0;
    }
}

impl Default for NaiveBayesClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::FeatureExtractor;

    #[test]
    fn test_classifier_basic() {
        let mut classifier = NaiveBayesClassifier::new();
        let extractor = FeatureExtractor::new();

        // Train with search examples
        let features1 = extractor.extract("search for TODO");
        classifier.train(&features1, ActionClass::Search);

        let features2 = extractor.extract("find pattern in file");
        classifier.train(&features2, ActionClass::Search);

        // Train with bash example
        let features3 = extractor.extract("run git status");
        classifier.train(&features3, ActionClass::Bash);

        // Predict
        let test_features = extractor.extract("search for FIXME");
        let prediction = classifier.predict(&test_features);

        assert!(prediction.is_some());
        let (predicted_class, _prob) = prediction.unwrap();
        assert_eq!(predicted_class, ActionClass::Search);
    }

    #[test]
    fn test_classifier_incremental_learning() {
        let mut classifier = NaiveBayesClassifier::new();
        let extractor = FeatureExtractor::new();

        // Initially train with bash examples
        for _ in 0..5 {
            let features = extractor.extract("run git status");
            classifier.train(&features, ActionClass::Bash);
        }

        assert_eq!(classifier.sample_count(), 5);

        // Add more training data
        for _ in 0..3 {
            let features = extractor.extract("search for TODO");
            classifier.train(&features, ActionClass::Search);
        }

        assert_eq!(classifier.sample_count(), 8);
    }

    #[test]
    fn test_classifier_clear() {
        let mut classifier = NaiveBayesClassifier::new();
        let extractor = FeatureExtractor::new();

        let features = extractor.extract("search for TODO");
        classifier.train(&features, ActionClass::Search);

        assert_eq!(classifier.sample_count(), 1);

        classifier.clear();
        assert_eq!(classifier.sample_count(), 0);
    }

    #[test]
    fn test_classifier_no_training_data() {
        let classifier = NaiveBayesClassifier::new();
        let extractor = FeatureExtractor::new();

        let features = extractor.extract("search for TODO");
        let prediction = classifier.predict(&features);

        assert!(prediction.is_none());
    }
}
