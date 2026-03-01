//! Composite validation for POE architecture.
//!
//! This module implements the two-phase validation pipeline that orchestrates
//! hard (deterministic) and semantic (LLM-based) validation.
//!
//! ## Validation Pipeline
//!
//! 1. **Phase 1: Hard Validation (Fast Fail)**
//!    - Execute all deterministic checks (file existence, regex, commands)
//!    - If ANY hard constraint fails, return immediately with failure verdict
//!    - This avoids wasting LLM resources on obviously failing outputs
//!
//! 2. **Phase 2: Semantic Validation (Quality Assessment)**
//!    - Only runs if all hard constraints pass
//!    - Evaluates soft metrics using LLM-based judgment
//!    - Calculates weighted quality score
//!    - Determines if soft metric thresholds are met

use crate::error::Result;
use crate::poe::types::{RuleResult, SoftRuleResult, SuccessManifest, Verdict, WorkerOutput};
use crate::poe::validation::{HardValidator, SemanticValidator};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// Composite validator that orchestrates hard and semantic validation.
///
/// The CompositeValidator implements a two-phase validation strategy:
/// - Phase 1: Hard validation (deterministic, fast fail)
/// - Phase 2: Semantic validation (LLM-based, quality assessment)
///
/// This approach optimizes validation by avoiding expensive LLM calls
/// when basic constraints aren't met.
pub struct CompositeValidator {
    /// Validator for deterministic checks
    hard_validator: HardValidator,
    /// Validator for LLM-based semantic checks
    semantic_validator: SemanticValidator,
}

