//! Experience clustering and deduplication
//!
//! This module implements clustering logic to group similar experiences
//! and prevent the experience database from growing unbounded.

use crate::error::{AlephError, Result};
use crate::memory::cortex::{EvolutionStatus, Experience};
use crate::memory::store::MemoryBackend;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Configuration for clustering service
#[derive(Debug, Clone)]
pub struct ClusteringConfig {
    /// Minimum similarity threshold for clustering (0.0-1.0)
    pub similarity_threshold: f64,
    /// Minimum cluster size to consider for merging
    pub min_cluster_size: usize,
    /// Enable clustering
    pub enabled: bool,
}

impl Default for ClusteringConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.95, // Very high threshold for deduplication
            min_cluster_size: 2,
            enabled: true,
        }
    }
}

/// Cluster metadata
#[derive(Debug, Clone)]
pub struct Cluster {
    /// Cluster ID (hash of representative experience)
    pub cluster_id: String,
    /// Member experience IDs
    pub members: Vec<String>,
    /// Representative experience (highest value)
    pub representative_id: String,
}

/// Clustering service for grouping similar experiences
pub struct ClusteringService {
    db: MemoryBackend,
    config: ClusteringConfig,
}

impl ClusteringService {
    /// Create a new clustering service
    pub fn new(db: MemoryBackend, config: ClusteringConfig) -> Self {
        Self { db, config }
    }

    /// Run clustering on candidate experiences
    pub async fn cluster_experiences(&self) -> Result<Vec<Cluster>> {
        if !self.config.enabled {
            debug!("Clustering disabled");
            return Ok(Vec::new());
        }

        info!("Starting experience clustering");

        // Step 1: Get all verified/distilled experiences
        let experiences = self.get_clusterable_experiences().await?;

        if experiences.is_empty() {
            debug!("No experiences to cluster");
            return Ok(Vec::new());
        }

        info!("Found {} experiences to cluster", experiences.len());

        // Step 2: Group by pattern_hash (fast deduplication)
        let hash_groups = self.group_by_pattern_hash(&experiences);

        info!("Grouped into {} hash-based clusters", hash_groups.len());

        // Step 3: For each hash group, perform vector-based clustering
        let mut all_clusters = Vec::new();
        for (pattern_hash, group_experiences) in hash_groups {
            if group_experiences.len() < self.config.min_cluster_size {
                continue;
            }

            let clusters = self
                .cluster_by_vector_similarity(&pattern_hash, group_experiences)
                .await?;
            all_clusters.extend(clusters);
        }

        info!("Created {} clusters total", all_clusters.len());

        // Step 4: Merge duplicates within each cluster
        for cluster in &all_clusters {
            if let Err(e) = self.merge_cluster_members(cluster).await {
                warn!("Failed to merge cluster {}: {}", cluster.cluster_id, e);
            }
        }

        Ok(all_clusters)
    }

    /// Get experiences that can be clustered
    async fn get_clusterable_experiences(&self) -> Result<Vec<Experience>> {
        // TODO: Implement experience queries via new store API
        // The experience table is not yet part of the LanceDB store traits.
        // Old code: db.query_experiences_by_status(EvolutionStatus::Verified/Distilled, 1000)
        Ok(Vec::new())
    }

    /// Group experiences by pattern_hash
    fn group_by_pattern_hash(
        &self,
        experiences: &[Experience],
    ) -> HashMap<String, Vec<Experience>> {
        let mut groups: HashMap<String, Vec<Experience>> = HashMap::new();

        for exp in experiences {
            groups
                .entry(exp.pattern_hash.clone())
                .or_default()
                .push(exp.clone());
        }

        groups
    }

    /// Cluster experiences within a hash group using vector similarity
    async fn cluster_by_vector_similarity(
        &self,
        pattern_hash: &str,
        experiences: Vec<Experience>,
    ) -> Result<Vec<Cluster>> {
        let mut clusters = Vec::new();
        let mut unclustered: Vec<Experience> = experiences;

        while !unclustered.is_empty() {
            // Take the first experience as cluster seed
            let seed = unclustered.remove(0);

            // Find similar experiences
            let mut cluster_members = vec![seed.clone()];
            let mut remaining = Vec::new();

            for exp in unclustered {
                if self.are_similar(&seed, &exp).await? {
                    cluster_members.push(exp);
                } else {
                    remaining.push(exp);
                }
            }

            unclustered = remaining;

            // Create cluster if we have enough members
            if cluster_members.len() >= self.config.min_cluster_size {
                // Find representative (highest value)
                let representative = self.find_representative(&cluster_members)?;

                let cluster = Cluster {
                    cluster_id: format!("{}_{}", pattern_hash, representative.id),
                    members: cluster_members.iter().map(|e| e.id.clone()).collect(),
                    representative_id: representative.id.clone(),
                };

                clusters.push(cluster);
            }
        }

        Ok(clusters)
    }

    /// Check if two experiences are similar
    async fn are_similar(&self, exp1: &Experience, exp2: &Experience) -> Result<bool> {
        // If both have intent vectors, use cosine similarity
        if let (Some(ref vec1), Some(ref vec2)) = (&exp1.intent_vector, &exp2.intent_vector) {
            let similarity = self.cosine_similarity(vec1, vec2);
            Ok(similarity >= self.config.similarity_threshold)
        } else {
            // Fallback to pattern_hash comparison
            Ok(exp1.pattern_hash == exp2.pattern_hash)
        }
    }

