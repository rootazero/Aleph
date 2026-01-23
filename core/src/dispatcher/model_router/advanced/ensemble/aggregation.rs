//! Response aggregation for ensemble results
//!
//! This module provides:
//! - EnsembleResult: Final aggregated result from ensemble execution
//! - ResponseAggregator: Combines multiple model responses using various strategies
//! - Similarity calculations (Jaccard)

use super::scorers::{create_scorer, QualityScorer};
use super::types::{ModelExecutionResult, QualityMetric, TokenUsage};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ============================================================================
// Ensemble Result
// ============================================================================

/// Result from ensemble execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleResult {
    /// Final aggregated response
    pub response: String,
    /// Model that produced the selected response
    pub selected_model: String,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// All model results
    pub all_results: Vec<ModelExecutionResult>,
    /// Aggregation method used
    pub aggregation_method: String,
    /// Total cost of ensemble (sum of all models)
    pub total_cost_usd: f64,
    /// Total latency (wall clock time, max of individual latencies)
    pub total_latency_ms: u64,
    /// Consensus level (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consensus_level: Option<f64>,
    /// Number of successful model calls
    pub successful_count: usize,
    /// Number of failed model calls
    pub failed_count: usize,
}

impl EnsembleResult {
    /// Create a result from a single successful response (fallback)
    pub fn from_single(result: ModelExecutionResult) -> Self {
        let response = result.response.clone().unwrap_or_default();
        let model_id = result.model_id.clone();
        let latency = result.latency_ms;
        let cost = result.cost_usd;
        let confidence = result.quality_score.unwrap_or(0.5);
        let success = result.success;

        Self {
            response,
            selected_model: model_id,
            confidence,
            all_results: vec![result],
            aggregation_method: "single".to_string(),
            total_cost_usd: cost,
            total_latency_ms: latency,
            consensus_level: None,
            successful_count: if success { 1 } else { 0 },
            failed_count: if success { 0 } else { 1 },
        }
    }

    /// Create an error result
    pub fn error(_error: impl Into<String>) -> Self {
        Self {
            response: String::new(),
            selected_model: String::new(),
            confidence: 0.0,
            all_results: Vec::new(),
            aggregation_method: "error".to_string(),
            total_cost_usd: 0.0,
            total_latency_ms: 0,
            consensus_level: None,
            successful_count: 0,
            failed_count: 0,
        }
    }

    /// Check if this is a successful result
    pub fn is_success(&self) -> bool {
        !self.response.is_empty() && self.successful_count > 0
    }

    /// Get total tokens used
    pub fn total_tokens(&self) -> TokenUsage {
        let input: u32 = self.all_results.iter().map(|r| r.tokens.input_tokens).sum();
        let output: u32 = self
            .all_results
            .iter()
            .map(|r| r.tokens.output_tokens)
            .sum();
        TokenUsage::new(input, output)
    }
}

// ============================================================================
// Response Aggregator
// ============================================================================

/// Aggregates multiple model responses into a single result
pub struct ResponseAggregator {
    scorer: Box<dyn QualityScorer>,
    consensus_threshold: f64,
}

impl ResponseAggregator {
    /// Create a new response aggregator
    pub fn new(metric: &QualityMetric) -> Self {
        Self {
            scorer: create_scorer(metric),
            consensus_threshold: 0.7,
        }
    }

