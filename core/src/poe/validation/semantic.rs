//! Semantic validation for POE architecture.
//!
//! This module implements LLM-based semantic validation for soft metrics.
//! It uses the existing AiProvider abstraction to evaluate content against
//! natural language criteria.

use crate::agents::thinking::ThinkLevel;
use crate::error::Result;
use crate::poe::types::{JudgeTarget, ModelTier, SoftMetric, SoftRuleResult, ValidationRule};
use crate::providers::AiProvider;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// System prompt for the LLM judge.
const POE_JUDGE_SYSTEM_PROMPT: &str = r#"You are an impartial AI judge evaluating content against specific criteria.

Your job is to evaluate ONLY against the criteria given. Be objective and precise.

Output ONLY valid JSON in this exact format:
{
  "passed": true/false,
  "score": 0-100,
  "reason": "Brief explanation",
  "suggestion": "How to fix if failed (or null if passed)"
}

Output ONLY JSON, no markdown."#;

/// Default timeout for command execution (30 seconds).
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 30_000;

/// Response from the LLM judge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeResponse {
    /// Whether the content passed the criteria
    pub passed: bool,
    /// Score from 0-100
    pub score: u8,
    /// Brief explanation of the judgment
    pub reason: String,
    /// Suggestion for improvement (only if failed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// Semantic validator using LLM-based evaluation.
///
/// This validator uses an AI provider to evaluate content against
/// natural language criteria defined in soft metrics.
pub struct SemanticValidator {
    /// The AI provider to use for evaluation
    provider: Arc<dyn AiProvider>,
}

impl SemanticValidator {
    /// Create a new SemanticValidator with the given AI provider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }

    /// Validate all soft metrics and return results.
    ///
    /// Metrics are validated sequentially to respect rate limits
    /// and ensure consistent evaluation.
    pub async fn validate_all(&self, metrics: &[SoftMetric]) -> Result<Vec<SoftRuleResult>> {
        let mut results = Vec::with_capacity(metrics.len());

        for metric in metrics {
            results.push(self.validate_single(metric).await?);
        }

        Ok(results)
    }

