//! LlmJudge - evaluates implementations against specifications.
//!
//! Uses LLM with extended thinking to provide quality scores and feedback.

use std::collections::HashMap;
use crate::sync_primitives::Arc;

use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::error::{AlephError, Result};
use crate::providers::AiProvider;
use crate::agents::thinking::ThinkLevel;

use super::spec_writer::extract_json;
use super::types::{EvaluationResult, Spec, TestCase, TestResult};

/// System prompt for evaluation
const JUDGE_SYSTEM_PROMPT: &str = r#"You are a senior code reviewer evaluating an implementation against its specification.

Analyze the implementation carefully and output a JSON evaluation:
{
  "score": 0.0 to 1.0,
  "criterion_scores": {"criterion_text": score, ...},
  "feedback": "Detailed feedback about the implementation",
  "suggestions": ["Specific improvement suggestion 1", "..."],
  "is_acceptable": true/false (true if score >= 0.8)
}

Scoring guidelines:
- 1.0: Perfect implementation, all criteria met
- 0.8-0.99: Good implementation, minor issues
- 0.6-0.79: Acceptable but needs improvement
- 0.4-0.59: Significant issues, needs rework
- 0.0-0.39: Fails to meet basic requirements

Consider:
- Correctness: Does it do what the spec says?
- Completeness: Are all acceptance criteria addressed?
- Edge cases: Are edge cases handled properly?
- Code quality: Is it clean, maintainable, idiomatic?

Output ONLY valid JSON, no markdown."#;

/// LlmJudge evaluates implementations against specifications.
pub struct LlmJudge {
    provider: Arc<dyn AiProvider>,
    think_level: ThinkLevel,
}

impl LlmJudge {
    /// Create a new LlmJudge with the given AI provider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            provider,
            think_level: ThinkLevel::Medium,
        }
    }

    /// Create with specified thinking level.
    pub fn with_think_level(mut self, level: ThinkLevel) -> Self {
        self.think_level = level;
        self
    }

    /// Evaluate an implementation against its spec and test results.
    pub async fn evaluate(
        &self,
        spec: &Spec,
        tests: &[TestCase],
        test_results: &[TestResult],
        implementation: &str,
    ) -> Result<EvaluationResult> {
        info!(spec_id = %spec.id, "Evaluating implementation");

        // Build prompt
        let prompt = self.build_prompt(spec, tests, test_results, implementation);

        // Call LLM with thinking for thorough evaluation
        let response = if self.provider.supports_thinking() {
            self.provider
                .process_with_thinking(&prompt, Some(JUDGE_SYSTEM_PROMPT), self.think_level)
                .await?
        } else {
            self.provider
                .process(&prompt, Some(JUDGE_SYSTEM_PROMPT))
                .await?
        };

        debug!(response = %response, "LLM evaluation response");

        // Parse response
        let result = self.parse_response(&response)?;

        info!(
            spec_id = %spec.id,
            score = result.score,
            is_acceptable = result.is_acceptable,
            "Evaluation complete"
        );

        Ok(result)
    }

    /// Quick evaluation based only on test results.
    pub fn quick_evaluate(&self, test_results: &[TestResult]) -> EvaluationResult {
        if test_results.is_empty() {
            return EvaluationResult::failing(
                0.0,
                "No test results",
                vec!["Run tests first".into()],
            );
        }

        let passed = test_results.iter().filter(|r| r.passed).count();
        let total = test_results.len();
        let score = passed as f32 / total as f32;

        let feedback = format!("{}/{} tests passed", passed, total);

        let suggestions: Vec<String> = test_results
            .iter()
            .filter(|r| !r.passed)
            .filter_map(|r| {
                r.error
                    .as_ref()
                    .map(|e| format!("Fix {}: {}", r.test_name, e))
            })
            .collect();

        if score >= 0.8 {
            EvaluationResult::passing(score, feedback)
        } else {
            EvaluationResult::failing(score, feedback, suggestions)
        }
    }

    /// Build evaluation prompt.
    fn build_prompt(
        &self,
        spec: &Spec,
        _tests: &[TestCase],
        test_results: &[TestResult],
        implementation: &str,
    ) -> String {
        let criteria = spec
            .acceptance_criteria
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{}. {}", i + 1, c))
            .collect::<Vec<_>>()
            .join("\n");

        let test_summary = test_results
            .iter()
            .map(|r| {
                let status = if r.passed { "PASS" } else { "FAIL" };
                let error = r
                    .error
                    .as_ref()
                    .map(|e| format!(" - {}", e))
                    .unwrap_or_default();
                format!("[{}] {}{}", status, r.test_name, error)
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"Evaluate this implementation:

## Specification
Title: {}
Description: {}

Acceptance Criteria:
{}

## Test Results
{}

## Implementation
```
{}
```

Evaluate against the acceptance criteria and test results."#,
            spec.title, spec.description, criteria, test_summary, implementation
        )
    }

    /// Parse LLM response into evaluation result.
    fn parse_response(&self, response: &str) -> Result<EvaluationResult> {
        let json_str = extract_json(response);

        let parsed: EvaluationResponse = serde_json::from_str(&json_str).map_err(|e| {
            warn!(error = %e, response = %response, "Failed to parse evaluation");
            AlephError::Other {
                message: format!("Failed to parse evaluation: {}", e),
                suggestion: Some("Ensure the LLM returned valid JSON".to_string()),
            }
        })?;

        Ok(EvaluationResult {
            score: parsed.score.clamp(0.0, 1.0),
            criterion_scores: parsed.criterion_scores.unwrap_or_default(),
            feedback: parsed.feedback,
            suggestions: parsed.suggestions.unwrap_or_default(),
            is_acceptable: parsed.is_acceptable.unwrap_or(parsed.score >= 0.8),
        })
    }
}

