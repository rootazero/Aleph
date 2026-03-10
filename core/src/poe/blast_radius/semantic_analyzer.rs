//! System 2: LLM-based semantic risk analyzer for gray-zone tasks.
//!
//! When StaticSafetyScanner returns Indeterminate, this analyzer uses an
//! LLM to assess risk based on the full task context. Falls back to
//! "presumption of guilt" (High risk) on any LLM or parse failure.

use crate::poe::types::{BlastRadius, RiskLevel, SuccessManifest};
use crate::providers::AiProvider;
use serde::Deserialize;

// ============================================================================
// Semantic Risk Analyzer
// ============================================================================

/// LLM-based risk analyzer for gray-zone tasks that System 1 cannot classify.
///
/// Follows a "presumption of guilt" principle: any failure in LLM communication
/// or response parsing results in High risk, never a downgrade.
pub struct SemanticRiskAnalyzer;

/// Intermediate struct for parsing LLM JSON responses.
#[derive(Debug, Deserialize)]
struct LlmRiskResponse {
    scope: f32,
    destructiveness: f32,
    reversibility: f32,
    level: String,
    reasoning: String,
}

impl SemanticRiskAnalyzer {
    /// Analyze a manifest using an LLM provider. Falls back to High on error.
    pub async fn analyze(
        manifest: &SuccessManifest,
        provider: &dyn AiProvider,
    ) -> BlastRadius {
        let prompt = Self::build_prompt(manifest);

        let response = provider
            .process(&prompt, Some(SYSTEM_PROMPT))
            .await;

        match response {
            Ok(text) => {
                Self::parse_llm_response(&text).unwrap_or_else(|e| {
                    Self::fallback_blast_radius(format!("LLM response parse failed: {}", e))
                })
            }
            Err(e) => {
                Self::fallback_blast_radius(format!("LLM call failed: {}", e))
            }
        }
    }

    /// Build the risk assessment prompt from a manifest.
    pub fn build_prompt(manifest: &SuccessManifest) -> String {
        let constraints_json = serde_json::to_string_pretty(&manifest.hard_constraints)
            .unwrap_or_else(|_| "[]".to_string());
        let metrics_json = serde_json::to_string_pretty(&manifest.soft_metrics)
            .unwrap_or_else(|_| "[]".to_string());

        format!(
            r#"Assess the risk of the following task:

Task ID: {}
Objective: {}

Hard Constraints:
{}

Soft Metrics:
{}

Respond with ONLY a JSON object (no markdown fences) with these fields:
- "scope": float 0.0-1.0 (impact scope: file count, module depth, user coverage)
- "destructiveness": float 0.0-1.0 (data deletion, prod config changes)
- "reversibility": float 0.0-1.0 (1.0 = fully reversible, 0.0 = irreversible)
- "level": one of "negligible", "low", "medium", "high", "critical"
- "reasoning": brief explanation of your assessment"#,
            manifest.task_id,
            manifest.objective,
            constraints_json,
            metrics_json,
        )
    }

    /// Parse the LLM response JSON into a BlastRadius.
    pub fn parse_llm_response(response: &str) -> Result<BlastRadius, String> {
        let json_str = Self::extract_json(response);

        let parsed: LlmRiskResponse = serde_json::from_str(json_str)
            .map_err(|e| format!("JSON parse error: {}", e))?;

        let level = match parsed.level.to_lowercase().as_str() {
            "negligible" => RiskLevel::Negligible,
            "low" => RiskLevel::Low,
            "medium" => RiskLevel::Medium,
            "high" => RiskLevel::High,
            "critical" => RiskLevel::Critical,
            other => return Err(format!("Unknown risk level: {}", other)),
        };

        Ok(BlastRadius::new(
            parsed.scope,
            parsed.destructiveness,
            parsed.reversibility,
            level,
            parsed.reasoning,
        ))
    }

    /// Extract JSON from LLM output, stripping markdown code fences if present.
    pub fn extract_json(response: &str) -> &str {
        let trimmed = response.trim();

        // Strip ```json ... ``` or ``` ... ```
        if let Some(rest) = trimmed.strip_prefix("```json") {
            if let Some(json) = rest.strip_suffix("```") {
                return json.trim();
            }
        }
        if let Some(rest) = trimmed.strip_prefix("```") {
            if let Some(json) = rest.strip_suffix("```") {
                return json.trim();
            }
        }

        trimmed
    }

    /// Fallback blast radius: "presumption of guilt" returns High risk.
    pub fn fallback_blast_radius(reason: impl Into<String>) -> BlastRadius {
        BlastRadius::new(
            0.5,
            0.5,
            0.5,
            RiskLevel::High,
            format!("Fallback (presumption of guilt): {}", reason.into()),
        )
    }

    /// Merge System 1 and System 2 results. System 1 is NEVER downgraded by System 2.
    ///
    /// If System 1 classified higher than System 2, keep System 1's level.
    /// If System 2 classified higher, use System 2's assessment.
    /// Numeric dimensions (scope, destructiveness, reversibility) take the
    /// more conservative value (max scope/destructiveness, min reversibility).
    pub fn merge_with_system1(
        system1: Option<&BlastRadius>,
        system2: BlastRadius,
    ) -> BlastRadius {
        let Some(s1) = system1 else {
            return system2;
        };

        let level = std::cmp::max(s1.level, system2.level);
        let scope = s1.scope.max(system2.scope);
        let destructiveness = s1.destructiveness.max(system2.destructiveness);
        let reversibility = s1.reversibility.min(system2.reversibility);

        let reasoning = if s1.level > system2.level {
            format!(
                "System 1 ({:?}) overrides System 2 ({:?}): {}",
                s1.level, system2.level, s1.reasoning
            )
        } else if system2.level > s1.level {
            format!(
                "System 2 escalated from {:?} to {:?}: {}",
                s1.level, system2.level, system2.reasoning
            )
        } else {
            format!(
                "System 1 and 2 agree ({:?}): {}",
                system2.level, system2.reasoning
            )
        };

        BlastRadius::new(scope, destructiveness, reversibility, level, reasoning)
    }
}

