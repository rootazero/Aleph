//! Binary intent classifier using LLM.
//!
//! Determines whether a user message requires tool execution ("execute")
//! or is pure conversation ("converse"). Language-agnostic by design.

use crate::intent::types::{DetectionLayer, ExecuteMetadata, IntentResult};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use serde::Deserialize;
use std::time::Duration;
use tracing::warn;

/// System prompt for binary intent classification.
const SYSTEM_PROMPT: &str = r#"You are an intent classifier. Given a user message, determine if it requires
tool execution or is pure conversation.

Respond with JSON only:
{"intent": "execute" | "converse", "confidence": 0.0-1.0}

Guidelines:
- "execute": user wants to perform an action (file operations, code execution,
  web search, image generation, system commands, downloads, etc.)
- "converse": user wants information, explanation, analysis, creative writing,
  translation, or general chat

Examples:
- "organize my downloads folder" → execute
- "what is quantum computing" → converse
- "run the test suite" → execute
- "explain this error message" → converse
- "search for flights to Tokyo" → execute
- "write me a poem about rain" → converse"#;

/// Configuration for the binary classifier.
#[derive(Debug, Clone)]
pub struct AiBinaryConfig {
    /// Minimum character count (by `chars().count()`) to attempt classification.
    pub min_input_length: usize,
    /// Timeout for the LLM call.
    pub timeout: Duration,
    /// Minimum confidence to accept a classification result.
    pub confidence_threshold: f32,
}

impl Default for AiBinaryConfig {
    fn default() -> Self {
        Self {
            min_input_length: 8,
            timeout: Duration::from_secs(3),
            confidence_threshold: 0.6,
        }
    }
}

/// Binary intent classifier backed by an LLM provider.
///
/// Returns `Some(IntentResult)` when classification succeeds with sufficient
/// confidence, or `None` when the input is too short, the LLM times out,
/// or confidence is below threshold.
pub struct AiBinaryClassifier {
    provider: Arc<dyn AiProvider>,
    config: AiBinaryConfig,
}

/// Internal deserialization target for the LLM JSON response.
#[derive(Deserialize)]
struct AiResponse {
    intent: String,
    confidence: f32,
}

impl AiBinaryClassifier {
    /// Create a new binary classifier with the given provider and config.
    pub fn new(provider: Arc<dyn AiProvider>, config: AiBinaryConfig) -> Self {
        Self { provider, config }
    }

    /// Classify user input as execute or converse.
    ///
    /// Returns `None` when:
    /// - The input is shorter than `min_input_length` characters
    /// - The LLM call times out or errors
    /// - The response cannot be parsed
    /// - Confidence is below the configured threshold
    pub async fn classify(&self, input: &str) -> Option<IntentResult> {
        if input.chars().count() < self.config.min_input_length {
            return None;
        }

        let combined = format!(
            "[TASK: Intent Classification - Return JSON ONLY]\n\n{}\n\n---\nUser message: {}",
            SYSTEM_PROMPT, input
        );

        let response = match tokio::time::timeout(
            self.config.timeout,
            self.provider.process(&combined, None),
        )
        .await
        {
            Ok(Ok(text)) => text,
            Ok(Err(e)) => {
                warn!(error = %e, "AI binary classifier provider error");
                return None;
            }
            Err(_) => {
                warn!(
                    timeout_ms = self.config.timeout.as_millis() as u64,
                    "AI binary classifier timed out"
                );
                return None;
            }
        };

        self.parse_response(&response)
    }

    /// Parse the LLM response into an `IntentResult`.
    fn parse_response(&self, response: &str) -> Option<IntentResult> {
        let json_str = extract_json(response)?;

        let ai: AiResponse = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, response = %response, "Failed to parse AI binary response");
                return None;
            }
        };

        if ai.confidence < self.config.confidence_threshold {
            return None;
        }

        match ai.intent.as_str() {
            "execute" => Some(IntentResult::Execute {
                confidence: ai.confidence,
                metadata: ExecuteMetadata::default_with_layer(DetectionLayer::L3),
            }),
            "converse" => Some(IntentResult::Converse {
                confidence: ai.confidence,
            }),
            _ => None,
        }
    }
}

/// Extract a JSON object from text that may contain surrounding prose or
/// markdown code fences.
fn extract_json(text: &str) -> Option<String> {
    let text = text.trim();

    // 1. Plain JSON starting with `{`
    if text.starts_with('{') {
        if let Some(_end) = text.rfind('}') {
            // Validate it parses as generic JSON value
            if serde_json::from_str::<serde_json::Value>(text).is_ok() {
                return Some(text.to_string());
            }
        }
    }

    // 2. Markdown code block
    if text.starts_with("```") {
        let mut lines: Vec<&str> = Vec::new();
        let mut in_block = false;
        for line in text.lines() {
            if line.starts_with("```") {
                if in_block {
                    break;
                }
                in_block = true;
                continue;
            }
            if in_block {
                lines.push(line);
            }
        }
        let candidate = lines.join("\n");
        if serde_json::from_str::<serde_json::Value>(&candidate).is_ok() {
            return Some(candidate);
        }
    }

    // 3. Find last JSON object in the text
    if let Some(end) = text.rfind('}') {
        if let Some(start) = text[..=end].rfind('{') {
            let candidate = &text[start..=end];
            if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                return Some(candidate.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;

    // ── Mock providers ──────────────────────────────────────────────

    struct MockProvider {
        response: String,
    }

    impl AiProvider for MockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            let resp = self.response.clone();
            Box::pin(async move { Ok(resp) })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    struct HangingProvider;

    impl AiProvider for HangingProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok("never".to_string())
            })
        }

        fn name(&self) -> &str {
            "hanging"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────

    fn make_classifier(response: &str) -> AiBinaryClassifier {
        AiBinaryClassifier::new(
            Arc::new(MockProvider {
                response: response.to_string(),
            }),
            AiBinaryConfig {
                min_input_length: 5,
                ..Default::default()
            },
        )
    }

    // ── Tests ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn classify_execute() {
        let c = make_classifier(r#"{"intent":"execute","confidence":0.92}"#);
        let result = c.classify("organize my downloads folder").await.unwrap();
        assert!(result.is_execute());
        assert!(result.confidence() > 0.9);
    }

    #[tokio::test]
    async fn classify_converse() {
        let c = make_classifier(r#"{"intent":"converse","confidence":0.88}"#);
        let result = c.classify("what is quantum computing").await.unwrap();
        assert!(result.is_converse());
    }

    #[tokio::test]
    async fn classify_low_confidence_returns_none() {
        let c = make_classifier(r#"{"intent":"execute","confidence":0.3}"#);
        let result = c.classify("maybe do something").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn classify_too_short_returns_none() {
        let c = make_classifier(r#"{"intent":"execute","confidence":0.99}"#);
        let result = c.classify("hi").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn classify_timeout_returns_none() {
        let c = AiBinaryClassifier::new(
            Arc::new(HangingProvider),
            AiBinaryConfig {
                min_input_length: 5,
                timeout: Duration::from_millis(100),
                ..Default::default()
            },
        );
        let result = c.classify("organize my downloads folder").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn classify_json_in_markdown_block() {
        let response = "```json\n{\"intent\":\"execute\",\"confidence\":0.85}\n```";
        let c = make_classifier(response);
        let result = c.classify("run the test suite").await.unwrap();
        assert!(result.is_execute());
    }

    #[tokio::test]
    async fn classify_malformed_json_returns_none() {
        let c = make_classifier("I'm not sure what you mean");
        let result = c.classify("do something interesting").await;
        assert!(result.is_none());
    }
}
