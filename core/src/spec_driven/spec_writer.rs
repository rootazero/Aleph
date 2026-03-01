//! SpecWriter - generates specifications from requirements.
//!
//! Uses LLM to transform user requirements into structured specifications
//! with acceptance criteria and implementation notes.

use crate::sync_primitives::Arc;

use serde::Deserialize;
use tracing::{debug, info};

use crate::error::{AlephError, Result};
use crate::providers::AiProvider;

use super::types::{Spec, SpecMetadata, SpecTarget};

/// System prompt for spec generation
const SPEC_SYSTEM_PROMPT: &str = r#"You are a senior software architect. Generate a clear, actionable specification from the user's requirement.

Output a JSON object with this structure:
{
  "title": "Short title (max 50 chars)",
  "description": "Detailed description of what needs to be built",
  "acceptance_criteria": ["Criterion 1", "Criterion 2", ...],
  "implementation_notes": "Optional hints and constraints",
  "target": {
    "language": "rust|python|typescript|etc",
    "framework": "optional framework name",
    "output_path": "suggested/file/path.ext"
  }
}

Rules:
- Each acceptance criterion must be testable and specific
- Include at least 3 acceptance criteria
- Be explicit about edge cases
- Keep it concise but complete
- Output ONLY valid JSON, no markdown"#;

/// SpecWriter generates specifications from requirements.
pub struct SpecWriter {
    provider: Arc<dyn AiProvider>,
}

impl SpecWriter {
    /// Create a new SpecWriter with the given AI provider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }

    /// Generate a specification from a requirement.
    pub async fn generate(&self, requirement: &str) -> Result<Spec> {
        info!(requirement = %requirement, "Generating spec");

        // Build prompt
        let prompt = format!(
            "Generate a specification for the following requirement:\n\n{}",
            requirement
        );

        // Call LLM
        let response = self
            .provider
            .process(&prompt, Some(SPEC_SYSTEM_PROMPT))
            .await?;

        debug!(response = %response, "LLM response");

        // Parse response
        let spec = self.parse_response(&response, requirement)?;

        info!(spec_id = %spec.id, title = %spec.title, "Spec generated");

        Ok(spec)
    }

    /// Parse LLM response into a Spec.
    fn parse_response(&self, response: &str, original_requirement: &str) -> Result<Spec> {
        // Try to extract JSON from response (handle markdown code blocks)
        let json_str = extract_json(response);

        let parsed: SpecResponse = serde_json::from_str(&json_str).map_err(|e| {
            AlephError::Other {
                message: format!("Failed to parse spec response: {}", e),
                suggestion: Some("Ensure the LLM returned valid JSON".to_string()),
            }
        })?;

        // Generate ID
        let id = format!("spec-{}", &uuid::Uuid::new_v4().to_string()[..8]);

        // Build Spec
        let mut spec = Spec::new(&id, &parsed.title, &parsed.description);

        for criterion in parsed.acceptance_criteria {
            spec = spec.with_criterion(criterion);
        }

        spec.implementation_notes = parsed.implementation_notes;
        spec.target = parsed.target.unwrap_or_default();
        spec.metadata = SpecMetadata {
            created_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            ),
            original_requirement: original_requirement.to_string(),
            iteration: 0,
        };

        Ok(spec)
    }
}

/// Internal struct for parsing LLM response
#[derive(Debug, Deserialize)]
struct SpecResponse {
    title: String,
    description: String,
    acceptance_criteria: Vec<String>,
    implementation_notes: Option<String>,
    target: Option<SpecTarget>,
}

/// Extract JSON from response (handles markdown code blocks)
pub fn extract_json(response: &str) -> String {
    // Try to find JSON in code block
    if let Some(start) = response.find("```json") {
        if let Some(end) = response[start + 7..].find("```") {
            return response[start + 7..start + 7 + end].trim().to_string();
        }
    }

    // Try to find JSON in generic code block
    if let Some(start) = response.find("```") {
        let after_start = start + 3;
        // Skip language identifier if present
        let content_start = response[after_start..]
            .find('\n')
            .map(|i| after_start + i + 1)
            .unwrap_or(after_start);
        if let Some(end) = response[content_start..].find("```") {
            return response[content_start..content_start + end].trim().to_string();
        }
    }

    // Assume entire response is JSON
    response.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_code_block() {
        let response = r#"Here's the spec:
```json
{"title": "Test", "description": "A test spec"}
```
"#;
        let json = extract_json(response);
        assert!(json.starts_with('{'));
        assert!(json.contains("Test"));
    }

    #[test]
    fn test_extract_json_plain() {
        let response = r#"{"title": "Test", "description": "A test spec"}"#;
        let json = extract_json(response);
        assert_eq!(json, response);
    }

    #[test]
    fn test_extract_json_generic_block() {
        let response = "```\n{\"title\": \"Test\"}\n```";
        let json = extract_json(response);
        assert!(json.contains("Test"));
    }
}
