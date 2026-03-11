//! Experience store -- trait and implementations for POE experience persistence.
//!
//! `ExperienceStore` provides async storage for crystallized POE experiences,
//! enabling vector-based similarity search for experience replay.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::error::AlephError;

/// A stored experience entry with its embedding vector.
type ExperienceEntry = (PoeExperience, Vec<f32>);

// ============================================================================
// PoeExperience -- crystallized execution experience
// ============================================================================

/// A crystallized POE execution experience.
///
/// Records the outcome of a complete P->O->E cycle along with metadata
/// for future pattern matching and experience replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoeExperience {
    /// Unique experience ID
    pub id: String,
    /// ID of the task that generated this experience
    pub task_id: String,
    /// The task objective
    pub objective: String,
    /// Pattern ID for grouping similar tasks (e.g., "poe-create-rust-file")
    pub pattern_id: String,
    /// JSON-encoded tool/action sequence used
    pub tool_sequence_json: String,
    /// Optional JSON-encoded parameter mapping
    pub parameter_mapping: Option<String>,
    /// Satisfaction score (0.0-1.0)
    pub satisfaction: f32,
    /// Distance score from validation (0.0 = perfect)
    pub distance_score: f32,
    /// Number of attempts used
    pub attempts: u8,
    /// Total execution duration in milliseconds
    pub duration_ms: u64,
    /// When this experience was created (Unix timestamp ms)
    pub created_at: i64,
}

// ============================================================================
// ExperienceStore trait
// ============================================================================

/// Async trait for POE experience storage.
///
/// Provides CRUD operations and vector-based similarity search for
/// experience replay. Implementations must be Send + Sync for use
/// in async contexts.
#[async_trait]
pub trait ExperienceStore: Send + Sync {
    /// Insert a new experience with its embedding vector.
    ///
    /// # Arguments
    /// * `experience` - The experience record to store
    /// * `embedding` - The embedding vector for similarity search
    async fn insert(
        &self,
        experience: PoeExperience,
        embedding: &[f32],
    ) -> Result<(), AlephError>;

    /// Search for similar experiences using vector similarity.
    ///
    /// Returns experiences with similarity above the threshold, ordered
    /// by similarity (highest first).
    ///
    /// # Arguments
    /// * `query_embedding` - The query embedding vector
    /// * `limit` - Maximum number of results
    /// * `min_similarity` - Minimum similarity threshold (0.0-1.0)
    ///
    /// # Returns
    /// Vec of (experience, similarity_score) tuples
    async fn vector_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        min_similarity: f64,
    ) -> Result<Vec<(PoeExperience, f64)>, AlephError>;

    /// Get all experiences for a given pattern ID.
    async fn get_by_pattern_id(
        &self,
        pattern_id: &str,
    ) -> Result<Vec<PoeExperience>, AlephError>;

    /// Count total experiences.
    async fn count(&self) -> Result<usize, AlephError>;

    /// Delete an experience by ID.
    ///
    /// Returns `true` if an experience was removed, `false` if not found.
    async fn delete(&self, experience_id: &str) -> Result<bool, AlephError>;

    /// Retrieve experiences matching any of the given IDs.
    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<PoeExperience>, AlephError>;
}

// ============================================================================
// InMemoryExperienceStore -- for testing
// ============================================================================

/// In-memory implementation of ExperienceStore for testing.
///
/// Stores experiences and embeddings in memory. Vector search uses
/// cosine similarity.
pub struct InMemoryExperienceStore {
    entries: Arc<RwLock<Vec<ExperienceEntry>>>,
}

impl InMemoryExperienceStore {
    /// Create a new empty in-memory store.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for InMemoryExperienceStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| *x as f64 * *y as f64)
        .sum();
    let norm_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[async_trait]
impl ExperienceStore for InMemoryExperienceStore {
    async fn insert(
        &self,
        experience: PoeExperience,
        embedding: &[f32],
    ) -> Result<(), AlephError> {
        let mut entries = self.entries.write().await;
        entries.push((experience, embedding.to_vec()));
        Ok(())
    }

    async fn vector_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        min_similarity: f64,
    ) -> Result<Vec<(PoeExperience, f64)>, AlephError> {
        let entries = self.entries.read().await;
        let mut results: Vec<(PoeExperience, f64)> = entries
            .iter()
            .map(|(exp, emb)| {
                let sim = cosine_similarity(query_embedding, emb);
                (exp.clone(), sim)
            })
            .filter(|(_, sim)| *sim >= min_similarity)
            .collect();

        // Sort by similarity descending
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        Ok(results)
    }

    async fn get_by_pattern_id(
        &self,
        pattern_id: &str,
    ) -> Result<Vec<PoeExperience>, AlephError> {
        let entries = self.entries.read().await;
        Ok(entries
            .iter()
            .filter(|(exp, _)| exp.pattern_id == pattern_id)
            .map(|(exp, _)| exp.clone())
            .collect())
    }

