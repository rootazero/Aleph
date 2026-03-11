//! Pattern extraction from experiences using LLM analysis
//!
//! This module implements the core pattern extraction logic that converts
//! raw execution traces into reusable, parameterized patterns.

use crate::error::{AlephError, Result};
use crate::utils::json_extract::extract_json_robust;
use super::experience::{EnvironmentContext, Experience, ParameterMapping};
use super::synthesis_backend::{PatternSynthesisBackend, PatternSynthesisRequest, ToolSequenceTrace};
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Extracted pattern from an experience
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedPattern {
    /// Natural language description of the pattern
    pub description: String,
    /// Parameter mapping for variable extraction
    pub parameter_mapping: ParameterMapping,
    /// Pattern hash for deduplication
    pub pattern_hash: String,
}

/// Configuration for pattern extraction
#[derive(Debug, Clone)]
pub struct PatternExtractorConfig {
    /// Model to use for realtime extraction (default: claude-3-5-haiku-20241022)
    pub realtime_model: String,
    /// Model to use for batch extraction (default: claude-3-5-haiku-20241022)
    pub batch_model: String,
    /// Temperature for LLM calls (default: 0.3 for consistency)
    pub temperature: f32,
    /// Max tokens for response (default: 2000)
    pub max_tokens: u32,
}

impl Default for PatternExtractorConfig {
    fn default() -> Self {
        Self {
            realtime_model: "claude-3-5-haiku-20241022".to_string(),
            batch_model: "claude-3-5-haiku-20241022".to_string(),
            temperature: 0.3,
            max_tokens: 2000,
        }
    }
}

/// Pattern extractor service
pub struct PatternExtractor {
    config: PatternExtractorConfig,
    backend: Option<Arc<dyn PatternSynthesisBackend>>,
}

impl PatternExtractor {
    /// Create a new pattern extractor (no backend — uses stub LLM logic)
    pub fn new(config: PatternExtractorConfig) -> Self {
        Self {
            config,
            backend: None,
        }
    }

    /// Create a pattern extractor with an LLM synthesis backend
    pub fn with_backend(
        config: PatternExtractorConfig,
        backend: Arc<dyn PatternSynthesisBackend>,
    ) -> Self {
        Self {
            config,
            backend: Some(backend),
        }
    }

    /// Extract pattern from an experience.
    ///
    /// When a `PatternSynthesisBackend` is available, delegates synthesis to it.
    /// Otherwise falls back to the legacy stub LLM path.
    pub async fn extract_pattern(
        &self,
        experience: &Experience,
        use_realtime_model: bool,
    ) -> Result<ExtractedPattern> {
        info!("Extracting pattern from experience: {}", experience.id);

        // Fast path: delegate to backend when available
        if let Some(ref backend) = self.backend {
            let request = self.build_synthesis_request(experience);
            let suggestion = backend
                .synthesize_pattern(request)
                .await
                .map_err(|e| AlephError::Other {
                    message: format!("Backend synthesis failed: {}", e),
                    suggestion: Some("Check backend configuration".to_string()),
                })?;

            return Ok(ExtractedPattern {
                description: suggestion.description,
                parameter_mapping: ParameterMapping {
                    variables: std::collections::HashMap::new(),
                },
                pattern_hash: suggestion.pattern_hash,
            });
        }

        // Legacy stub path
        let model = if use_realtime_model {
            &self.config.realtime_model
        } else {
            &self.config.batch_model
        };

        // Build the extraction prompt
        let prompt = self.build_extraction_prompt(experience);

        // Call LLM
        let response = self.call_llm(model, &prompt).await?;

        // Parse response
        let extracted = self.parse_llm_response(&response)?;

        // Generate pattern hash
        let pattern_hash = self.generate_pattern_hash(&extracted);

        Ok(ExtractedPattern {
            description: extracted.description,
            parameter_mapping: extracted.parameter_mapping,
            pattern_hash,
        })
    }

    /// Build a `PatternSynthesisRequest` from an `Experience`.
    fn build_synthesis_request(&self, experience: &Experience) -> PatternSynthesisRequest {
        let trace = ToolSequenceTrace {
            tool_sequence_json: experience.tool_sequence_json.clone(),
            satisfaction: experience.success_score as f32,
            duration_ms: experience.latency_ms.unwrap_or(0) as u64,
            attempts: 1,
        };

        PatternSynthesisRequest {
            objective: experience.user_intent.clone(),
            tool_sequences: vec![trace],
            env_context: experience.environment_context_json.clone(),
            existing_patterns: vec![],
        }
    }

