//! Dynamic Association Clustering
//!
//! Finds related facts at query time without pre-stored clusters.
//! This implements the "dynamic clustering at query time" approach
//! from the Memory v2 design document.

use crate::memory::context::MemoryFact;
use serde::{Deserialize, Serialize};

/// Association cluster result
///
/// Represents a cluster of related facts with a center fact
/// and its associations.
#[derive(Debug, Clone)]
pub struct AssociationCluster {
    /// Cluster center (most relevant fact)
    pub center_fact: MemoryFact,
    /// Related facts in the cluster
    pub related_facts: Vec<MemoryFact>,
    /// LLM-generated theme label (optional)
    pub cluster_theme: Option<String>,
    /// Average similarity within cluster
    pub avg_similarity: f32,
}

impl AssociationCluster {
    /// Create a new cluster with just a center fact
    pub fn new(center: MemoryFact) -> Self {
        Self {
            center_fact: center,
            related_facts: Vec::new(),
            cluster_theme: None,
            avg_similarity: 1.0,
        }
    }

    /// Add a related fact to the cluster
    pub fn add_related(&mut self, fact: MemoryFact, similarity: f32) {
        self.related_facts.push(fact);
        // Update average similarity
        let total = self.related_facts.len() as f32;
        self.avg_similarity = (self.avg_similarity * (total - 1.0) + similarity) / total;
    }

    /// Set the cluster theme
    pub fn with_theme(mut self, theme: String) -> Self {
        self.cluster_theme = Some(theme);
        self
    }

    /// Get the total size of the cluster (center + related)
    pub fn size(&self) -> usize {
        1 + self.related_facts.len()
    }
}

/// Association retriever configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssociationConfig {
    /// Vector space expansion radius (default: 0.4)
    /// Facts within this distance are considered related
    pub expansion_radius: f32,
    /// Maximum associations to return (default: 5)
    pub max_associations: usize,
    /// Minimum cluster size to include (default: 2)
    pub min_cluster_size: usize,
    /// Whether to generate theme labels (default: false)
    pub generate_theme: bool,
}

impl Default for AssociationConfig {
    fn default() -> Self {
        Self {
            expansion_radius: 0.4,
            max_associations: 5,
            min_cluster_size: 2,
            generate_theme: false,
        }
    }
}

impl AssociationConfig {
    /// Create config with custom expansion radius
    pub fn with_radius(mut self, radius: f32) -> Self {
        self.expansion_radius = radius;
        self
    }

    /// Create config with custom max associations
    pub fn with_max_associations(mut self, max: usize) -> Self {
        self.max_associations = max;
        self
    }

    /// Enable theme generation
    pub fn with_theme_generation(mut self) -> Self {
        self.generate_theme = true;
        self
    }
}

/// Dynamic association retriever
///
/// Finds related facts at query time using vector proximity.
pub struct AssociationRetriever {
    config: AssociationConfig,
}

impl AssociationRetriever {
    /// Create a new association retriever
    pub fn new(config: AssociationConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(AssociationConfig::default())
    }

    /// Get current configuration
    pub fn config(&self) -> &AssociationConfig {
        &self.config
    }

    /// Find associations for a list of facts
    ///
    /// Groups facts into clusters based on vector similarity.
    /// Returns clusters that meet the minimum size requirement.
    pub fn find_associations(&self, facts: &[MemoryFact]) -> Vec<AssociationCluster> {
        if facts.is_empty() {
            return Vec::new();
        }

        let mut clusters = Vec::new();
        let mut used = vec![false; facts.len()];

        // For each fact, try to form a cluster
        for (i, center) in facts.iter().enumerate() {
            if used[i] {
                continue;
            }

            let center_embedding = match &center.embedding {
                Some(e) => e,
                None => continue,
            };

            let mut cluster = AssociationCluster::new(center.clone());
            used[i] = true;

            // Find related facts within expansion radius
            for (j, candidate) in facts.iter().enumerate() {
                if used[j] || i == j {
                    continue;
                }

                let candidate_embedding = match &candidate.embedding {
                    Some(e) => e,
                    None => continue,
                };

                let similarity = Self::cosine_similarity(center_embedding, candidate_embedding);
                let distance = 1.0 - similarity;

                if distance <= self.config.expansion_radius {
                    cluster.add_related(candidate.clone(), similarity);
                    used[j] = true;
                }
            }

            // Only include clusters that meet minimum size
            if cluster.size() >= self.config.min_cluster_size {
                clusters.push(cluster);
            }

            // Stop if we have enough clusters
            if clusters.len() >= self.config.max_associations {
                break;
            }
        }

        clusters
    }

    /// Calculate cosine similarity between two vectors
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }

        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }

        dot / (norm_a * norm_b)
    }
}

impl Default for AssociationRetriever {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::FactType;

    #[test]
    fn test_association_config_default() {
        let config = AssociationConfig::default();
        assert!((config.expansion_radius - 0.4).abs() < 0.01);
        assert_eq!(config.max_associations, 5);
        assert_eq!(config.min_cluster_size, 2);
        assert!(!config.generate_theme);
    }