    /// Set consensus threshold
    pub fn with_consensus_threshold(mut self, threshold: f64) -> Self {
        self.consensus_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Score all results
    pub fn score_results(&self, results: &mut [ModelExecutionResult], prompt: &str) {
        for result in results.iter_mut() {
            if let Some(response) = &result.response {
                result.quality_score = Some(self.scorer.score(response, prompt));
            }
        }
    }

    /// Aggregate using best-of-n strategy
    pub fn best_of_n(
        &self,
        mut results: Vec<ModelExecutionResult>,
        prompt: &str,
    ) -> EnsembleResult {
        // Score all successful results
        self.score_results(&mut results, prompt);

        let successful: Vec<_> = results.iter().filter(|r| r.has_response()).collect();

        if successful.is_empty() {
            return self.fallback_error(&results);
        }

        // Find best by quality score
        let best = successful
            .iter()
            .max_by(|a, b| {
                a.quality_score
                    .unwrap_or(0.0)
                    .partial_cmp(&b.quality_score.unwrap_or(0.0))
                    .unwrap()
            })
            .unwrap();

        let total_cost: f64 = results.iter().map(|r| r.cost_usd).sum();
        let max_latency = results.iter().map(|r| r.latency_ms).max().unwrap_or(0);
        let successful_count = successful.len();
        let failed_count = results.len() - successful_count;

        EnsembleResult {
            response: best.response.clone().unwrap(),
            selected_model: best.model_id.clone(),
            confidence: best.quality_score.unwrap_or(0.5),
            all_results: results,
            aggregation_method: "best_of_n".to_string(),
            total_cost_usd: total_cost,
            total_latency_ms: max_latency,
            consensus_level: None,
            successful_count,
            failed_count,
        }
    }

    /// Aggregate using consensus detection
    pub fn consensus(
        &self,
        mut results: Vec<ModelExecutionResult>,
        prompt: &str,
    ) -> EnsembleResult {
        self.score_results(&mut results, prompt);

        let successful: Vec<_> = results.iter().filter(|r| r.has_response()).collect();

        if successful.len() < 2 {
            return self.best_of_n(results, prompt);
        }

        // Calculate pairwise similarity
        let similarities = self.calculate_similarities(&successful);
        let consensus_level = if similarities.is_empty() {
            0.0
        } else {
            similarities.iter().sum::<f64>() / similarities.len() as f64
        };

        // Get best result first
        let mut result = self.best_of_n(results, prompt);
        result.consensus_level = Some(consensus_level);
        result.aggregation_method = "consensus".to_string();

        // Adjust confidence based on consensus
        if consensus_level < self.consensus_threshold {
            result.confidence *= 0.7; // Reduce confidence for low consensus
        }

        result
    }

    /// Aggregate using voting
    pub fn voting(&self, mut results: Vec<ModelExecutionResult>, prompt: &str) -> EnsembleResult {
        self.score_results(&mut results, prompt);

        let successful: Vec<_> = results.iter().filter(|r| r.has_response()).collect();

        if successful.len() < 3 {
            return self.best_of_n(results, prompt);
        }

        // Group responses by similarity
        let groups = self.group_by_similarity(&successful);

        // Find largest group
        let largest_group = groups.iter().max_by_key(|g| g.len()).unwrap();

        // Select best from largest group
        let best = largest_group
            .iter()
            .max_by(|a, b| {
                a.quality_score
                    .unwrap_or(0.0)
                    .partial_cmp(&b.quality_score.unwrap_or(0.0))
                    .unwrap()
            })
            .unwrap();

        let total_cost: f64 = results.iter().map(|r| r.cost_usd).sum();
        let max_latency = results.iter().map(|r| r.latency_ms).max().unwrap_or(0);
        let vote_confidence = largest_group.len() as f64 / successful.len() as f64;
        let successful_count = successful.len();
        let total_count = results.len();

        EnsembleResult {
            response: best.response.clone().unwrap(),
            selected_model: best.model_id.clone(),
            confidence: vote_confidence * best.quality_score.unwrap_or(0.5),
            all_results: results,
            aggregation_method: "voting".to_string(),
            total_cost_usd: total_cost,
            total_latency_ms: max_latency,
            consensus_level: Some(vote_confidence),
            successful_count,
            failed_count: total_count - successful_count,
        }
    }

    /// Calculate pairwise Jaccard similarity between responses
    fn calculate_similarities(&self, results: &[&ModelExecutionResult]) -> Vec<f64> {
        let mut similarities = Vec::new();

        for i in 0..results.len() {
            for j in (i + 1)..results.len() {
                if let (Some(r1), Some(r2)) = (&results[i].response, &results[j].response) {
                    similarities.push(jaccard_similarity(r1, r2));
                }
            }
        }

        similarities
    }

    /// Group responses by similarity
    fn group_by_similarity<'a>(
        &self,
        results: &[&'a ModelExecutionResult],
    ) -> Vec<Vec<&'a ModelExecutionResult>> {
        if results.is_empty() {
            return Vec::new();
        }

        let mut groups: Vec<Vec<&ModelExecutionResult>> = Vec::new();

        for result in results {
            let mut found_group = false;

            for group in &mut groups {
                // Check similarity with first member of group
                if let (Some(r1), Some(r2)) = (&group[0].response, &result.response) {
                    if jaccard_similarity(r1, r2) >= self.consensus_threshold {
                        group.push(result);
                        found_group = true;
                        break;
                    }
                }
            }

            if !found_group {
                groups.push(vec![result]);
            }
        }

        groups
    }

    /// Create error result when all models failed
    fn fallback_error(&self, results: &[ModelExecutionResult]) -> EnsembleResult {
        let total_cost: f64 = results.iter().map(|r| r.cost_usd).sum();
        let max_latency = results.iter().map(|r| r.latency_ms).max().unwrap_or(0);

        let error_msg = results
            .iter()
            .filter_map(|r| r.error.as_ref())
            .next()
            .cloned()
            .unwrap_or_else(|| "All models failed".to_string());

        EnsembleResult {
            response: format!("Error: {}", error_msg),
            selected_model: String::new(),
            confidence: 0.0,
            all_results: results.to_vec(),
            aggregation_method: "fallback_error".to_string(),
            total_cost_usd: total_cost,
            total_latency_ms: max_latency,
            consensus_level: None,
            successful_count: 0,
            failed_count: results.len(),
        }
    }
}

// ============================================================================
// Similarity Functions
// ============================================================================

/// Calculate Jaccard similarity between two strings (word-based)
pub fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let words_a: HashSet<_> = a
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() > 2)
        .collect();

    let words_b: HashSet<_> = b
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() > 2)
        .collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}
