//! SolidificationDetector - detects patterns ready for solidification.
//!
//! Analyzes execution metrics and generates solidification suggestions.

use crate::sync_primitives::Arc;


use crate::error::{AlephError, Result};
use crate::providers::AiProvider;

use super::tracker::EvolutionTracker;
use super::types::{SkillMetrics, SolidificationConfig, SolidificationSuggestion};

/// System prompt for generating skill suggestions
const SUGGESTION_SYSTEM_PROMPT: &str = r#"You are a skill extraction expert. Based on the execution metrics and sample contexts, generate a skill suggestion.

Output a JSON object:
{
  "suggested_name": "kebab-case-name",
  "suggested_description": "One sentence description",
  "instructions_preview": "Markdown instructions (2-3 paragraphs max)"
}

Rules:
- Name should be descriptive and kebab-case
- Description should explain what the skill does
- Instructions should be concise but complete
- Focus on the most common use cases from the contexts
- Output ONLY valid JSON, no markdown"#;

/// Detector for solidification candidates
pub struct SolidificationDetector {
    tracker: Arc<EvolutionTracker>,
    config: SolidificationConfig,
    provider: Option<Arc<dyn AiProvider>>,
}

impl SolidificationDetector {
    /// Create a new detector
    pub fn new(tracker: Arc<EvolutionTracker>) -> Self {
        Self {
            tracker,
            config: SolidificationConfig::default(),
            provider: None,
        }
    }

    /// Set configuration
    pub fn with_config(mut self, config: SolidificationConfig) -> Self {
        self.config = config;
        self
    }

    /// Set AI provider for generating suggestions
    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Detect all candidates ready for solidification
    pub fn detect_candidates(&self) -> Result<Vec<SkillMetrics>> {
        self.tracker.get_solidification_candidates(&self.config)
    }

    /// Generate a solidification suggestion for a candidate
    pub async fn generate_suggestion(
        &self,
        metrics: &SkillMetrics,
    ) -> Result<SolidificationSuggestion> {
        let sample_contexts: Vec<String> = metrics
            .context_frequency
            .iter()
            .take(5)
            .map(|(ctx, _)| ctx.clone())
            .collect();

        // If we have an AI provider, use it to generate better suggestions
        if let Some(provider) = &self.provider {
            let prompt = format!(
                r#"Generate a skill suggestion based on these metrics:

Pattern ID: {}
Total executions: {}
Success rate: {:.0}%
Sample contexts:
{}

Generate a skill that captures this pattern."#,
                metrics.skill_id,
                metrics.total_executions,
                metrics.success_rate() * 100.0,
                sample_contexts
                    .iter()
                    .map(|c| format!("- {}", c))
                    .collect::<Vec<_>>()
                    .join("\n")
            );

            let response = provider
                .process(&prompt, Some(SUGGESTION_SYSTEM_PROMPT))
                .await?;
            let parsed = parse_suggestion_response(&response, metrics, &sample_contexts)?;
            return Ok(parsed);
        }

        // Fallback: generate simple suggestion
        let suggested_name = generate_name_from_contexts(&sample_contexts);
        let suggested_description = format!(
            "Auto-generated skill from {} successful executions",
            metrics.successful_executions
        );

        Ok(SolidificationSuggestion {
            pattern_id: metrics.skill_id.clone(),
            suggested_name,
            suggested_description,
            confidence: metrics.success_rate(),
            metrics: metrics.clone(),
            sample_contexts,
            instructions_preview:
                "# Instructions\n\nThis skill was auto-generated from repeated successful patterns."
                    .to_string(),
        })
    }

    /// Check if any candidates exist
    pub fn has_candidates(&self) -> Result<bool> {
        let candidates = self.detect_candidates()?;
        Ok(!candidates.is_empty())
    }

    /// Get the configuration
    pub fn config(&self) -> &SolidificationConfig {
        &self.config
    }
}

/// Parse AI-generated suggestion response
fn parse_suggestion_response(
    response: &str,
    metrics: &SkillMetrics,
    sample_contexts: &[String],
) -> Result<SolidificationSuggestion> {
    use crate::spec_driven::spec_writer::extract_json;

    let json_str = extract_json(response);

    #[derive(serde::Deserialize)]
    struct SuggestionResponse {
        suggested_name: String,
        suggested_description: String,
        instructions_preview: String,
    }

    let parsed: SuggestionResponse = serde_json::from_str(&json_str).map_err(|e| {
        AlephError::Other {
            message: format!("Failed to parse suggestion: {}", e),
            suggestion: None,
        }
    })?;

    Ok(SolidificationSuggestion {
        pattern_id: metrics.skill_id.clone(),
        suggested_name: parsed.suggested_name,
        suggested_description: parsed.suggested_description,
        confidence: metrics.success_rate(),
        metrics: metrics.clone(),
        sample_contexts: sample_contexts.to_vec(),
        instructions_preview: parsed.instructions_preview,
    })
}

/// Generate a simple name from contexts
fn generate_name_from_contexts(contexts: &[String]) -> String {
    if contexts.is_empty() {
        return "auto-skill".to_string();
    }

    // Extract common words
    let first = contexts[0].to_lowercase();
    let words: Vec<&str> = first.split_whitespace().take(3).collect();

    words
        .join("-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_name_from_contexts() {
        let contexts = vec!["refactor authentication module".to_string()];
        let name = generate_name_from_contexts(&contexts);
        assert_eq!(name, "refactor-authentication-module");
    }

    #[test]
    fn test_generate_name_empty() {
        let contexts: Vec<String> = vec![];
        let name = generate_name_from_contexts(&contexts);
        assert_eq!(name, "auto-skill");
    }

    #[test]
    fn test_detector_creation() {
        let tracker = Arc::new(EvolutionTracker::in_memory().unwrap());
        let detector = SolidificationDetector::new(tracker);
        assert_eq!(detector.config().min_success_count, 3);
    }

    #[test]
    fn test_has_candidates_empty() {
        let tracker = Arc::new(EvolutionTracker::in_memory().unwrap());
        let detector = SolidificationDetector::new(tracker);
        assert!(!detector.has_candidates().unwrap());
    }
}