    /// Validate a single soft metric and return the result.
    pub async fn validate_single(&self, metric: &SoftMetric) -> Result<SoftRuleResult> {
        // Only handle SemanticCheck rules
        let (target, prompt, passing_criteria, model_tier) = match &metric.rule {
            ValidationRule::SemanticCheck {
                target,
                prompt,
                passing_criteria,
                model_tier,
            } => (target, prompt, passing_criteria, *model_tier),
            // For non-semantic rules, return a neutral result
            _ => {
                return Ok(SoftRuleResult::new(metric.clone(), 1.0)
                    .with_feedback("Not a semantic check - skipped".to_string()));
            }
        };

        // Resolve the target content
        let content = match self.resolve_target(target).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(SoftRuleResult::new(metric.clone(), 0.0)
                    .with_feedback(format!("Failed to resolve target: {}", e)));
            }
        };

        // Build the evaluation prompt
        let eval_prompt = self.build_eval_prompt(&content, prompt, passing_criteria);

        // Get the thinking level based on model tier
        let think_level = self.model_tier_to_think_level(model_tier);

        // Call the LLM for evaluation
        let response = self
            .provider
            .process_with_thinking(&eval_prompt, Some(POE_JUDGE_SYSTEM_PROMPT), think_level)
            .await;

        // Parse the response
        match response {
            Ok(response_text) => {
                match self.parse_judge_response(&response_text) {
                    Ok(judge_response) => {
                        // Convert score to 0.0-1.0 range
                        let normalized_score = (judge_response.score as f32) / 100.0;

                        let mut result = SoftRuleResult::new(metric.clone(), normalized_score);

                        // Add feedback
                        let feedback = if let Some(suggestion) = &judge_response.suggestion {
                            format!("{} | Suggestion: {}", judge_response.reason, suggestion)
                        } else {
                            judge_response.reason.clone()
                        };
                        result = result.with_feedback(feedback);

                        Ok(result)
                    }
                    Err(parse_error) => {
                        // Failed to parse response - treat as evaluation failure
                        Ok(SoftRuleResult::new(metric.clone(), 0.5).with_feedback(format!(
                            "Failed to parse judge response: {}. Raw response: {}",
                            parse_error,
                            truncate_string(&response_text, 200)
                        )))
                    }
                }
            }
            Err(e) => {
                // LLM call failed - return error score with feedback
                Ok(SoftRuleResult::new(metric.clone(), 0.0)
                    .with_feedback(format!("LLM evaluation failed: {}", e)))
            }
        }
    }

    /// Resolve a JudgeTarget to its content string.
    async fn resolve_target(&self, target: &JudgeTarget) -> Result<String> {
        match target {
            JudgeTarget::File(path) => self.read_file_content(path).await,
            JudgeTarget::Content(content) => Ok(content.clone()),
            JudgeTarget::CommandOutput { cmd, args } => {
                self.run_command_and_capture(cmd, args).await
            }
        }
    }

    /// Read file content from the given path.
    async fn read_file_content(&self, path: &Path) -> Result<String> {
        if !path.exists() {
            return Err(crate::error::AlephError::IoError(format!(
                "File does not exist: {}",
                path.display()
            )));
        }

        tokio::fs::read_to_string(path).await.map_err(|e| {
            crate::error::AlephError::IoError(format!(
                "Failed to read file {}: {}",
                path.display(),
                e
            ))
        })
    }

    /// Run a command and capture its output.
    async fn run_command_and_capture(&self, cmd: &str, args: &[String]) -> Result<String> {
        let mut command = Command::new(cmd);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = command.spawn().map_err(|e| {
            crate::error::AlephError::IoError(format!("Failed to spawn command '{}': {}", cmd, e))
        })?;

        let duration = Duration::from_millis(DEFAULT_COMMAND_TIMEOUT_MS);

        match timeout(duration, child.wait_with_output()).await {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                Ok(format!("{}{}", stdout, stderr))
            }
            Ok(Err(e)) => Err(crate::error::AlephError::IoError(format!(
                "Command '{}' failed: {}",
                cmd, e
            ))),
            Err(_) => Err(crate::error::AlephError::IoError(format!(
                "Command '{}' timed out after {}ms",
                cmd, DEFAULT_COMMAND_TIMEOUT_MS
            ))),
        }
    }

    /// Build the evaluation prompt for the LLM judge.
    fn build_eval_prompt(&self, content: &str, prompt: &str, passing_criteria: &str) -> String {
        format!(
            r#"## Content to Evaluate

{}

## Evaluation Prompt

{}

## Passing Criteria

{}

Please evaluate the content against the criteria above and provide your judgment."#,
            truncate_string(content, 10000),
            prompt,
            passing_criteria
        )
    }

    /// Map ModelTier to ThinkLevel.
    fn model_tier_to_think_level(&self, tier: ModelTier) -> ThinkLevel {
        match tier {
            ModelTier::LocalFast | ModelTier::CloudFast => ThinkLevel::Off,
            ModelTier::CloudSmart => ThinkLevel::Low,
            ModelTier::CloudDeep => ThinkLevel::High,
        }
    }

    /// Parse the judge response from LLM output.
    ///
    /// This function handles responses that may be wrapped in markdown code blocks.
    fn parse_judge_response(&self, response: &str) -> std::result::Result<JudgeResponse, String> {
        let json_str = extract_json_from_response(response);

        serde_json::from_str(&json_str).map_err(|e| {
            format!(
                "JSON parse error: {}. Extracted JSON: {}",
                e,
                truncate_string(&json_str, 200)
            )
        })
    }
}

/// Extract JSON from a response that may have markdown wrappers.
///
/// Handles cases like:
/// - Pure JSON: `{"passed": true, ...}`
/// - Markdown code block: ```json\n{"passed": true, ...}\n```
/// - Markdown with language: ```\n{"passed": true, ...}\n```
fn extract_json_from_response(response: &str) -> String {
    let trimmed = response.trim();

    // Check for markdown code blocks
    if trimmed.starts_with("```") {
        // Find the end of the first line (language specifier)
        let start_idx = trimmed.find('\n').map(|i| i + 1).unwrap_or(3);

        // Find the closing ```
        if let Some(end_marker_idx) = trimmed.rfind("```") {
            if end_marker_idx > start_idx {
                return trimmed[start_idx..end_marker_idx].trim().to_string();
            }
        }

        // If no proper closing, take everything after the first line
        return trimmed[start_idx..].trim().to_string();
    }

    // Try to find JSON object boundaries
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end > start {
                return trimmed[start..=end].to_string();
            }
        }
    }

    // Return as-is if no JSON structure found
    trimmed.to_string()
}