    /// Build the extraction prompt
    fn build_extraction_prompt(&self, experience: &Experience) -> String {
        // Parse environment context if available
        let (working_dir, platform) = if let Some(ref env_json) = experience.environment_context_json {
            if let Ok(env) = serde_json::from_str::<EnvironmentContext>(env_json) {
                (env.working_directory, env.platform)
            } else {
                ("unknown".to_string(), "unknown".to_string())
            }
        } else {
            ("unknown".to_string(), "unknown".to_string())
        };

        format!(
            r#"You are an expert at analyzing task execution patterns and extracting reusable templates.

# Task
Analyze the following task execution trace and extract a reusable pattern.

# Input Data

## User Intent
{intent}

## Tool Sequence (JSON)
{tool_sequence}

## Environment Context
- Working Directory: {working_dir}
- Platform: {platform}

## Execution Metrics
- Success: {success}
- Token Efficiency: {token_efficiency}
- Latency: {latency_ms}ms

# Your Task

Extract a reusable pattern from this execution trace. Provide your analysis in the following JSON format:

```json
{{
  "description": "A concise natural language description of what this pattern does (1-2 sentences)",
  "parameter_mapping": {{
    "variables": {{
      "variable_name": {{
        "type": "string|path|number|boolean",
        "extraction_rule": "regex:pattern OR keyword_after:text OR entity_type:TYPE",
        "default": null
      }}
    }}
  }},
  "key_steps": [
    "Step 1 description",
    "Step 2 description"
  ]
}}
```

## Guidelines

1. **Description**: Focus on the "what" and "why", not the "how"
2. **Variables**: Identify parts that change between executions (file paths, search terms, etc.)
3. **Extraction Rules**:
   - Use `regex:` for pattern matching
   - Use `keyword_after:` for simple text extraction
   - Use `entity_type:` for NER-based extraction (FILE, PATH, PERSON, etc.)
4. **Key Steps**: List 3-5 critical decision points or actions

# Output

Provide ONLY the JSON object, no additional text."#,
            intent = experience.user_intent,
            tool_sequence = experience.tool_sequence_json,
            working_dir = working_dir,
            platform = platform,
            success = if experience.success_score > 0.5 {
                "Yes"
            } else {
                "No"
            },
            token_efficiency = experience
                .token_efficiency
                .map(|e| format!("{:.2}", e))
                .unwrap_or_else(|| "N/A".to_string()),
            latency_ms = experience
                .latency_ms
                .map(|l| l.to_string())
                .unwrap_or_else(|| "N/A".to_string()),
        )
    }

    /// Call LLM for pattern extraction
    async fn call_llm(&self, model: &str, _prompt: &str) -> Result<String> {
        debug!("Calling LLM model: {}", model);

        // TODO: Implement actual LLM call through ProviderManager
        // For now, return a placeholder

        // This is a placeholder implementation
        // In the real implementation, we would:
        // 1. Get the appropriate provider from provider_manager
        // 2. Create a completion request with the prompt
        // 3. Parse the response

        Err(AlephError::Other {
            message: "LLM integration not yet implemented".to_string(),
            suggestion: Some("This will be implemented when integrating with ProviderManager".to_string()),
        })
    }

    /// Parse LLM response into structured data
    fn parse_llm_response(&self, response: &str) -> Result<ExtractedPatternRaw> {
        // Extract JSON from response using robust extractor
        let json_value = match extract_json_robust(response) {
            Some(v) => v,
            None => {
                warn!("No JSON found in pattern extraction response, returning default pattern");
                return Ok(ExtractedPatternRaw {
                    description: "Pattern extraction failed — raw text response".to_string(),
                    parameter_mapping: ParameterMapping {
                        variables: std::collections::HashMap::new(),
                    },
                    key_steps: vec![],
                });
            }
        };

        // Parse JSON
        serde_json::from_value(json_value).map_err(|e| {
            warn!("Failed to parse pattern extraction JSON: {}", e);
            AlephError::Other {
                message: format!("Failed to parse LLM response: {}", e),
                suggestion: Some("Check LLM output format".to_string()),
            }
        })
    }

    /// Generate pattern hash for deduplication
    fn generate_pattern_hash(&self, pattern: &ExtractedPatternRaw) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();

        // Hash the description and key steps
        pattern.description.hash(&mut hasher);
        for step in &pattern.key_steps {
            step.hash(&mut hasher);
        }

        format!("{:x}", hasher.finish())
    }
}