const SYSTEM_PROMPT: &str = "You are a security risk assessor for an AI agent system. \
Analyze the given task and its constraints to determine the blast radius (potential impact). \
Be conservative: when uncertain, rate higher risk. Respond with JSON only.";

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_on_parse_failure() {
        let result = SemanticRiskAnalyzer::parse_llm_response("not json at all");
        assert!(result.is_err());

        let fallback = SemanticRiskAnalyzer::fallback_blast_radius("test failure");
        assert_eq!(fallback.level, RiskLevel::High);
        assert!(fallback.reasoning.contains("presumption of guilt"));
    }

    #[test]
    fn test_parse_llm_response_valid() {
        let json = r#"{
            "scope": 0.3,
            "destructiveness": 0.2,
            "reversibility": 0.9,
            "level": "low",
            "reasoning": "Simple file operation"
        }"#;

        let result = SemanticRiskAnalyzer::parse_llm_response(json).unwrap();
        assert_eq!(result.level, RiskLevel::Low);
        assert!((result.scope - 0.3).abs() < f32::EPSILON);
        assert!((result.destructiveness - 0.2).abs() < f32::EPSILON);
        assert!((result.reversibility - 0.9).abs() < f32::EPSILON);
        assert_eq!(result.reasoning, "Simple file operation");
    }

    #[test]
    fn test_parse_llm_response_with_code_fence() {
        let json = r#"```json
{
    "scope": 0.5,
    "destructiveness": 0.7,
    "reversibility": 0.3,
    "level": "high",
    "reasoning": "Modifies core config"
}
```"#;

        let result = SemanticRiskAnalyzer::parse_llm_response(json).unwrap();
        assert_eq!(result.level, RiskLevel::High);
    }

    #[test]
    fn test_parse_llm_response_invalid() {
        let cases = vec![
            "{}",                          // missing fields
            r#"{"scope": "not a number"}"#, // wrong type
            "random text",                  // not JSON
            "",                            // empty
        ];

        for case in cases {
            let result = SemanticRiskAnalyzer::parse_llm_response(case);
            assert!(result.is_err(), "Expected error for input: {}", case);
        }
    }

    #[test]
    fn test_parse_llm_response_unknown_level() {
        let json = r#"{
            "scope": 0.5,
            "destructiveness": 0.5,
            "reversibility": 0.5,
            "level": "extreme",
            "reasoning": "test"
        }"#;

        let result = SemanticRiskAnalyzer::parse_llm_response(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown risk level"));
    }

    #[test]
    fn test_system1_never_downgraded() {
        let system1 = BlastRadius::new(
            0.8, 0.9, 0.2,
            RiskLevel::Critical,
            "System 1: force push detected",
        );

        let system2 = BlastRadius::new(
            0.3, 0.2, 0.8,
            RiskLevel::Low,
            "System 2: seems fine",
        );

        let merged = SemanticRiskAnalyzer::merge_with_system1(Some(&system1), system2);

        // System 1's Critical must not be downgraded to System 2's Low
        assert_eq!(merged.level, RiskLevel::Critical);
        // Conservative: max scope, max destructiveness, min reversibility
        assert!((merged.scope - 0.8).abs() < f32::EPSILON);
        assert!((merged.destructiveness - 0.9).abs() < f32::EPSILON);
        assert!((merged.reversibility - 0.2).abs() < f32::EPSILON);
        assert!(merged.reasoning.contains("System 1"));
    }

    #[test]
    fn test_system2_can_escalate() {
        let system1 = BlastRadius::new(
            0.2, 0.1, 0.9,
            RiskLevel::Low,
            "System 1: single constraint",
        );

        let system2 = BlastRadius::new(
            0.7, 0.8, 0.3,
            RiskLevel::High,
            "System 2: database migration detected",
        );

        let merged = SemanticRiskAnalyzer::merge_with_system1(Some(&system1), system2);

        // System 2 can escalate
        assert_eq!(merged.level, RiskLevel::High);
        assert!(merged.reasoning.contains("System 2 escalated"));
    }

    #[test]
    fn test_no_system1_uses_system2() {
        let system2 = BlastRadius::new(
            0.4, 0.3, 0.7,
            RiskLevel::Medium,
            "LLM assessed medium risk",
        );

        let merged = SemanticRiskAnalyzer::merge_with_system1(None, system2);
        assert_eq!(merged.level, RiskLevel::Medium);
        assert_eq!(merged.reasoning, "LLM assessed medium risk");
    }

    #[test]
    fn test_extract_json_plain() {
        let input = r#"{"key": "value"}"#;
        assert_eq!(SemanticRiskAnalyzer::extract_json(input), input);
    }

    #[test]
    fn test_extract_json_with_fences() {
        let input = "```json\n{\"key\": \"value\"}\n```";
        assert_eq!(
            SemanticRiskAnalyzer::extract_json(input),
            "{\"key\": \"value\"}"
        );
    }

    #[test]
    fn test_build_prompt_contains_task_info() {
        let manifest = SuccessManifest::new("task-42", "Deploy to production");
        let prompt = SemanticRiskAnalyzer::build_prompt(&manifest);
        assert!(prompt.contains("task-42"));
        assert!(prompt.contains("Deploy to production"));
        assert!(prompt.contains("scope"));
        assert!(prompt.contains("destructiveness"));
        assert!(prompt.contains("reversibility"));
    }
}
