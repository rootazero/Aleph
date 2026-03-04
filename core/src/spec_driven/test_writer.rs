//! TestWriter - generates test cases from specifications.
//!
//! Uses LLM to create comprehensive test cases including edge cases.

use crate::sync_primitives::Arc;

use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::error::Result;
use crate::providers::AiProvider;
use crate::utils::json_extract::extract_json_robust;

use super::types::{AssertionType, Spec, TestCase, TestType};

/// System prompt for test generation
const TEST_SYSTEM_PROMPT: &str = r#"You are a senior QA engineer. Generate comprehensive test cases for the given specification.

Output a JSON array of test cases:
[
  {
    "name": "test_function_name",
    "description": "What this test verifies",
    "test_type": "unit|integration|e2e",
    "input": <any JSON value>,
    "expected": <any JSON value>,
    "assertion": "equals|contains|matches|greater_than|less_than|not_null|throws",
    "is_edge_case": false
  }
]

Rules:
- Include at least one test per acceptance criterion
- Include at least 2 edge cases (empty input, boundary values, error conditions)
- Test names should be descriptive (test_<what>_<when>_<expected>)
- Use snake_case for test names
- Output ONLY valid JSON array, no markdown"#;

/// TestWriter generates test cases from specifications.
pub struct TestWriter {
    provider: Arc<dyn AiProvider>,
}

impl TestWriter {
    /// Create a new TestWriter with the given AI provider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }

    /// Generate test cases for a specification.
    pub async fn generate(&self, spec: &Spec) -> Result<Vec<TestCase>> {
        info!(spec_id = %spec.id, title = %spec.title, "Generating tests");

        // Build prompt
        let prompt = self.build_prompt(spec);

        // Call LLM
        let response = self
            .provider
            .process(&prompt, Some(TEST_SYSTEM_PROMPT))
            .await?;

        debug!(response = %response, "LLM response");

        // Parse response
        let tests = self.parse_response(&response)?;

        info!(
            spec_id = %spec.id,
            test_count = tests.len(),
            edge_cases = tests.iter().filter(|t| t.is_edge_case).count(),
            "Tests generated"
        );

        Ok(tests)
    }

    /// Build prompt from spec.
    fn build_prompt(&self, spec: &Spec) -> String {
        let criteria = spec
            .acceptance_criteria
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{}. {}", i + 1, c))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"Generate test cases for this specification:

Title: {}
Description: {}

Acceptance Criteria:
{}

Target Language: {}
{}"#,
            spec.title,
            spec.description,
            criteria,
            spec.target.language,
            spec.implementation_notes
                .as_ref()
                .map(|n| format!("\nNotes: {}", n))
                .unwrap_or_default()
        )
    }

    /// Parse LLM response into test cases.
    fn parse_response(&self, response: &str) -> Result<Vec<TestCase>> {
        let json_value = match extract_json_robust(response) {
            Some(v) => v,
            None => {
                warn!("No JSON found in test writer response, returning empty test list");
                return Ok(vec![]);
            }
        };

        let parsed: Vec<TestCaseResponse> = match serde_json::from_value(json_value) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to parse test cases JSON: {}, returning empty test list", e);
                return Ok(vec![]);
            }
        };

        let tests = parsed
            .into_iter()
            .map(|tc| TestCase {
                name: tc.name,
                description: tc.description,
                test_type: tc.test_type.unwrap_or_default(),
                input: tc.input,
                expected: tc.expected,
                assertion: tc.assertion.unwrap_or_default(),
                is_edge_case: tc.is_edge_case.unwrap_or(false),
            })
            .collect();

        Ok(tests)
    }
}

/// Internal struct for parsing LLM response
#[derive(Debug, Deserialize)]
struct TestCaseResponse {
    name: String,
    description: String,
    test_type: Option<TestType>,
    input: serde_json::Value,
    expected: serde_json::Value,
    assertion: Option<AssertionType>,
    is_edge_case: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_prompt() {
        let spec = Spec::new("id", "Add Numbers", "Add two numbers together")
            .with_criterion("Should handle positive numbers")
            .with_criterion("Should handle negative numbers")
            .with_language("rust");

        let writer = TestWriter::new(Arc::new(MockProvider));
        let prompt = writer.build_prompt(&spec);

        assert!(prompt.contains("Add Numbers"));
        assert!(prompt.contains("positive numbers"));
        assert!(prompt.contains("rust"));
    }

    struct MockProvider;

    impl crate::providers::AiProvider for MockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            Box::pin(async { Ok("[]".to_string()) })
        }

        fn process_with_thinking(
            &self,
            input: &str,
            system_prompt: Option<&str>,
            _level: crate::agents::thinking::ThinkLevel,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            self.process(input, system_prompt)
        }

        fn process_with_image(
            &self,
            input: &str,
            _image: Option<&crate::ImageData>,
            system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            self.process(input, system_prompt)
        }

        fn process_with_attachments(
            &self,
            input: &str,
            _attachments: Option<&[crate::core::MediaAttachment]>,
            system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            self.process(input, system_prompt)
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "gray"
        }
    }
}
