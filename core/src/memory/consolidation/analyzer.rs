//! Consolidation analyzer for identifying frequent facts

use std::collections::HashMap;
use std::sync::Arc;

use crate::memory::{FactType, MemoryFact, VectorDatabase};
use crate::providers::AiProvider;
use crate::Result;

use super::profile::{ConsolidatedFact, ProfileCategory, UserProfile};

/// A fact with its access frequency
#[derive(Debug, Clone)]
pub struct FrequentFact {
    /// The fact
    pub fact: MemoryFact,

    /// Access frequency score (higher = more frequent)
    pub frequency_score: f32,
}

/// Analyzes facts to identify consolidation opportunities
pub struct ConsolidationAnalyzer {
    database: Arc<VectorDatabase>,
    provider: Option<Arc<dyn AiProvider>>,
}

/// Configuration for consolidation analysis
#[derive(Debug, Clone)]
pub struct ConsolidationConfig {
    /// Minimum frequency score to consider (default: 0.5)
    pub min_frequency_score: f32,

    /// Maximum facts to analyze (default: 100)
    pub max_facts: usize,

    /// Similarity threshold for grouping facts (default: 0.8)
    pub similarity_threshold: f32,

    /// Whether to use LLM for categorization (default: false for MVP)
    pub use_llm_categorization: bool,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            min_frequency_score: 0.5,
            max_facts: 100,
            similarity_threshold: 0.8,
            use_llm_categorization: false,
        }
    }
}

impl ConsolidationAnalyzer {
    /// Create a new consolidation analyzer
    pub fn new(
        database: Arc<VectorDatabase>,
        provider: Option<Arc<dyn AiProvider>>,
    ) -> Self {
        Self { database, provider }
    }

    /// Analyze facts and generate a user profile
    pub async fn generate_profile(&self, config: ConsolidationConfig) -> Result<UserProfile> {
        // Get all valid facts
        let all_facts = self.database.get_all_facts(false).await?;

        // Calculate frequency scores
        let frequent_facts = self.calculate_frequency_scores(all_facts, &config);

        // Group facts by category
        let categories = if config.use_llm_categorization && self.provider.is_some() {
            self.categorize_with_llm(&frequent_facts).await?
        } else {
            self.categorize_by_type(&frequent_facts)
        };

        // Consolidate facts within each category
        let mut profile = UserProfile::new();
        for (category_name, facts) in categories {
            let consolidated = self.consolidate_category(facts, config.similarity_threshold);
            profile.add_category(consolidated);
        }

        Ok(profile)
    }

    /// Calculate frequency scores for facts
    ///
    /// For MVP, we use a simple heuristic based on:
    /// - Fact confidence
    /// - Recency (updated_at timestamp)
    fn calculate_frequency_scores(
        &self,
        facts: Vec<MemoryFact>,
        config: &ConsolidationConfig,
    ) -> Vec<FrequentFact> {
        let now = chrono::Utc::now().timestamp();
        let mut scored_facts: Vec<FrequentFact> = facts
            .into_iter()
            .map(|fact| {
                // Calculate recency score (facts updated in last 30 days get higher score)
                let days_old = (now - fact.updated_at) / 86400;
                let recency_score = if days_old < 30 {
                    1.0 - (days_old as f32 / 30.0)
                } else {
                    0.0
                };

                // Combine confidence and recency
                let frequency_score = (fact.confidence * 0.7) + (recency_score * 0.3);

                FrequentFact {
                    fact,
                    frequency_score,
                }
            })
            .filter(|ff| ff.frequency_score >= config.min_frequency_score)
            .collect();

        // Sort by frequency score (descending)
        scored_facts.sort_by(|a, b| {
            b.frequency_score
                .partial_cmp(&a.frequency_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top N facts
        scored_facts.truncate(config.max_facts);
        scored_facts
    }

    /// Categorize facts by their FactType (simple approach)
    fn categorize_by_type(
        &self,
        frequent_facts: &[FrequentFact],
    ) -> HashMap<String, Vec<FrequentFact>> {
        let mut categories: HashMap<String, Vec<FrequentFact>> = HashMap::new();

        for ff in frequent_facts {
            let category_name = match ff.fact.fact_type {
                FactType::Preference => "preferences",
                FactType::Plan => "plans",
                FactType::Learning => "learning",
                FactType::Project => "projects",
                FactType::Personal => "personal",
                FactType::Other => "other",
            };

            categories
                .entry(category_name.to_string())
                .or_insert_with(Vec::new)
                .push(ff.clone());
        }

        categories
    }

    /// Categorize facts using LLM (advanced approach)
    async fn categorize_with_llm(
        &self,
        _frequent_facts: &[FrequentFact],
    ) -> Result<HashMap<String, Vec<FrequentFact>>> {
        // TODO: Implement LLM-based categorization
        // For now, fall back to type-based categorization
        Ok(self.categorize_by_type(_frequent_facts))
    }

    /// Consolidate similar facts within a category
    fn consolidate_category(
        &self,
        facts: Vec<FrequentFact>,
        similarity_threshold: f32,
    ) -> ProfileCategory {
        let category_name = if !facts.is_empty() {
            match facts[0].fact.fact_type {
                FactType::Preference => "preferences",
                FactType::Plan => "plans",
                FactType::Learning => "learning",
                FactType::Project => "projects",
                FactType::Personal => "personal",
                FactType::Other => "other",
            }
        } else {
            "unknown"
        };

        let mut category = ProfileCategory::new(category_name.to_string());

        // Group similar facts
        let mut processed = vec![false; facts.len()];

        for i in 0..facts.len() {
            if processed[i] {
                continue;
            }

            let mut group = vec![&facts[i]];
            processed[i] = true;

            // Find similar facts
            for j in (i + 1)..facts.len() {
                if processed[j] {
                    continue;
                }

                if self.are_similar(&facts[i].fact, &facts[j].fact, similarity_threshold) {
                    group.push(&facts[j]);
                    processed[j] = true;
                }
            }

            // Create consolidated fact from group
            let consolidated = self.consolidate_group(group);
            category.add_fact(consolidated);
        }

        category
    }

    /// Check if two facts are similar
    fn are_similar(&self, fact1: &MemoryFact, fact2: &MemoryFact, threshold: f32) -> bool {
        let (Some(emb1), Some(emb2)) = (&fact1.embedding, &fact2.embedding) else {
            return false;
        };

        let similarity = cosine_similarity(emb1, emb2);
        similarity >= threshold
    }

    /// Consolidate a group of similar facts into one
    fn consolidate_group(&self, group: Vec<&FrequentFact>) -> ConsolidatedFact {
        // Use the fact with highest frequency score as the representative
        let representative = group
            .iter()
            .max_by(|a, b| {
                a.frequency_score
                    .partial_cmp(&b.frequency_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        let source_fact_ids: Vec<String> = group.iter().map(|ff| ff.fact.id.clone()).collect();

        // Calculate aggregate access count (use frequency score as proxy)
        let access_count = (representative.frequency_score * 100.0) as u32;

        ConsolidatedFact::new(
            representative.fact.content.clone(),
            source_fact_ids,
            access_count,
            representative.fact.updated_at,
        )
    }
}

/// Calculate cosine similarity between two vectors
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot_product / (norm_a * norm_b)
}
