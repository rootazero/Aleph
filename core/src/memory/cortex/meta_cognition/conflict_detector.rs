//! Semantic conflict detection for behavioral anchors
//!
//! This module detects when new behavioral anchors conflict with existing ones
//! using semantic similarity analysis. It helps prevent redundant or contradictory
//! rules from accumulating in the system.

use crate::error::AlephError;
use crate::memory::smart_embedder::SmartEmbedder;
use std::sync::Arc;

use super::types::BehavioralAnchor;

/// Type of conflict detected between behavioral anchors
#[derive(Debug, Clone, PartialEq)]
pub enum ConflictType {
    /// Anchors are semantically identical (similarity > 0.85)
    Redundant,
    /// Anchors are very similar and may need human review (similarity > 0.70)
    NeedsReview,
    /// Anchors logically contradict each other
    LogicalContradiction,
    /// Anchors have conflicting empirical evidence
    EmpiricalConflict,
}

/// Report of a detected conflict between anchors
#[derive(Debug, Clone)]
pub struct ConflictReport {
    /// ID of the existing anchor that conflicts
    pub existing_anchor_id: String,
    /// Type of conflict detected
    pub conflict_type: ConflictType,
    /// Semantic similarity score (0.0-1.0)
    pub similarity_score: f32,
    /// Human-readable explanation of the conflict
    pub explanation: String,
}

/// Detector for semantic conflicts between behavioral anchors
pub struct ConflictDetector {
    /// Embedding model for semantic similarity
    embedder: Arc<SmartEmbedder>,
}

impl ConflictDetector {
    /// Create a new conflict detector with the given embedder
    pub fn new(embedder: Arc<SmartEmbedder>) -> Self {
        Self { embedder }
    }

    /// Detect semantic conflicts between a new anchor and existing anchors
    ///
    /// This method performs semantic similarity analysis to identify:
    /// - Redundant anchors (similarity > 0.85)
    /// - Anchors needing review (similarity > 0.70)
    ///
    /// # Arguments
    ///
    /// * `new_anchor` - The new behavioral anchor to check
    /// * `existing_anchors` - List of existing anchors to check against
    ///
    /// # Returns
    ///
    /// A vector of conflict reports, sorted by similarity score (highest first)
    pub async fn detect_semantic_conflicts(
        &self,
        new_anchor: &BehavioralAnchor,
        existing_anchors: &[BehavioralAnchor],
    ) -> Result<Vec<ConflictReport>, AlephError> {
        let mut conflicts = Vec::new();

        // Embed the new anchor's rule text
        let new_embedding = self.embedder.embed(&new_anchor.rule_text).await?;

        for existing in existing_anchors {
            // Skip if anchor is comparing with itself
            if existing.id == new_anchor.id {
                continue;
            }

            // Optimization: only check conflicts if tags overlap
            if !has_tag_overlap(&new_anchor.trigger_tags, &existing.trigger_tags) {
                continue;
            }

            // Compute semantic similarity
            let existing_embedding = self.embedder.embed(&existing.rule_text).await?;
            let similarity = cosine_similarity(&new_embedding, &existing_embedding);

            // Classify conflict type based on similarity threshold
            let conflict_type = if similarity > 0.85 {
                Some(ConflictType::Redundant)
            } else if similarity > 0.70 {
                Some(ConflictType::NeedsReview)
            } else {
                None
            };

            if let Some(conflict_type) = conflict_type {
                let explanation = match conflict_type {
                    ConflictType::Redundant => {
                        format!(
                            "Anchor '{}' is semantically identical to existing anchor '{}'",
                            new_anchor.id, existing.id
                        )
                    }
                    ConflictType::NeedsReview => {
                        format!(
                            "Anchor '{}' is very similar to existing anchor '{}' and may need review",
                            new_anchor.id, existing.id
                        )
                    }
                    ConflictType::LogicalContradiction => {
                        format!(
                            "Anchor '{}' logically contradicts existing anchor '{}'",
                            new_anchor.id, existing.id
                        )
                    }
                    ConflictType::EmpiricalConflict => {
                        format!(
                            "Anchor '{}' has conflicting empirical evidence with existing anchor '{}'",
                            new_anchor.id, existing.id
                        )
                    }
                };

                conflicts.push(ConflictReport {
                    existing_anchor_id: existing.id.clone(),
                    conflict_type,
                    similarity_score: similarity,
                    explanation,
                });
            }
        }

        // Sort by similarity score (highest first)
        conflicts.sort_by(|a, b| {
            b.similarity_score
                .partial_cmp(&a.similarity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(conflicts)
    }
}

/// Check if two tag lists have any overlap
///
/// Returns true if at least one tag appears in both lists.
fn has_tag_overlap(tags_a: &[String], tags_b: &[String]) -> bool {
    tags_a.iter().any(|tag| tags_b.contains(tag))
}

/// Compute cosine similarity between two embedding vectors
///
/// Returns a value between -1.0 and 1.0, where:
/// - 1.0 means identical vectors
/// - 0.0 means orthogonal vectors
/// - -1.0 means opposite vectors
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let magnitude_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let magnitude_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if magnitude_a == 0.0 || magnitude_b == 0.0 {
        return 0.0;
    }

    dot_product / (magnitude_a * magnitude_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        // Identical vectors
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        // Orthogonal vectors
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 0.001);

        // Opposite vectors
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 0.001);

        // Similar vectors
        let a = vec![1.0, 1.0, 0.0];
        let b = vec![1.0, 0.9, 0.0];
        let similarity = cosine_similarity(&a, &b);
        assert!(similarity > 0.9 && similarity < 1.0);

        // Different length vectors
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);

        // Zero vectors
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_has_tag_overlap() {
        // Complete overlap
        let tags_a = vec!["Python".to_string(), "macOS".to_string()];
        let tags_b = vec!["Python".to_string(), "Linux".to_string()];
        assert!(has_tag_overlap(&tags_a, &tags_b));

        // No overlap
        let tags_a = vec!["Python".to_string(), "macOS".to_string()];
        let tags_b = vec!["Rust".to_string(), "Linux".to_string()];
        assert!(!has_tag_overlap(&tags_a, &tags_b));

        // Empty lists
        let tags_a = vec![];
        let tags_b = vec!["Python".to_string()];
        assert!(!has_tag_overlap(&tags_a, &tags_b));

        // Both empty
        let tags_a: Vec<String> = vec![];
        let tags_b: Vec<String> = vec![];
        assert!(!has_tag_overlap(&tags_a, &tags_b));

        // Multiple overlaps
        let tags_a = vec!["Python".to_string(), "macOS".to_string(), "shell".to_string()];
        let tags_b = vec!["Python".to_string(), "macOS".to_string()];
        assert!(has_tag_overlap(&tags_a, &tags_b));
    }
}