    #[test]
    fn test_cluster_creation() {
        let center = MemoryFact::new(
            "User likes Rust".to_string(),
            FactType::Preference,
            vec![],
        );

        let related = vec![MemoryFact::new(
            "User uses Cargo".to_string(),
            FactType::Learning,
            vec![],
        )];

        let cluster = AssociationCluster {
            center_fact: center,
            related_facts: related,
            cluster_theme: None,
            avg_similarity: 0.85,
        };

        assert_eq!(cluster.related_facts.len(), 1);
        assert_eq!(cluster.size(), 2);
    }

    #[test]
    fn test_cluster_add_related() {
        let center = MemoryFact::new(
            "User likes Rust".to_string(),
            FactType::Preference,
            vec![],
        );

        let mut cluster = AssociationCluster::new(center);
        assert_eq!(cluster.size(), 1);

        let related = MemoryFact::new("User uses Cargo".to_string(), FactType::Learning, vec![]);
        cluster.add_related(related, 0.9);

        assert_eq!(cluster.size(), 2);
        assert!((cluster.avg_similarity - 0.9).abs() < 0.01);
    }

    #[test]
    fn test_retriever_creation() {
        let retriever = AssociationRetriever::default();
        assert!((retriever.config().expansion_radius - 0.4).abs() < 0.01);
    }

    #[test]
    fn test_find_associations_empty() {
        let retriever = AssociationRetriever::default();
        let clusters = retriever.find_associations(&[]);
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_cosine_similarity() {
        // Identical vectors
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = AssociationRetriever::cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.01);

        // Orthogonal vectors
        let c = vec![1.0, 0.0, 0.0];
        let d = vec![0.0, 1.0, 0.0];
        let sim2 = AssociationRetriever::cosine_similarity(&c, &d);
        assert!(sim2.abs() < 0.01);
    }

    #[test]
    fn test_find_associations_with_embeddings() {
        // Create facts with similar embeddings (within expansion_radius of 0.4)
        let fact1 = MemoryFact::new(
            "User likes Rust".to_string(),
            FactType::Preference,
            vec![],
        )
        .with_embedding(vec![1.0, 0.0, 0.0]);

        let fact2 = MemoryFact::new(
            "User uses Cargo".to_string(),
            FactType::Learning,
            vec![],
        )
        .with_embedding(vec![0.9, 0.1, 0.0]); // Similar to fact1

        let fact3 = MemoryFact::new(
            "User prefers tokio".to_string(),
            FactType::Preference,
            vec![],
        )
        .with_embedding(vec![0.85, 0.15, 0.0]); // Similar to fact1 and fact2

        let fact4 = MemoryFact::new(
            "User likes pizza".to_string(),
            FactType::Personal,
            vec![],
        )
        .with_embedding(vec![0.0, 0.0, 1.0]); // Orthogonal, different cluster

        let facts = vec![fact1, fact2, fact3, fact4];
        let retriever = AssociationRetriever::default();
        let clusters = retriever.find_associations(&facts);

        // Should form at least one cluster with the Rust-related facts
        assert!(!clusters.is_empty());
        // First cluster should have size >= 2 (min_cluster_size)
        assert!(clusters[0].size() >= 2);
    }

    #[test]
    fn test_cluster_with_theme() {
        let center = MemoryFact::new(
            "User likes Rust".to_string(),
            FactType::Preference,
            vec![],
        );

        let cluster =
            AssociationCluster::new(center).with_theme("Programming Languages".to_string());

        assert_eq!(cluster.cluster_theme, Some("Programming Languages".to_string()));
    }

    #[test]
    fn test_config_builder() {
        let config = AssociationConfig::default()
            .with_radius(0.5)
            .with_max_associations(10)
            .with_theme_generation();

        assert!((config.expansion_radius - 0.5).abs() < 0.01);
        assert_eq!(config.max_associations, 10);
        assert!(config.generate_theme);
    }

    #[test]
    fn test_cosine_similarity_edge_cases() {
        // Empty vectors
        let sim1 = AssociationRetriever::cosine_similarity(&[], &[]);
        assert_eq!(sim1, 0.0);

        // Mismatched lengths
        let sim2 = AssociationRetriever::cosine_similarity(&[1.0, 0.0], &[1.0]);
        assert_eq!(sim2, 0.0);

        // Zero vectors
        let sim3 = AssociationRetriever::cosine_similarity(&[0.0, 0.0], &[0.0, 0.0]);
        assert_eq!(sim3, 0.0);
    }

    #[test]
    fn test_facts_without_embeddings_skipped() {
        let fact1 = MemoryFact::new(
            "User likes Rust".to_string(),
            FactType::Preference,
            vec![],
        ); // No embedding

        let fact2 = MemoryFact::new(
            "User uses Cargo".to_string(),
            FactType::Learning,
            vec![],
        ); // No embedding

        let facts = vec![fact1, fact2];
        let retriever = AssociationRetriever::default();
        let clusters = retriever.find_associations(&facts);

        // Should return empty since no facts have embeddings
        assert!(clusters.is_empty());
    }
}
