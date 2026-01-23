//! Quality scoring implementations for ensemble response evaluation
//!
//! This module provides various quality scorers:
//! - LengthScorer: Scores based on response length
//! - StructureScorer: Scores based on markdown structure
//! - LengthAndStructureScorer: Combined scoring
//! - ConfidenceMarkersScorer: Detects confidence language
//! - RelevanceScorer: Word overlap with prompt

use super::types::QualityMetric;
use std::collections::HashSet;

// ============================================================================
// Quality Scorer Trait
// ============================================================================

/// Trait for scoring response quality
pub trait QualityScorer: Send + Sync {
    /// Score a response (0.0 - 1.0, higher is better)
    fn score(&self, response: &str, prompt: &str) -> f64;

    /// Get the metric type this scorer implements
    fn metric(&self) -> QualityMetric;
}

// ============================================================================
// Length Scorer
// ============================================================================

/// Length-based quality scorer
pub struct LengthScorer;

impl QualityScorer for LengthScorer {
    fn score(&self, response: &str, _prompt: &str) -> f64 {
        // Normalize length: 1000 chars = 1.0, capped at 1.0
        (response.len() as f64 / 1000.0).min(1.0)
    }

    fn metric(&self) -> QualityMetric {
        QualityMetric::Length
    }
}

// ============================================================================
// Structure Scorer
// ============================================================================

/// Structure-based quality scorer
pub struct StructureScorer;

impl QualityScorer for StructureScorer {
    fn score(&self, response: &str, _prompt: &str) -> f64 {
        let mut score: f64 = 0.0;

        // Check for code blocks
        if response.contains("```") {
            score += 0.3;
        }

        // Check for bullet lists
        if response.contains("\n- ") || response.contains("\n* ") || response.contains("\n• ") {
            score += 0.25;
        }

        // Check for numbered lists
        if response.contains("\n1.") || response.contains("\n1)") {
            score += 0.2;
        }

        // Check for headers
        if response.contains("\n## ") || response.contains("\n### ") || response.contains("\n# ") {
            score += 0.25;
        }

        // Check for paragraphs (multiple double newlines)
        if response.matches("\n\n").count() >= 2 {
            score += 0.15;
        }

        // Check for emphasis (bold/italic)
        if response.contains("**") || response.contains("__") || response.contains("*") {
            score += 0.1;
        }

        score.min(1.0)
    }

    fn metric(&self) -> QualityMetric {
        QualityMetric::Structure
    }
}

// ============================================================================
// Length and Structure Scorer
// ============================================================================

/// Combined length and structure scorer
pub struct LengthAndStructureScorer {
    length_weight: f64,
    structure_weight: f64,
}

impl Default for LengthAndStructureScorer {
    fn default() -> Self {
        Self {
            length_weight: 0.4,
            structure_weight: 0.6,
        }
    }
}

impl LengthAndStructureScorer {
    /// Create with custom weights
    pub fn new(length_weight: f64, structure_weight: f64) -> Self {
        let total = length_weight + structure_weight;
        Self {
            length_weight: length_weight / total,
            structure_weight: structure_weight / total,
        }
    }
}

impl QualityScorer for LengthAndStructureScorer {
    fn score(&self, response: &str, prompt: &str) -> f64 {
        let length_scorer = LengthScorer;
        let structure_scorer = StructureScorer;

        let length_score = length_scorer.score(response, prompt);
        let structure_score = structure_scorer.score(response, prompt);

        self.length_weight * length_score + self.structure_weight * structure_score
    }

    fn metric(&self) -> QualityMetric {
        QualityMetric::LengthAndStructure
    }
}

// ============================================================================
// Confidence Markers Scorer
// ============================================================================

/// Confidence markers scorer
pub struct ConfidenceMarkersScorer;

impl QualityScorer for ConfidenceMarkersScorer {
    fn score(&self, response: &str, _prompt: &str) -> f64 {
        let response_lower = response.to_lowercase();
        let mut score: f64 = 0.5; // Start neutral

        // Positive confidence markers
        let positive_markers = [
            "i'm confident",
            "i am confident",
            "certainly",
            "definitely",
            "clearly",
            "without a doubt",
            "absolutely",
            "the answer is",
            "this is correct",
        ];

        for marker in positive_markers {
            if response_lower.contains(marker) {
                score += 0.1;
            }
        }

        // Negative/hedging markers
        let negative_markers = [
            "i think",
            "i believe",
            "might be",
            "could be",
            "possibly",
            "perhaps",
            "i'm not sure",
            "i am not sure",
            "uncertain",
            "it depends",
        ];

        for marker in negative_markers {
            if response_lower.contains(marker) {
                score -= 0.05;
            }
        }

        score.clamp(0.0, 1.0)
    }

    fn metric(&self) -> QualityMetric {
        QualityMetric::ConfidenceMarkers
    }
}

// ============================================================================
// Relevance Scorer
// ============================================================================

/// Relevance scorer (based on word overlap with prompt)
pub struct RelevanceScorer;

impl QualityScorer for RelevanceScorer {
    fn score(&self, response: &str, prompt: &str) -> f64 {
        let prompt_words: HashSet<_> = prompt
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .filter(|w| w.len() > 3) // Skip short words
            .collect();

        if prompt_words.is_empty() {
            return 0.5;
        }

        let response_words: HashSet<_> = response
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .filter(|w| w.len() > 3)
            .collect();

        let overlap = prompt_words.intersection(&response_words).count();
        let coverage = overlap as f64 / prompt_words.len() as f64;

        // Scale: 30% coverage = 0.5, 60% coverage = 1.0
        ((coverage - 0.3) / 0.3).clamp(0.0, 1.0)
    }

    fn metric(&self) -> QualityMetric {
        QualityMetric::Relevance
    }
}

// ============================================================================
// Factory Function
// ============================================================================

/// Create a scorer from a QualityMetric
pub fn create_scorer(metric: &QualityMetric) -> Box<dyn QualityScorer> {
    match metric {
        QualityMetric::Length => Box::new(LengthScorer),
        QualityMetric::Structure => Box::new(StructureScorer),
        QualityMetric::LengthAndStructure => Box::new(LengthAndStructureScorer::default()),
        QualityMetric::ConfidenceMarkers => Box::new(ConfidenceMarkersScorer),
        QualityMetric::Relevance => Box::new(RelevanceScorer),
        QualityMetric::Custom(_) => Box::new(LengthAndStructureScorer::default()), // Fallback
    }
}