    async fn count(&self) -> Result<usize, AlephError> {
        let entries = self.entries.read().await;
        Ok(entries.len())
    }

    async fn delete(&self, experience_id: &str) -> Result<bool, AlephError> {
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|(exp, _)| exp.id != experience_id);
        Ok(entries.len() < before)
    }

    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<PoeExperience>, AlephError> {
        let entries = self.entries.read().await;
        Ok(entries
            .iter()
            .filter(|(exp, _)| ids.contains(&exp.id))
            .map(|(exp, _)| exp.clone())
            .collect())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_experience(id: &str, pattern: &str, satisfaction: f32) -> PoeExperience {
        PoeExperience {
            id: id.into(),
            task_id: format!("task-{}", id),
            objective: format!("Test objective for {}", id),
            pattern_id: pattern.into(),
            tool_sequence_json: "[]".into(),
            parameter_mapping: None,
            satisfaction,
            distance_score: 1.0 - satisfaction,
            attempts: 1,
            duration_ms: 1000,
            created_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    #[tokio::test]
    async fn test_insert_and_count() {
        let store = InMemoryExperienceStore::new();
        assert_eq!(store.count().await.unwrap(), 0);

        let exp = make_experience("1", "poe-test", 0.9);
        store.insert(exp, &[1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_get_by_pattern_id() {
        let store = InMemoryExperienceStore::new();

        store
            .insert(make_experience("1", "poe-auth", 0.9), &[1.0, 0.0])
            .await
            .unwrap();
        store
            .insert(make_experience("2", "poe-auth", 0.7), &[0.0, 1.0])
            .await
            .unwrap();
        store
            .insert(make_experience("3", "poe-other", 0.5), &[0.5, 0.5])
            .await
            .unwrap();

        let results = store.get_by_pattern_id("poe-auth").await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.pattern_id == "poe-auth"));
    }

    #[tokio::test]
    async fn test_vector_search() {
        let store = InMemoryExperienceStore::new();

        // Insert experiences with known embeddings
        store
            .insert(make_experience("1", "p1", 0.9), &[1.0, 0.0, 0.0])
            .await
            .unwrap();
        store
            .insert(make_experience("2", "p2", 0.7), &[0.0, 1.0, 0.0])
            .await
            .unwrap();
        store
            .insert(make_experience("3", "p3", 0.5), &[0.9, 0.1, 0.0])
            .await
            .unwrap();

        // Search with query close to first embedding
        let results = store
            .vector_search(&[1.0, 0.0, 0.0], 5, 0.5)
            .await
            .unwrap();

        assert!(!results.is_empty());
        // First result should be the most similar (exact match)
        assert_eq!(results[0].0.id, "1");
        assert!((results[0].1 - 1.0).abs() < 0.01); // cosine similarity ~1.0
    }

    #[tokio::test]
    async fn test_vector_search_respects_threshold() {
        let store = InMemoryExperienceStore::new();

        store
            .insert(make_experience("1", "p1", 0.9), &[1.0, 0.0])
            .await
            .unwrap();
        store
            .insert(make_experience("2", "p2", 0.7), &[0.0, 1.0])
            .await
            .unwrap();

        // High threshold should filter out dissimilar
        let results = store.vector_search(&[1.0, 0.0], 5, 0.9).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.id, "1");
    }

    #[tokio::test]
    async fn test_vector_search_respects_limit() {
        let store = InMemoryExperienceStore::new();

        for i in 0..10 {
            store
                .insert(make_experience(&i.to_string(), "p1", 0.9), &[1.0, 0.0])
                .await
                .unwrap();
        }

        let results = store.vector_search(&[1.0, 0.0], 3, 0.0).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[tokio::test]
    async fn test_delete_experience() {
        let store = InMemoryExperienceStore::new();

        store.insert(make_experience("1", "p1", 0.9), &[1.0, 0.0]).await.unwrap();
        store.insert(make_experience("2", "p1", 0.8), &[0.0, 1.0]).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 2);

        // Delete existing
        let removed = store.delete("1").await.unwrap();
        assert!(removed);
        assert_eq!(store.count().await.unwrap(), 1);

        // Delete non-existent
        let removed = store.delete("999").await.unwrap();
        assert!(!removed);
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_get_by_ids() {
        let store = InMemoryExperienceStore::new();

        store.insert(make_experience("a", "p1", 0.9), &[1.0]).await.unwrap();
        store.insert(make_experience("b", "p1", 0.8), &[0.5]).await.unwrap();
        store.insert(make_experience("c", "p2", 0.7), &[0.0]).await.unwrap();

        let results = store
            .get_by_ids(&["a".into(), "c".into()])
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        let ids: Vec<&str> = results.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"a"));
        assert!(ids.contains(&"c"));

        // Empty ids list
        let results = store.get_by_ids(&[]).await.unwrap();
        assert!(results.is_empty());

        // Non-existent ids
        let results = store.get_by_ids(&["zzz".into()]).await.unwrap();
        assert!(results.is_empty());
    }
}
