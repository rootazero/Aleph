//! Pattern extraction from experiences using LLM analysis
//!
//! This module implements the core pattern extraction logic that converts
//! raw execution traces into reusable, parameterized patterns.

use crate::error::{AlephError, Result};
use crate::memory::cortex::{EnvironmentContext, Experience, ParameterConfig, ParameterMapping};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

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
}

impl PatternExtractor {
    /// Create a new pattern extractor
    pub fn new(config: PatternExtractorConfig) -> Self {
        Self { config }
    }

    /// Extract pattern from an experience
    pub async fn extract_pattern(
        &self,
        experience: &Experience,
        use_realtime_model: bool,
    ) -> Result<ExtractedPattern> {
        info!("Extracting pattern from experience: {}", experience.id);

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
    async fn call_llm(&self, model: &str, prompt: &str) -> Result<String> {
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
        // Extract JSON from response (handle markdown code blocks)
        let json_str = self.extract_json(response)?;

        // Parse JSON
        serde_json::from_str(&json_str).map_err(|e| AlephError::Other {
            message: format!("Failed to parse LLM response: {}", e),
            suggestion: Some("Check LLM output format".to_string()),
        })
    }

    /// Extract JSON from response (handle markdown code blocks)
    fn extract_json(&self, response: &str) -> Result<String> {
        let trimmed = response.trim();

        // Check if wrapped in markdown code block
        if trimmed.starts_with("```json") {
            let start = trimmed.find('{').ok_or_else(|| AlephError::Other {
                message: "No JSON object found in response".to_string(),
                suggestion: None,
            })?;
            let end = trimmed.rfind('}').ok_or_else(|| AlephError::Other {
                message: "No JSON object found in response".to_string(),
                suggestion: None,
            })?;
            Ok(trimmed[start..=end].to_string())
        } else if trimmed.starts_with('{') {
            Ok(trimmed.to_string())
        } else {
            Err(AlephError::Other {
                message: "Response does not contain valid JSON".to_string(),
                suggestion: None,
            })
        }
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
    use crate::memory::cortex::{EnvironmentContext, ExperienceBuilder};

    #[test]
    fn test_extract_json_from_markdown() {
        let config = PatternExtractorConfig::default();
        let extractor = PatternExtractor::new(config);

        let response = r#"```json
{
  "description": "Test pattern",
  "parameter_mapping": {
    "variables": {}
  },
  "key_steps": ["step1"]
}
```"#;

        let json = extractor.extract_json(response).unwrap();
        assert!(json.contains("Test pattern"));
    }

    #[test]
    fn test_extract_json_plain() {
        let config = PatternExtractorConfig::default();
        let extractor = PatternExtractor::new(config);

        let response = r#"{"description": "Test", "parameter_mapping": {"variables": {}}, "key_steps": []}"#;

        let json = extractor.extract_json(response).unwrap();
        assert!(json.contains("Test"));
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
}