/// Raw extracted pattern from LLM (before hash generation)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExtractedPatternRaw {
    description: String,
    parameter_mapping: ParameterMapping,
    key_steps: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::poe::crystallization::experience::{EnvironmentContext, ExperienceBuilder};
    use crate::poe::crystallization::synthesis_backend::{
        PatternSuggestion, PatternSynthesisBackend, PatternSynthesisRequest,
    };
    use crate::poe::crystallization::experience_store::PoeExperience;
    use crate::poe::crystallization::pattern_model;
    use async_trait::async_trait;

    #[test]
    fn test_extract_json_from_markdown() {
        let response = r#"```json
{
  "description": "Test pattern",
  "parameter_mapping": {
    "variables": {}
  },
  "key_steps": ["step1"]
}
```"#;

        let result = extract_json_robust(response);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["description"], "Test pattern");
    }

    #[test]
    fn test_extract_json_plain() {
        let response = r#"{"description": "Test", "parameter_mapping": {"variables": {}}, "key_steps": []}"#;

        let result = extract_json_robust(response);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["description"], "Test");
    }

    #[test]
    fn test_parse_llm_response_plain_text_fallback() {
        let config = PatternExtractorConfig::default();
        let extractor = PatternExtractor::new(config);

        // Plain text should return default pattern, not error
        let result = extractor.parse_llm_response("这是一个纯文本回复");
        assert!(result.is_ok());
        let pattern = result.unwrap();
        assert!(pattern.description.contains("extraction failed"));
    }

    #[test]
    fn test_generate_pattern_hash() {
        let config = PatternExtractorConfig::default();
        let extractor = PatternExtractor::new(config);

        let pattern = ExtractedPatternRaw {
            description: "Test pattern".to_string(),
            parameter_mapping: ParameterMapping {
                variables: HashMap::new(),
            },
            key_steps: vec!["step1".to_string(), "step2".to_string()],
        };

        let hash1 = extractor.generate_pattern_hash(&pattern);
        let hash2 = extractor.generate_pattern_hash(&pattern);

        // Same pattern should produce same hash
        assert_eq!(hash1, hash2);

        // Different pattern should produce different hash
        let mut pattern2 = pattern.clone();
        pattern2.description = "Different pattern".to_string();
        let hash3 = extractor.generate_pattern_hash(&pattern2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_build_extraction_prompt() {
        let config = PatternExtractorConfig::default();
        let extractor = PatternExtractor::new(config);

        let env_context = EnvironmentContext {
            working_directory: "/test/dir".to_string(),
            platform: "macos".to_string(),
            permissions: vec![],
            metadata: HashMap::new(),
        };

        let experience = ExperienceBuilder::new(
            "test-1".to_string(),
            "Search for TODO comments".to_string(),
            r#"{"tools": ["grep"]}"#.to_string(),
        )
        .environment_context_json(serde_json::to_string(&env_context).unwrap())
        .latency_ms(5000)
        .build();

        let prompt = extractor.build_extraction_prompt(&experience);

        assert!(prompt.contains("Search for TODO comments"));
        assert!(prompt.contains("/test/dir"));
        assert!(prompt.contains("macos"));
        assert!(prompt.contains("JSON format"));
    }

    // -- Stub backend for testing ------------------------------------------------

    struct StubBackend {
        description: String,
    }

    #[async_trait]
    impl PatternSynthesisBackend for StubBackend {
        async fn synthesize_pattern(
            &self,
            _request: PatternSynthesisRequest,
        ) -> anyhow::Result<PatternSuggestion> {
            Ok(PatternSuggestion {
                description: self.description.clone(),
                steps: vec![],
                parameter_mapping: pattern_model::ParameterMapping::default(),
                pattern_hash: "stub-hash-42".to_string(),
                confidence: 0.95,
            })
        }

        async fn evaluate_confidence(
            &self,
            _pattern_hash: &str,
            _occurrences: &[PoeExperience],
        ) -> anyhow::Result<f32> {
            Ok(0.9)
        }
    }

    #[tokio::test]
    async fn test_extract_with_backend() {
        let backend = Arc::new(StubBackend {
            description: "Compile and test Rust project".to_string(),
        });

        let extractor = PatternExtractor::with_backend(
            PatternExtractorConfig::default(),
            backend,
        );

        let experience = ExperienceBuilder::new(
            "exp-1".to_string(),
            "Build the project".to_string(),
            r#"["cargo build", "cargo test"]"#.to_string(),
        )
        .build();

        let result = extractor.extract_pattern(&experience, true).await;
        assert!(result.is_ok());

        let pattern = result.unwrap();
        assert_eq!(pattern.description, "Compile and test Rust project");
        assert_eq!(pattern.pattern_hash, "stub-hash-42");
    }

    #[test]
    fn test_build_synthesis_request() {
        let extractor = PatternExtractor::new(PatternExtractorConfig::default());

        let experience = ExperienceBuilder::new(
            "exp-2".to_string(),
            "Search logs for errors".to_string(),
            r#"["grep -r error"]"#.to_string(),
        )
        .latency_ms(3000)
        .build();

        let request = extractor.build_synthesis_request(&experience);
        assert_eq!(request.objective, "Search logs for errors");
        assert_eq!(request.tool_sequences.len(), 1);
        assert_eq!(request.tool_sequences[0].duration_ms, 3000);
    }
}