/// Internal struct for parsing LLM response
#[derive(Debug, Deserialize)]
struct EvaluationResponse {
    score: f32,
    criterion_scores: Option<HashMap<String, f32>>,
    feedback: String,
    suggestions: Option<Vec<String>>,
    is_acceptable: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quick_evaluate_all_pass() {
        let results = vec![
            TestResult {
                test_name: "test1".into(),
                passed: true,
                actual_output: None,
                error: None,
                duration_ms: 10,
            },
            TestResult {
                test_name: "test2".into(),
                passed: true,
                actual_output: None,
                error: None,
                duration_ms: 20,
            },
        ];

        let judge = LlmJudge::new(Arc::new(MockProvider));
        let result = judge.quick_evaluate(&results);

        assert_eq!(result.score, 1.0);
        assert!(result.is_acceptable);
    }

    #[test]
    fn test_quick_evaluate_partial_pass() {
        let results = vec![
            TestResult {
                test_name: "test1".into(),
                passed: true,
                actual_output: None,
                error: None,
                duration_ms: 10,
            },
            TestResult {
                test_name: "test2".into(),
                passed: false,
                actual_output: None,
                error: Some("assertion failed".into()),
                duration_ms: 20,
            },
        ];

        let judge = LlmJudge::new(Arc::new(MockProvider));
        let result = judge.quick_evaluate(&results);

        assert_eq!(result.score, 0.5);
        assert!(!result.is_acceptable);
        assert!(!result.suggestions.is_empty());
    }

    #[test]
    fn test_quick_evaluate_empty() {
        let judge = LlmJudge::new(Arc::new(MockProvider));
        let result = judge.quick_evaluate(&[]);

        assert_eq!(result.score, 0.0);
        assert!(!result.is_acceptable);
    }

    struct MockProvider;

    impl crate::providers::AiProvider for MockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            Box::pin(async {
                Ok(r#"{"score": 0.9, "feedback": "Good", "is_acceptable": true}"#.to_string())
            })
        }

        fn process_with_thinking(
            &self,
            input: &str,
            system_prompt: Option<&str>,
            _level: ThinkLevel,
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
