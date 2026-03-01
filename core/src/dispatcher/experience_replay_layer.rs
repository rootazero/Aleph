//! L1.5 Experience Replay Layer - Fast pattern matching without LLM
//!
//! This layer sits between L1 (cache) and L2 (semantic routing), enabling
//! "muscle memory" execution by replaying verified patterns.

use crate::error::{AlephError, Result};
use crate::poe::crystallization::experience::{
    Experience, ExperienceBuilder, EvolutionStatus, ParameterMapping, ReplayMatch,
};
use crate::poe::crystallization::experience_store::{ExperienceStore, PoeExperience};
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use tracing::{debug, info};

/// Configuration for Experience Replay Layer
#[derive(Debug, Clone)]
pub struct ExperienceReplayConfig {
    /// Minimum similarity threshold for matching (0.0-1.0)
    pub similarity_threshold: f64,
    /// Maximum number of candidates to consider
    pub max_candidates: usize,
    /// Enable L1.5 routing
    pub enabled: bool,
}

impl Default for ExperienceReplayConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.85,
            max_candidates: 5,
            enabled: true,
        }
    }
}

/// L1.5 Experience Replay Layer
pub struct ExperienceReplayLayer {
    experience_store: Arc<dyn ExperienceStore>,
    embedder: Arc<dyn EmbeddingProvider>,
    config: ExperienceReplayConfig,
}

impl ExperienceReplayLayer {
    /// Create a new Experience Replay Layer
    pub fn new(
        experience_store: Arc<dyn ExperienceStore>,
        embedder: Arc<dyn EmbeddingProvider>,
        config: ExperienceReplayConfig,
    ) -> Self {
        Self {
            experience_store,
            embedder,
            config,
        }
    }

    /// Try to match intent against verified experiences
    pub async fn try_match(&self, intent: &str) -> Result<Option<ReplayMatch>> {
        if !self.config.enabled {
            debug!("L1.5 routing disabled");
            return Ok(None);
        }

        info!("L1.5: Attempting to match intent: {}", intent);

        // Step 1: Generate intent embedding
        let intent_vector = self.embedder.embed(intent).await.map_err(|e| {
            AlephError::Other {
                message: format!("Failed to embed intent: {}", e),
                suggestion: None,
            }
        })?;

        // Step 2: Search for similar experiences via ExperienceStore
        let search_results = self
            .experience_store
            .vector_search(
                &intent_vector,
                self.config.max_candidates,
                self.config.similarity_threshold,
            )
            .await
            .map_err(|e| AlephError::Other {
                message: format!("Experience search failed: {}", e),
                suggestion: None,
            })?;

        let candidates: Vec<(Experience, f64)> = search_results
            .into_iter()
            .map(|(poe_exp, sim)| (poe_experience_to_experience(poe_exp, sim), sim))
            .collect();

        if candidates.is_empty() {
            debug!("L1.5: No matching experiences found");
            return Ok(None);
        }

        info!("L1.5: Found {} candidate experiences", candidates.len());

        // Step 3: Select best match
        let best_match = self.select_best_match(intent, candidates).await?;

        // Step 4: Extract and fill parameters
        let filled_sequence = self.fill_parameters(intent, &best_match).await?;

        let replay_match = ReplayMatch {
            experience_id: best_match.id.clone(),
            tool_sequence: filled_sequence,
            confidence: best_match.similarity_score,
        };

        info!(
            "L1.5: Matched experience {} with confidence {:.2}",
            replay_match.experience_id, replay_match.confidence
        );

        Ok(Some(replay_match))
    }

    /// Select the best matching experience from candidates
    async fn select_best_match(
        &self,
        _intent: &str,
        mut candidates: Vec<(Experience, f64)>,
    ) -> Result<ExperienceWithScore> {
        // Sort by similarity score (highest first)
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        // Take the best match
        let (experience, similarity) = candidates
            .into_iter()
            .next()
            .ok_or_else(|| AlephError::Other {
                message: "No candidates available".to_string(),
                suggestion: None,
            })?;

        Ok(ExperienceWithScore {
            id: experience.id,
            tool_sequence_json: experience.tool_sequence_json,
            parameter_mapping: experience.parameter_mapping,
            similarity_score: similarity,
        })
    }

    /// Fill parameters into the tool sequence
    async fn fill_parameters(
        &self,
        intent: &str,
        experience: &ExperienceWithScore,
    ) -> Result<String> {
        // If no parameter mapping, return tool sequence as-is
        let Some(ref mapping_json) = experience.parameter_mapping else {
            debug!("No parameter mapping, using tool sequence as-is");
            return Ok(experience.tool_sequence_json.clone());
        };

        // Parse parameter mapping
        let mapping: ParameterMapping =
            serde_json::from_str(mapping_json).map_err(|e| AlephError::Other {
                message: format!("Failed to parse parameter mapping: {}", e),
                suggestion: None,
            })?;

        // Extract parameters from intent
        let extracted_params = self.extract_parameters(intent, &mapping)?;

        // Fill parameters into tool sequence
        let mut filled_sequence = experience.tool_sequence_json.clone();
        for (var_name, value) in extracted_params {
            // Simple string replacement for now
            // TODO: Implement more sophisticated parameter filling
            let placeholder = format!("{{{{{}}}}}", var_name);
            filled_sequence = filled_sequence.replace(&placeholder, &value);
        }

        Ok(filled_sequence)
    }

