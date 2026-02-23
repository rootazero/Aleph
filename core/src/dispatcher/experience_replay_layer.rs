//! L1.5 Experience Replay Layer - Fast pattern matching without LLM
//!
//! This layer sits between L1 (cache) and L2 (semantic routing), enabling
//! "muscle memory" execution by replaying verified patterns.

use crate::error::{AlephError, Result};
use crate::memory::cortex::{Experience, ParameterMapping, ReplayMatch};
use crate::memory::store::MemoryBackend;
use crate::memory::smart_embedder::SmartEmbedder;
use std::sync::Arc;
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
    #[allow(dead_code)]
    db: MemoryBackend,
    embedder: Arc<SmartEmbedder>,
    config: ExperienceReplayConfig,
}

impl ExperienceReplayLayer {
    /// Create a new Experience Replay Layer
    pub fn new(
        db: MemoryBackend,
        embedder: Arc<SmartEmbedder>,
        config: ExperienceReplayConfig,
    ) -> Self {
        Self {
            db,
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
        let _intent_vector = self.embedder.embed(intent).await.map_err(|e| {
            AlephError::Other {
                message: format!("Failed to embed intent: {}", e),
                suggestion: None,
            }
        })?;

        // TODO: Implement experience vector search via new store API
        // Old code: db.vector_search_experiences(vector, max_candidates, threshold, statuses)
        let candidates: Vec<(Experience, f64)> = Vec::new();

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
    use crate::memory::cortex::{ExperienceBuilder, ParameterConfig};
    use std::collections::HashMap;
    use tempfile::TempDir;

    async fn create_test_db() -> (MemoryBackend, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let backend = crate::memory::store::lance::LanceMemoryBackend::open_or_create(temp_dir.path()).await.unwrap();
        (Arc::new(backend), temp_dir)
    }

    #[tokio::test]
    async fn test_extract_with_regex() {
        let (db, temp) = create_test_db().await;
        let cache_dir = temp.path().join("embedder_cache");
        let embedder = Arc::new(SmartEmbedder::new(cache_dir, 3600));
        let config = ExperienceReplayConfig::default();
        let layer = ExperienceReplayLayer::new(db, embedder, config);

        let text = "Search for TODO in file main.rs";
        let pattern = r"file\s+(\S+)";

        let result = layer.extract_with_regex(text, pattern).unwrap();
        assert_eq!(result, Some("main.rs".to_string()));
    }

    #[tokio::test]
    async fn test_extract_after_keyword() {
        let (db, temp) = create_test_db().await;
        let cache_dir = temp.path().join("embedder_cache");
        let embedder = Arc::new(SmartEmbedder::new(cache_dir, 3600));
        let config = ExperienceReplayConfig::default();
        let layer = ExperienceReplayLayer::new(db, embedder, config);

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
