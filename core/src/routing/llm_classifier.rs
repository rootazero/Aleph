//! LLM-based task classification — prompt builder and response parser.
//!
//! This module does NOT make actual LLM calls. It provides:
//! - `build_classify_prompt` — constructs the classification prompt
//! - `parse_classify_response` — parses the LLM JSON response into a `TaskRoute`

use serde::Deserialize;

use super::task_router::{CollabStrategy, ManifestHints, TaskRoute};

/// Build a classification prompt for the given user message.
pub fn build_classify_prompt(message: &str) -> String {
    format!(
        r#"You are a task routing classifier. Analyze the following user message and classify it into one of these categories:

- "simple": straightforward questions, greetings, translations
- "multi_step": tasks requiring multiple sequential steps
- "critical": tasks requiring high-quality output with verification (reports, analysis, audits)
- "collaborative": tasks benefiting from multiple agents or perspectives

Respond with a JSON object:
{{
  "category": "<simple|multi_step|critical|collaborative>",
  "reason": "<brief explanation>",
  "hard_constraints": ["<optional constraint list for critical tasks>"],
  "quality_threshold": <0.0-1.0, only for critical>,
  "collab_strategy": "<parallel|adversarial|group_chat, only for collaborative>"
}}

User message: {message}"#
    )
}

/// Internal deserialization target for LLM responses.
#[derive(Deserialize)]
struct ClassifyResponse {
    category: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    hard_constraints: Vec<String>,
    #[serde(default = "default_quality_threshold")]
    quality_threshold: f64,
    #[serde(default)]
    collab_strategy: Option<String>,
}

fn default_quality_threshold() -> f64 {
    0.7
}

/// Parse an LLM classification response into a `TaskRoute`.
///
/// Handles markdown-wrapped JSON (```json ... ```).
/// Falls back to `TaskRoute::Simple` on parse failure.
pub fn parse_classify_response(response: &str) -> TaskRoute {
    let json_str = extract_json(response);

    let parsed: ClassifyResponse = match serde_json::from_str(json_str) {
        Ok(r) => r,
        Err(_) => return TaskRoute::Simple,
    };

    match parsed.category.as_str() {
        "critical" => TaskRoute::Critical {
            reason: parsed.reason,
            manifest_hints: ManifestHints {
                hard_constraints: parsed.hard_constraints,
                quality_threshold: parsed.quality_threshold,
            },
        },
        "multi_step" => TaskRoute::MultiStep {
            reason: parsed.reason,
        },
        "collaborative" => {
            let strategy = match parsed.collab_strategy.as_deref() {
                Some("adversarial") => CollabStrategy::Adversarial,
                Some("group_chat") => CollabStrategy::GroupChat,
                _ => CollabStrategy::Parallel,
            };
            TaskRoute::Collaborative {
                reason: parsed.reason,
                strategy,
            }
        }
        _ => TaskRoute::Simple,
    }
}

/// Extract JSON content from potentially markdown-wrapped text.
fn extract_json(text: &str) -> &str {
    let trimmed = text.trim();

    // Handle ```json ... ``` wrapping
    if let Some(start) = trimmed.find("```") {
        let after_backticks = &trimmed[start + 3..];
        // Skip optional language identifier (e.g., "json")
        let json_start = after_backticks
            .find('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let content = &after_backticks[json_start..];
        if let Some(end) = content.find("```") {
            return content[..end].trim();
        }
    }

    // Try to find raw JSON object
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            return &trimmed[start..=end];
        }
    }

    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let response = r#"{"category": "simple", "reason": "greeting"}"#;
        let route = parse_classify_response(response);
        assert_eq!(route.label(), "simple");
    }

    #[test]
    fn parse_critical_with_constraints() {
        let response = r#"{
            "category": "critical",
            "reason": "needs audit",
            "hard_constraints": ["must include sources"],
            "quality_threshold": 0.9
        }"#;
        let route = parse_classify_response(response);
        assert_eq!(route.label(), "critical");
        if let TaskRoute::Critical { manifest_hints, .. } = &route {
            assert_eq!(manifest_hints.hard_constraints.len(), 1);
            assert!((manifest_hints.quality_threshold - 0.9).abs() < f64::EPSILON);
        } else {
            panic!("expected Critical");
        }
    }

    #[test]
    fn parse_collaborative() {
        let response =
            r#"{"category": "collaborative", "reason": "debate", "collab_strategy": "adversarial"}"#;
        let route = parse_classify_response(response);
        assert_eq!(route.label(), "collaborative");
        if let TaskRoute::Collaborative { strategy, .. } = &route {
            assert!(matches!(strategy, CollabStrategy::Adversarial));
        } else {
            panic!("expected Collaborative");
        }
    }

    #[test]
    fn parse_markdown_wrapped() {
        let response = r#"Here is my analysis:
```json
{"category": "multi_step", "reason": "sequential tasks"}
```
"#;
        let route = parse_classify_response(response);
        assert_eq!(route.label(), "multi_step");
    }

    #[test]
    fn invalid_defaults_to_simple() {
        let route = parse_classify_response("this is not json at all");
        assert_eq!(route.label(), "simple");
    }

    #[test]
    fn prompt_contains_message() {
        let prompt = build_classify_prompt("hello world");
        assert!(prompt.contains("hello world"));
        assert!(prompt.contains("simple"));
        assert!(prompt.contains("critical"));
    }
}