    /// Extract parameters from intent using mapping rules
    fn extract_parameters(
        &self,
        intent: &str,
        mapping: &ParameterMapping,
    ) -> Result<Vec<(String, String)>> {
        let mut extracted = Vec::new();

        for (var_name, var_config) in &mapping.variables {
            // Parse extraction rule
            let value = if !var_config.extraction_rule.is_empty() {
                let rule = &var_config.extraction_rule;
                if let Some(pattern) = rule.strip_prefix("regex:") {
                    // Regex extraction
                    self.extract_with_regex(intent, pattern)?
                } else if let Some(keyword) = rule.strip_prefix("keyword_after:") {
                    // Keyword-based extraction
                    self.extract_after_keyword(intent, keyword)?
                } else {
                    // Unknown rule, use default if available
                    var_config.default.clone()
                }
            } else {
                // No rule, use default
                var_config.default.clone()
            };

            if let Some(val) = value {
                extracted.push((var_name.clone(), val));
            } else {
                // Required parameter missing
                debug!("Required parameter '{}' not found in intent", var_name);
                return Err(AlephError::Other {
                    message: format!("Required parameter '{}' not found", var_name),
                    suggestion: Some("Intent does not match pattern requirements".to_string()),
                });
            }
        }

        Ok(extracted)
    }

    /// Extract value using regex pattern
    fn extract_with_regex(&self, text: &str, pattern: &str) -> Result<Option<String>> {
        let re = regex::Regex::new(pattern).map_err(|e| AlephError::Other {
            message: format!("Invalid regex pattern: {}", e),
            suggestion: None,
        })?;

        Ok(re
            .captures(text)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string()))
    }

    /// Extract value after a keyword
    fn extract_after_keyword(&self, text: &str, keyword: &str) -> Result<Option<String>> {
        if let Some(pos) = text.to_lowercase().find(&keyword.to_lowercase()) {
            let after = &text[pos + keyword.len()..];
            // Extract until next space or end
            let value = after
                .split_whitespace()
                .next()
                .map(|s| s.to_string());
            Ok(value)
        } else {
            Ok(None)
        }
    }
}

/// Convert a PoeExperience from the store into the Experience type used by the replay pipeline.
fn poe_experience_to_experience(exp: PoeExperience, similarity: f64) -> Experience {
    ExperienceBuilder::new(exp.id, exp.objective, exp.tool_sequence_json)
        .pattern_hash(exp.pattern_id)
        .success_score(similarity)
        .latency_ms(exp.duration_ms as i64)
        .build()
        // Apply fields not covered by builder
        .with_parameter_mapping(exp.parameter_mapping)
        .with_evolution_status_if_high_satisfaction(exp.satisfaction)
}

/// Extension methods for Experience construction from PoeExperience.
trait ExperienceExt {
    fn with_parameter_mapping(self, mapping: Option<String>) -> Self;
    fn with_evolution_status_if_high_satisfaction(self, satisfaction: f32) -> Self;
}

impl ExperienceExt for Experience {
    fn with_parameter_mapping(mut self, mapping: Option<String>) -> Self {
        self.parameter_mapping = mapping;
        self
    }

    fn with_evolution_status_if_high_satisfaction(mut self, satisfaction: f32) -> Self {
        if satisfaction >= 0.8 {
            self.evolution_status = EvolutionStatus::Verified;
        }
        self
    }
}

/// Experience with similarity score
struct ExperienceWithScore {
    id: String,
    tool_sequence_json: String,
    parameter_mapping: Option<String>,
    similarity_score: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::experience_store::InMemoryExperienceStore;

    fn create_test_store() -> Arc<dyn ExperienceStore> {
        Arc::new(InMemoryExperienceStore::new())
    }

    #[tokio::test]
    async fn test_extract_with_regex() {
        let store = create_test_store();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(1024, "mock-model"),
        );
        let config = ExperienceReplayConfig::default();
        let layer = ExperienceReplayLayer::new(store, embedder, config);

        let text = "Search for TODO in file main.rs";
        let pattern = r"file\s+(\S+)";

        let result = layer.extract_with_regex(text, pattern).unwrap();
        assert_eq!(result, Some("main.rs".to_string()));
    }

    #[tokio::test]
    async fn test_extract_after_keyword() {
        let store = create_test_store();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(1024, "mock-model"),
        );
        let config = ExperienceReplayConfig::default();
        let layer = ExperienceReplayLayer::new(store, embedder, config);

        let text = "Search for TODO comments";
        let keyword = "for";

        let result = layer.extract_after_keyword(text, keyword).unwrap();
        assert_eq!(result, Some("TODO".to_string()));
    }

    #[test]
    fn test_config_default() {
        let config = ExperienceReplayConfig::default();
        assert_eq!(config.similarity_threshold, 0.85);
        assert_eq!(config.max_candidates, 5);
        assert!(config.enabled);
    }
}