/// Truncate a string to a maximum length, adding "..." if truncated.
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::MockProvider;
    use std::path::PathBuf;

    fn create_mock_validator(response: &str) -> SemanticValidator {
        let provider = Arc::new(MockProvider::new(response));
        SemanticValidator::new(provider)
    }

    #[test]
    fn test_extract_json_pure() {
        let response = r#"{"passed": true, "score": 85, "reason": "Good", "suggestion": null}"#;
        let extracted = extract_json_from_response(response);
        assert!(extracted.contains("\"passed\": true"));
    }

    #[test]
    fn test_extract_json_markdown_block() {
        let response = r#"```json
{"passed": true, "score": 85, "reason": "Good", "suggestion": null}
```"#;
        let extracted = extract_json_from_response(response);
        assert!(extracted.contains("\"passed\": true"));
        assert!(!extracted.contains("```"));
    }

    #[test]
    fn test_extract_json_markdown_no_language() {
        let response = r#"```
{"passed": false, "score": 30, "reason": "Bad", "suggestion": "Fix it"}
```"#;
        let extracted = extract_json_from_response(response);
        assert!(extracted.contains("\"passed\": false"));
    }

    #[test]
    fn test_extract_json_with_surrounding_text() {
        let response = r#"Here is my evaluation:
{"passed": true, "score": 90, "reason": "Excellent", "suggestion": null}
Thank you!"#;
        let extracted = extract_json_from_response(response);
        assert!(extracted.starts_with('{'));
        assert!(extracted.ends_with('}'));
    }

    #[test]
    fn test_parse_judge_response() {
        let validator = create_mock_validator("");
        let json = r#"{"passed": true, "score": 85, "reason": "Content is well structured", "suggestion": null}"#;

        let result = validator.parse_judge_response(json);
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert!(resp.passed);
        assert_eq!(resp.score, 85);
        assert_eq!(resp.reason, "Content is well structured");
        assert!(resp.suggestion.is_none());
    }

    #[test]
    fn test_parse_judge_response_with_suggestion() {
        let validator = create_mock_validator("");
        let json = r#"{"passed": false, "score": 40, "reason": "Missing documentation", "suggestion": "Add function comments"}"#;

        let result = validator.parse_judge_response(json);
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert!(!resp.passed);
        assert_eq!(resp.score, 40);
        assert_eq!(resp.suggestion.unwrap(), "Add function comments");
    }

    #[test]
    fn test_parse_judge_response_markdown_wrapped() {
        let validator = create_mock_validator("");
        let response = r#"```json
{"passed": true, "score": 100, "reason": "Perfect", "suggestion": null}
```"#;

        let result = validator.parse_judge_response(response);
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert!(resp.passed);
        assert_eq!(resp.score, 100);
    }

    #[test]
    fn test_model_tier_to_think_level() {
        let validator = create_mock_validator("");

        assert_eq!(
            validator.model_tier_to_think_level(ModelTier::LocalFast),
            ThinkLevel::Off
        );
        assert_eq!(
            validator.model_tier_to_think_level(ModelTier::CloudFast),
            ThinkLevel::Off
        );
        assert_eq!(
            validator.model_tier_to_think_level(ModelTier::CloudSmart),
            ThinkLevel::Low
        );
        assert_eq!(
            validator.model_tier_to_think_level(ModelTier::CloudDeep),
            ThinkLevel::High
        );
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 5), "hello...");
        assert_eq!(truncate_string("", 10), "");
    }

    #[test]
    fn test_build_eval_prompt() {
        let validator = create_mock_validator("");
        let prompt = validator.build_eval_prompt(
            "Test content here",
            "Is this good code?",
            "Must follow best practices",
        );

        assert!(prompt.contains("Test content here"));
        assert!(prompt.contains("Is this good code?"));
        assert!(prompt.contains("Must follow best practices"));
        assert!(prompt.contains("## Content to Evaluate"));
    }

    #[tokio::test]
    async fn test_validate_single_non_semantic() {
        let validator = create_mock_validator("");

        let metric = SoftMetric::new(ValidationRule::FileExists {
            path: PathBuf::from("/tmp/test.txt"),
        });

        let result = validator.validate_single(&metric).await.unwrap();
        assert_eq!(result.score, 1.0);
        assert!(result.feedback.as_ref().unwrap().contains("skipped"));
    }

    #[tokio::test]
    async fn test_validate_single_with_mock() {
        let mock_response =
            r#"{"passed": true, "score": 80, "reason": "Good quality", "suggestion": null}"#;
        let validator = create_mock_validator(mock_response);

        let metric = SoftMetric::new(ValidationRule::SemanticCheck {
            target: JudgeTarget::Content("Test content".to_string()),
            prompt: "Is this good?".to_string(),
            passing_criteria: "Must be good".to_string(),
            model_tier: ModelTier::CloudFast,
        });

        let result = validator.validate_single(&metric).await.unwrap();
        assert_eq!(result.score, 0.8); // 80/100
        assert!(result.feedback.as_ref().unwrap().contains("Good quality"));
    }

    #[tokio::test]
    async fn test_validate_single_with_suggestion() {
        let mock_response = r#"{"passed": false, "score": 50, "reason": "Needs improvement", "suggestion": "Add more details"}"#;
        let validator = create_mock_validator(mock_response);

        let metric = SoftMetric::new(ValidationRule::SemanticCheck {
            target: JudgeTarget::Content("Short content".to_string()),
            prompt: "Is this comprehensive?".to_string(),
            passing_criteria: "Must be thorough".to_string(),
            model_tier: ModelTier::CloudSmart,
        });

        let result = validator.validate_single(&metric).await.unwrap();
        assert_eq!(result.score, 0.5);
        let feedback = result.feedback.unwrap();
        assert!(feedback.contains("Needs improvement"));
        assert!(feedback.contains("Add more details"));
    }

    #[tokio::test]
    async fn test_validate_all() {
        let mock_response =
            r#"{"passed": true, "score": 90, "reason": "Excellent", "suggestion": null}"#;
        let validator = create_mock_validator(mock_response);

        let metrics = vec![
            SoftMetric::new(ValidationRule::SemanticCheck {
                target: JudgeTarget::Content("Content 1".to_string()),
                prompt: "Evaluate".to_string(),
                passing_criteria: "Good".to_string(),
                model_tier: ModelTier::CloudFast,
            }),
            SoftMetric::new(ValidationRule::SemanticCheck {
                target: JudgeTarget::Content("Content 2".to_string()),
                prompt: "Evaluate".to_string(),
                passing_criteria: "Good".to_string(),
                model_tier: ModelTier::CloudFast,
            }),
        ];

        let results = validator.validate_all(&metrics).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.score == 0.9));
    }

    #[tokio::test]
    async fn test_validate_file_target() {
        use tempfile::tempdir;
        use tokio::fs;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "This is test file content")
            .await
            .unwrap();

        let mock_response =
            r#"{"passed": true, "score": 95, "reason": "File looks good", "suggestion": null}"#;
        let validator = create_mock_validator(mock_response);

        let metric = SoftMetric::new(ValidationRule::SemanticCheck {
            target: JudgeTarget::File(file_path),
            prompt: "Evaluate file content".to_string(),
            passing_criteria: "Must be valid".to_string(),
            model_tier: ModelTier::CloudFast,
        });

        let result = validator.validate_single(&metric).await.unwrap();
        assert_eq!(result.score, 0.95);
    }

    #[tokio::test]
    async fn test_validate_command_target() {
        let mock_response = r#"{"passed": true, "score": 100, "reason": "Output is correct", "suggestion": null}"#;
        let validator = create_mock_validator(mock_response);

        let metric = SoftMetric::new(ValidationRule::SemanticCheck {
            target: JudgeTarget::CommandOutput {
                cmd: "echo".to_string(),
                args: vec!["Hello World".to_string()],
            },
            prompt: "Does the output say hello?".to_string(),
            passing_criteria: "Must greet the user".to_string(),
            model_tier: ModelTier::CloudFast,
        });

        let result = validator.validate_single(&metric).await.unwrap();
        assert_eq!(result.score, 1.0);
    }

    #[tokio::test]
    async fn test_validate_nonexistent_file() {
        let mock_response =
            r#"{"passed": true, "score": 100, "reason": "Good", "suggestion": null}"#;
        let validator = create_mock_validator(mock_response);

        let metric = SoftMetric::new(ValidationRule::SemanticCheck {
            target: JudgeTarget::File(PathBuf::from("/nonexistent/path/file.txt")),
            prompt: "Evaluate".to_string(),
            passing_criteria: "Good".to_string(),
            model_tier: ModelTier::CloudFast,
        });

        let result = validator.validate_single(&metric).await.unwrap();
        // Should fail to resolve target
        assert_eq!(result.score, 0.0);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Failed to resolve target"));
    }

    #[tokio::test]
    async fn test_invalid_json_response() {
        // Mock returns invalid JSON
        let mock_response = "This is not valid JSON at all";
        let validator = create_mock_validator(mock_response);

        let metric = SoftMetric::new(ValidationRule::SemanticCheck {
            target: JudgeTarget::Content("Test".to_string()),
            prompt: "Evaluate".to_string(),
            passing_criteria: "Good".to_string(),
            model_tier: ModelTier::CloudFast,
        });

        let result = validator.validate_single(&metric).await.unwrap();
        // Should return 0.5 for parse failures
        assert_eq!(result.score, 0.5);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Failed to parse"));
    }
}