impl CompositeValidator {
    /// Create a new CompositeValidator with the given AI provider.
    ///
    /// The provider is used by the SemanticValidator for LLM-based evaluation.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            hard_validator: HardValidator::new(),
            semantic_validator: SemanticValidator::new(provider),
        }
    }

    /// Validate worker output against the success manifest.
    ///
    /// ## Validation Phases
    ///
    /// 1. **Hard Validation (Fast Fail)**
    ///    - Runs all hard constraints from the manifest
    ///    - If any fail, returns immediately with a failure verdict
    ///    - Includes failure summary and fix suggestions
    ///
    /// 2. **Semantic Validation (Quality Assessment)**
    ///    - Only runs if all hard constraints pass
    ///    - Evaluates all soft metrics from the manifest
    ///    - Calculates weighted quality score
    ///    - Returns verdict with distance_score for entropy tracking
    ///
    /// ## Arguments
    ///
    /// * `manifest` - The success criteria definition
    /// * `_output` - The worker output (currently unused, reserved for future artifact validation)
    ///
    /// ## Returns
    ///
    /// A `Verdict` containing:
    /// - `passed`: Whether all criteria are met
    /// - `distance_score`: Distance from perfect success (0.0 = perfect, 1.0 = failure)
    /// - `reason`: Human-readable explanation
    /// - `suggestion`: Fix suggestions if failed
    /// - `hard_results`: Results from hard constraint validation
    /// - `soft_results`: Results from soft metric validation
    pub async fn validate(
        &self,
        manifest: &SuccessManifest,
        _output: &WorkerOutput,
    ) -> Result<Verdict> {
        // ========== Phase 1: Hard Validation ==========
        // Run all hard constraints first - fast fail if any fail
        let hard_results = self
            .hard_validator
            .validate_all(&manifest.hard_constraints)
            .await
            .map_err(crate::error::AlephError::other)?;

        // Check for hard failures
        let hard_failures: Vec<&RuleResult> =
            hard_results.iter().filter(|r| !r.passed).collect();

        if !hard_failures.is_empty() {
            // Fast fail: return immediately with hard failure verdict
            let reason = self.summarize_hard_failures(&hard_failures);
            let suggestion = self.suggest_hard_fix(&hard_failures);

            return Ok(Verdict::failure(reason)
                .with_suggestion(suggestion)
                .with_hard_results(hard_results)
                .with_distance_score(1.0)); // Maximum distance for hard failures
        }

        // ========== Phase 2: Semantic Validation ==========
        // Only run if all hard constraints pass
        let soft_results = self
            .semantic_validator
            .validate_all(&manifest.soft_metrics)
            .await?;

        // Calculate weighted soft score
        let (weighted_score, all_above_threshold) = self.calculate_soft_score(&soft_results);

        // Calculate distance_score for entropy tracking
        // 0.0 = perfect, 1.0 = complete failure
        let distance_score = 1.0 - weighted_score;

        // Determine overall pass/fail
        let passed = all_above_threshold;

        if passed {
            // Success verdict
            let reason = format!(
                "All {} hard constraints passed. Soft metric score: {:.1}%",
                manifest.hard_constraints.len(),
                weighted_score * 100.0
            );

            Ok(Verdict::success(reason)
                .with_hard_results(hard_results)
                .with_soft_results(soft_results)
                .with_distance_score(distance_score))
        } else {
            // Soft metrics failed threshold
            let reason = self.summarize_soft_failures(&soft_results);
            let suggestion = self.suggest_soft_fix(&soft_results);

            Ok(Verdict::failure(reason)
                .with_suggestion(suggestion)
                .with_hard_results(hard_results)
                .with_soft_results(soft_results)
                .with_distance_score(distance_score))
        }
    }

    // ========== Helper Methods ==========

    /// Summarize hard constraint failures into a human-readable message.
    fn summarize_hard_failures(&self, failures: &[&RuleResult]) -> String {
        if failures.is_empty() {
            return "No hard failures".to_string();
        }

        let failure_count = failures.len();
        let details: Vec<String> = failures
            .iter()
            .take(3) // Limit to first 3 for brevity
            .map(|r| {
                r.error
                    .clone()
                    .unwrap_or_else(|| "Unknown error".to_string())
            })
            .collect();

        let truncation_note = if failure_count > 3 {
            format!(" (+{} more)", failure_count - 3)
        } else {
            String::new()
        };

        format!(
            "{} hard constraint(s) failed: {}{}",
            failure_count,
            details.join("; "),
            truncation_note
        )
    }

    /// Generate fix suggestions for hard constraint failures.
    fn suggest_hard_fix(&self, failures: &[&RuleResult]) -> String {
        use crate::poe::types::ValidationRule;

        let suggestions: Vec<String> = failures
            .iter()
            .take(3) // Limit suggestions
            .filter_map(|r| match &r.rule {
                ValidationRule::FileExists { path } => {
                    Some(format!("Create file: {}", path.display()))
                }
                ValidationRule::FileNotExists { path } => {
                    Some(format!("Delete file: {}", path.display()))
                }
                ValidationRule::FileContains { path, pattern } => Some(format!(
                    "Ensure {} contains pattern matching: {}",
                    path.display(),
                    pattern
                )),
                ValidationRule::FileNotContains { path, pattern } => Some(format!(
                    "Remove content matching '{}' from {}",
                    pattern,
                    path.display()
                )),
                ValidationRule::DirStructureMatch { root, expected } => Some(format!(
                    "Create missing structure in {}: {}",
                    root.display(),
                    expected
                )),
                ValidationRule::CommandPasses { cmd, args, .. } => Some(format!(
                    "Fix command '{}' {} to exit with code 0",
                    cmd,
                    if args.is_empty() {
                        String::new()
                    } else {
                        format!("with args {:?}", args)
                    }
                )),
                ValidationRule::CommandOutputContains { cmd, pattern, .. } => Some(format!(
                    "Ensure command '{}' output contains: {}",
                    cmd, pattern
                )),
                ValidationRule::JsonSchemaValid { path, .. } => {
                    Some(format!("Fix JSON schema violations in {}", path.display()))
                }
                ValidationRule::SemanticCheck { .. } => {
                    // Semantic checks shouldn't appear in hard failures
                    None
                }
            })
            .collect();

        if suggestions.is_empty() {
            "Review the failed constraints and fix the underlying issues".to_string()
        } else {
            suggestions.join("; ")
        }
    }

    /// Calculate the weighted soft score and check if all metrics meet their thresholds.
    ///
    /// Returns a tuple of (weighted_score, all_above_threshold):
    /// - `weighted_score`: Value between 0.0 and 1.0 representing overall quality
    /// - `all_above_threshold`: Whether all individual metrics meet their thresholds
    fn calculate_soft_score(&self, results: &[SoftRuleResult]) -> (f32, bool) {
        if results.is_empty() {
            // No soft metrics = perfect score
            return (1.0, true);
        }

        let mut total_weight = 0.0f32;
        let mut weighted_sum = 0.0f32;
        let mut all_above_threshold = true;

        for result in results {
            let weight = result.metric.weight;
            let threshold = result.metric.threshold;
            let score = result.score;

            total_weight += weight;
            weighted_sum += score * weight;

            if score < threshold {
                all_above_threshold = false;
            }
        }

        // Calculate weighted average
        let weighted_score = if total_weight > 0.0 {
            weighted_sum / total_weight
        } else {
            1.0 // Default to perfect if no weights
        };

        (weighted_score.clamp(0.0, 1.0), all_above_threshold)
    }

    /// Summarize soft metric failures into a human-readable message.
    fn summarize_soft_failures(&self, results: &[SoftRuleResult]) -> String {
        let below_threshold: Vec<&SoftRuleResult> = results
            .iter()
            .filter(|r| r.score < r.metric.threshold)
            .collect();

        if below_threshold.is_empty() {
            return "All soft metrics passed".to_string();
        }

        let failure_details: Vec<String> = below_threshold
            .iter()
            .take(3)
            .map(|r| {
                format!(
                    "score {:.0}% < threshold {:.0}%{}",
                    r.score * 100.0,
                    r.metric.threshold * 100.0,
                    r.feedback
                        .as_ref()
                        .map(|f| format!(" ({})", truncate(f, 50)))
                        .unwrap_or_default()
                )
            })
            .collect();

        let truncation = if below_threshold.len() > 3 {
            format!(" (+{} more)", below_threshold.len() - 3)
        } else {
            String::new()
        };

        format!(
            "{} soft metric(s) below threshold: {}{}",
            below_threshold.len(),
            failure_details.join("; "),
            truncation
        )
    }

    /// Generate fix suggestions for soft metric failures.
    fn suggest_soft_fix(&self, results: &[SoftRuleResult]) -> String {
        let suggestions: Vec<String> = results
            .iter()
            .filter(|r| r.score < r.metric.threshold)
            .take(3)
            .filter_map(|r| {
                // Extract suggestion from feedback if present
                r.feedback.as_ref().map(|f| {
                    // Look for "Suggestion:" in feedback
                    if let Some(idx) = f.find("Suggestion:") {
                        f[idx + 11..].trim().to_string()
                    } else {
                        // Use the whole feedback if no explicit suggestion
                        format!("Improve: {}", truncate(f, 100))
                    }
                })
            })
            .collect();

        if suggestions.is_empty() {
            "Review semantic feedback and improve content quality".to_string()
        } else {
            suggestions.join("; ")
        }
    }
}