    /// Calculate cosine similarity between two vectors
    fn cosine_similarity(&self, vec1: &[f32], vec2: &[f32]) -> f64 {
        if vec1.len() != vec2.len() {
            return 0.0;
        }

        let dot_product: f32 = vec1.iter().zip(vec2.iter()).map(|(a, b)| a * b).sum();

        let norm1: f32 = vec1.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm2: f32 = vec2.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm1 == 0.0 || norm2 == 0.0 {
            return 0.0;
        }

        (dot_product / (norm1 * norm2)) as f64
    }

    /// Find the representative experience (highest value)
    fn find_representative<'a>(&self, experiences: &'a [Experience]) -> Result<&'a Experience> {
        experiences
            .iter()
            .max_by(|a, b| {
                a.success_score
                    .partial_cmp(&b.success_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .ok_or_else(|| AlephError::Other {
                message: "No experiences in cluster".to_string(),
                suggestion: None,
            })
    }

    /// Merge cluster members into representative
    async fn merge_cluster_members(&self, cluster: &Cluster) -> Result<()> {
        if cluster.members.len() < 2 {
            return Ok(());
        }

        info!(
            "Merging {} experiences into representative {}",
            cluster.members.len(),
            cluster.representative_id
        );

        // TODO: Implement experience CRUD via new store API
        // Old code used: db.get_experience(), db.delete_experience()
        debug!(
            "Merge skipped: experience store not yet migrated to LanceDB",
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::cortex::ExperienceBuilder;
    use tempfile::TempDir;

    async fn create_test_db() -> (MemoryBackend, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let backend = crate::memory::store::lance::LanceMemoryBackend::open_or_create(temp_dir.path()).await.unwrap();
        (Arc::new(backend), temp_dir)
    }

    #[tokio::test]
    async fn test_cosine_similarity() {
        let (db, _temp) = create_test_db().await;
        let config = ClusteringConfig::default();
        let service = ClusteringService::new(db, config);

        // Identical vectors
        let vec1 = vec![1.0, 0.0, 0.0];
        let vec2 = vec![1.0, 0.0, 0.0];
        assert!((service.cosine_similarity(&vec1, &vec2) - 1.0).abs() < 0.001);

        // Orthogonal vectors
        let vec3 = vec![1.0, 0.0, 0.0];
        let vec4 = vec![0.0, 1.0, 0.0];
        assert!((service.cosine_similarity(&vec3, &vec4) - 0.0).abs() < 0.001);

        // Similar vectors
        let vec5 = vec![1.0, 0.0, 0.0];
        let vec6 = vec![0.9, 0.1, 0.0];
        let sim = service.cosine_similarity(&vec5, &vec6);
        assert!(sim > 0.9);
    }

    #[tokio::test]
    async fn test_group_by_pattern_hash() {
        let (db, _temp) = create_test_db().await;
        let config = ClusteringConfig::default();
        let service = ClusteringService::new(db, config);

        let exp1 = ExperienceBuilder::new(
            "exp1".to_string(),
            "intent1".to_string(),
            "{}".to_string(),
        )
        .pattern_hash("hash1".to_string())
        .build();

        let exp2 = ExperienceBuilder::new(
            "exp2".to_string(),
            "intent2".to_string(),
            "{}".to_string(),
        )
        .pattern_hash("hash1".to_string())
        .build();

        let exp3 = ExperienceBuilder::new(
            "exp3".to_string(),
            "intent3".to_string(),
            "{}".to_string(),
        )
        .pattern_hash("hash2".to_string())
        .build();

        let experiences = vec![exp1, exp2, exp3];
        let groups = service.group_by_pattern_hash(&experiences);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups.get("hash1").unwrap().len(), 2);
        assert_eq!(groups.get("hash2").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_find_representative() {
        let (db, _temp) = create_test_db().await;
        let config = ClusteringConfig::default();
        let service = ClusteringService::new(db, config);

        let exp1 = ExperienceBuilder::new(
            "exp1".to_string(),
            "intent1".to_string(),
            "{}".to_string(),
        )
        .success_score(0.8)
        .build();

        let exp2 = ExperienceBuilder::new(
            "exp2".to_string(),
            "intent2".to_string(),
            "{}".to_string(),
        )
        .success_score(0.95)
        .build();

        let exp3 = ExperienceBuilder::new(
            "exp3".to_string(),
            "intent3".to_string(),
            "{}".to_string(),
        )
        .success_score(0.7)
        .build();

        let experiences = vec![exp1, exp2.clone(), exp3];
        let representative = service.find_representative(&experiences).unwrap();

        assert_eq!(representative.id, exp2.id);
        assert_eq!(representative.success_score, 0.95);
    }

    #[test]
    fn test_config_default() {
        let config = ClusteringConfig::default();
        assert_eq!(config.similarity_threshold, 0.95);
        assert_eq!(config.min_cluster_size, 2);
        assert!(config.enabled);
    }
}