/// Truncate a string to a maximum length.
fn truncate(s: &str, max_len: usize) -> String {
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
    use crate::poe::types::{JudgeTarget, ModelTier, SoftMetric, ValidationRule, WorkerOutput};
    use crate::providers::MockProvider;
    use std::path::PathBuf;

    fn create_test_validator(mock_response: &str) -> CompositeValidator {
        let provider = Arc::new(MockProvider::new(mock_response));
        CompositeValidator::new(provider)
    }

    fn create_basic_manifest() -> SuccessManifest {
        SuccessManifest::new("test-task", "Test objective")
    }

    fn create_basic_output() -> WorkerOutput {
        WorkerOutput::completed("Task completed")
    }

    #[tokio::test]
    async fn test_empty_manifest_passes() {
        let validator = create_test_validator("");
        let manifest = create_basic_manifest();
        let output = create_basic_output();

        let verdict = validator.validate(&manifest, &output).await.unwrap();

        assert!(verdict.passed);
        assert_eq!(verdict.distance_score, 0.0);
        assert!(verdict.hard_results.is_empty());
        assert!(verdict.soft_results.is_empty());
    }

    #[tokio::test]
    async fn test_hard_failure_fast_fail() {
        let validator = create_test_validator("");
        let manifest = SuccessManifest::new("test-task", "Test objective")
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("/nonexistent/file.txt"),
            });
        let output = create_basic_output();

        let verdict = validator.validate(&manifest, &output).await.unwrap();

        assert!(!verdict.passed);
        assert_eq!(verdict.distance_score, 1.0);
        assert_eq!(verdict.hard_results.len(), 1);
        assert!(!verdict.hard_results[0].passed);
        assert!(verdict.reason.contains("hard constraint"));
        assert!(verdict.soft_results.is_empty()); // Semantic validation skipped
    }

    #[tokio::test]
    async fn test_hard_passes_semantic_runs() {
        use tempfile::tempdir;
        use tokio::fs;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "Hello World").await.unwrap();

        let mock_response =
            r#"{"passed": true, "score": 90, "reason": "Good quality", "suggestion": null}"#;
        let validator = create_test_validator(mock_response);

        let manifest = SuccessManifest::new("test-task", "Test objective")
            .with_hard_constraint(ValidationRule::FileExists { path: file_path })
            .with_soft_metric(SoftMetric::new(ValidationRule::SemanticCheck {
                target: JudgeTarget::Content("Test content".to_string()),
                prompt: "Is this good?".to_string(),
                passing_criteria: "Must be good".to_string(),
                model_tier: ModelTier::CloudFast,
            }));

        let output = create_basic_output();
        let verdict = validator.validate(&manifest, &output).await.unwrap();

        assert!(verdict.passed);
        assert!(verdict.distance_score < 0.2); // High quality
        assert_eq!(verdict.hard_results.len(), 1);
        assert!(verdict.hard_results[0].passed);
        assert_eq!(verdict.soft_results.len(), 1);
    }

    #[tokio::test]
    async fn test_soft_below_threshold_fails() {
        let mock_response =
            r#"{"passed": false, "score": 50, "reason": "Low quality", "suggestion": "Improve it"}"#;
        let validator = create_test_validator(mock_response);

        let manifest = SuccessManifest::new("test-task", "Test objective").with_soft_metric(
            SoftMetric::new(ValidationRule::SemanticCheck {
                target: JudgeTarget::Content("Test content".to_string()),
                prompt: "Is this good?".to_string(),
                passing_criteria: "Must be good".to_string(),
                model_tier: ModelTier::CloudFast,
            })
            .with_threshold(0.8), // 80% threshold
        );

        let output = create_basic_output();
        let verdict = validator.validate(&manifest, &output).await.unwrap();

        assert!(!verdict.passed);
        assert_eq!(verdict.distance_score, 0.5); // 1.0 - 0.5
        assert!(verdict.soft_results.len() == 1);
        assert!(verdict.reason.contains("below threshold"));
    }

    #[tokio::test]
    async fn test_weighted_score_calculation() {
        let validator = create_test_validator("");

        // Create results with different weights
        let results = vec![
            SoftRuleResult::new(
                SoftMetric::new(ValidationRule::FileExists {
                    path: PathBuf::from("a.txt"),
                })
                .with_weight(0.7)
                .with_threshold(0.5),
                0.9, // 90% score
            ),
            SoftRuleResult::new(
                SoftMetric::new(ValidationRule::FileExists {
                    path: PathBuf::from("b.txt"),
                })
                .with_weight(0.3)
                .with_threshold(0.5),
                0.6, // 60% score
            ),
        ];

        let (score, all_above) = validator.calculate_soft_score(&results);

        // Weighted average: (0.9 * 0.7 + 0.6 * 0.3) / (0.7 + 0.3) = 0.81
        assert!((score - 0.81).abs() < 0.01);
        assert!(all_above);
    }

    #[tokio::test]
    async fn test_empty_soft_metrics_perfect_score() {
        let validator = create_test_validator("");
        let (score, all_above) = validator.calculate_soft_score(&[]);

        assert_eq!(score, 1.0);
        assert!(all_above);
    }

    #[test]
    fn test_summarize_hard_failures() {
        let validator = create_test_validator("");

        let failures = [
            RuleResult::fail(
                ValidationRule::FileExists {
                    path: PathBuf::from("/a.txt"),
                },
                "File not found: /a.txt",
            ),
            RuleResult::fail(
                ValidationRule::CommandPasses {
                    cmd: "test".to_string(),
                    args: vec![],
                    timeout_ms: 1000,
                },
                "Command failed with exit code 1",
            ),
        ];

        let failure_refs: Vec<&RuleResult> = failures.iter().collect();
        let summary = validator.summarize_hard_failures(&failure_refs);

        assert!(summary.contains("2 hard constraint(s) failed"));
        assert!(summary.contains("File not found"));
    }

    #[test]
    fn test_suggest_hard_fix() {
        let validator = create_test_validator("");

        let failures = [
            RuleResult::fail(
                ValidationRule::FileExists {
                    path: PathBuf::from("/missing.txt"),
                },
                "Not found",
            ),
            RuleResult::fail(
                ValidationRule::FileContains {
                    path: PathBuf::from("/file.txt"),
                    pattern: "hello".to_string(),
                },
                "Pattern not found",
            ),
        ];

        let failure_refs: Vec<&RuleResult> = failures.iter().collect();
        let suggestion = validator.suggest_hard_fix(&failure_refs);

        assert!(suggestion.contains("Create file:"));
        assert!(suggestion.contains("Ensure"));
    }

    #[test]
    fn test_summarize_soft_failures() {
        let validator = create_test_validator("");

        let results = vec![
            SoftRuleResult::new(
                SoftMetric::new(ValidationRule::FileExists {
                    path: PathBuf::from("a.txt"),
                })
                .with_threshold(0.8),
                0.5, // Below threshold
            )
            .with_feedback("Quality is low"),
            SoftRuleResult::new(
                SoftMetric::new(ValidationRule::FileExists {
                    path: PathBuf::from("b.txt"),
                })
                .with_threshold(0.6),
                0.9, // Above threshold
            ),
        ];

        let summary = validator.summarize_soft_failures(&results);

        assert!(summary.contains("1 soft metric(s) below threshold"));
        assert!(summary.contains("50%"));
        assert!(summary.contains("80%"));
    }

    #[test]
    fn test_suggest_soft_fix_with_explicit_suggestion() {
        let validator = create_test_validator("");

        let results = vec![SoftRuleResult::new(
            SoftMetric::new(ValidationRule::FileExists {
                path: PathBuf::from("a.txt"),
            })
            .with_threshold(0.8),
            0.5,
        )
        .with_feedback("Low quality | Suggestion: Add more documentation")];

        let suggestion = validator.suggest_soft_fix(&results);

        assert!(suggestion.contains("Add more documentation"));
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world test", 10), "hello worl...");
        assert_eq!(truncate("", 5), "");
    }

    #[tokio::test]
    async fn test_multiple_hard_failures() {
        let validator = create_test_validator("");

        let manifest = SuccessManifest::new("test-task", "Test objective")
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("/missing1.txt"),
            })
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("/missing2.txt"),
            })
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("/missing3.txt"),
            })
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("/missing4.txt"),
            });

        let output = create_basic_output();
        let verdict = validator.validate(&manifest, &output).await.unwrap();

        assert!(!verdict.passed);
        assert!(verdict.reason.contains("4 hard constraint(s) failed"));
        assert!(verdict.reason.contains("+1 more")); // Truncated
    }

    #[tokio::test]
    async fn test_distance_score_reflects_quality() {
        let mock_response =
            r#"{"passed": true, "score": 75, "reason": "Decent", "suggestion": null}"#;
        let validator = create_test_validator(mock_response);

        let manifest = SuccessManifest::new("test-task", "Test objective").with_soft_metric(
            SoftMetric::new(ValidationRule::SemanticCheck {
                target: JudgeTarget::Content("Content".to_string()),
                prompt: "Evaluate".to_string(),
                passing_criteria: "Good".to_string(),
                model_tier: ModelTier::CloudFast,
            })
            .with_threshold(0.7), // 70% threshold - will pass
        );

        let output = create_basic_output();
        let verdict = validator.validate(&manifest, &output).await.unwrap();

        assert!(verdict.passed);
        assert!((verdict.distance_score - 0.25).abs() < 0.01); // 1.0 - 0.75 = 0.25
    }
}
