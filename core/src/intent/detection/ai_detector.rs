//! AI-powered intent detection for language-agnostic intent classification.
//!
//! This module uses the default AI provider to detect user intent from input
//! in any language, extracting structured information like intent type and
//! parameters.
//!
//! # Architecture
//!
//! ```text
//! User Input (any language)
//!     ↓
//! [1] Quick pre-check (URLs, obvious patterns)
//!     ↓
//! [2] AI Intent Detection (lightweight request)
//!     ↓
//! AiIntentResult { intent, params, missing_params }
//! ```
//!
//! # Example
//!
//! ```ignore
//! let detector = AiIntentDetector::new(provider);
//! let result = detector.detect("¿Cómo está el clima en Madrid?").await?;
//! // result.intent == "search"
//! // result.params["location"] == "Madrid"
//! ```

use crate::config::IntentDetectionPolicy;
use crate::error::{AlephError, Result};
use crate::providers::AiProvider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::sync_primitives::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Result of AI intent detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiIntentResult {
    /// Detected intent type: "search", "video", "general"
    pub intent: String,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// Extracted parameters
    #[serde(default)]
    pub params: HashMap<String, String>,
    /// Parameters that are required but missing
    #[serde(default)]
    pub missing: Vec<String>,
}

impl Default for AiIntentResult {
    fn default() -> Self {
        AiIntentResult {
            intent: "general".to_string(),
            confidence: 0.0,
            params: HashMap::new(),
            missing: Vec::new(),
        }
    }
}

/// AI-powered intent detector.
pub struct AiIntentDetector {
    /// AI provider for intent detection
    provider: Arc<dyn AiProvider>,
    /// Confidence threshold for accepting intent
    confidence_threshold: f64,
    /// Timeout for intent detection
    timeout: Duration,
    /// Minimum input length for intent detection
    min_input_length: usize,
    /// Video URL patterns for quick detection
    video_url_patterns: Vec<String>,
}

impl AiIntentDetector {
    /// Create a new AI intent detector with default settings.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        let default_policy = IntentDetectionPolicy::default();
        Self::with_policy(provider, &default_policy)
    }

    /// Create a new AI intent detector with policy configuration.
    pub fn with_policy(provider: Arc<dyn AiProvider>, policy: &IntentDetectionPolicy) -> Self {
        AiIntentDetector {
            provider,
            confidence_threshold: policy.confidence_threshold,
            timeout: policy.timeout_duration(),
            min_input_length: policy.min_input_length as usize,
            video_url_patterns: policy.video_url_patterns.clone(),
        }
    }

    /// Set the confidence threshold.
    pub fn with_confidence_threshold(mut self, threshold: f64) -> Self {
        self.confidence_threshold = threshold;
        self
    }

    /// Set the timeout duration.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the minimum input length.
    pub fn with_min_input_length(mut self, length: usize) -> Self {
        self.min_input_length = length;
        self
    }

    /// Set the video URL patterns.
    pub fn with_video_url_patterns(mut self, patterns: Vec<String>) -> Self {
        self.video_url_patterns = patterns;
        self
    }

    /// Detect intent from user input using AI.
    ///
    /// Returns `Ok(Some(result))` if intent was detected with sufficient confidence,
    /// `Ok(None)` if no specific intent was detected (general chat),
    /// `Err` if detection failed.
    pub async fn detect(&self, input: &str) -> Result<Option<AiIntentResult>> {
        // Quick pre-check: skip very short inputs (configurable via policy)
        if input.trim().len() < self.min_input_length {
            return Ok(None);
        }

        // Quick pre-check: if input contains URL, likely video intent
        if self.contains_video_url(input) {
            return Ok(Some(AiIntentResult {
                intent: "video".to_string(),
                confidence: 1.0,
                params: self.extract_url(input),
                missing: Vec::new(),
            }));
        }

        // Use AI for detection
        // Combine system prompt and user prompt to work with prepend-mode providers
        // This ensures intent detection works regardless of provider's system_prompt_mode setting
        // Use a clear separator to help the model understand this is a classification task
        let system_prompt = self.get_system_prompt();
        let user_prompt = self.build_detection_prompt(input);
        let combined_prompt = format!(
            "[TASK: Intent Classification - Return JSON ONLY, do NOT answer the question]\n\n{}\n\n---\n\n{}",
            system_prompt, user_prompt
        );

        info!(
            input_length = input.len(),
            input_preview = %input.chars().take(50).collect::<String>(),
            "Starting AI intent detection"
        );

        // Call AI provider with timeout
        // Pass None for system_prompt since we've combined it into the user prompt
        let response =
            tokio::time::timeout(self.timeout, self.provider.process(&combined_prompt, None))
                .await
                .map_err(|_| {
                    warn!(
                        "AI intent detection timed out after {}ms",
                        self.timeout.as_millis()
                    );
                    AlephError::Timeout {
                        suggestion: Some(format!(
                            "AI intent detection timed out after {}ms",
                            self.timeout.as_millis()
                        )),
                    }
                })??;

        info!(
            response_preview = %response.chars().take(200).collect::<String>(),
            "AI intent detection raw response"
        );

        // Parse response
        let result = self.parse_response(&response)?;

        info!(
            intent = %result.intent,
            confidence = result.confidence,
            params = ?result.params,
            missing = ?result.missing,
            "AI intent detection completed"
        );

        // Check confidence threshold
        if result.intent != "general" && result.confidence >= self.confidence_threshold {
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    /// Check if input contains a video URL (YouTube, Bilibili, etc.).
    /// Uses configurable URL patterns from policy.
    fn contains_video_url(&self, input: &str) -> bool {
        self.video_url_patterns.iter().any(|p| input.contains(p))
    }

    /// Extract URL from input.
    fn extract_url(&self, input: &str) -> HashMap<String, String> {
        let mut params = HashMap::new();

        // Simple URL extraction using regex
        let url_pattern = regex::Regex::new(
            r"(https?://(?:www\.)?(?:youtube\.com/watch\?v=|youtu\.be/|youtube\.com/shorts/|bilibili\.com/video/|b23\.tv/)[^\s]+)"
        ).ok();

        if let Some(re) = url_pattern {
            if let Some(caps) = re.captures(input) {
                if let Some(url) = caps.get(1) {
                    params.insert("url".to_string(), url.as_str().to_string());
                }
            }
        }

        params
    }

    /// Build the prompt for AI intent detection.
    fn build_detection_prompt(&self, input: &str) -> String {
        format!(
            r#"[INPUT TO CLASSIFY]
"{}"

[REQUIRED OUTPUT]
Return ONLY a JSON object. Do NOT answer the question. Do NOT provide any explanation.
Just output the JSON classification:"#,
            input.replace('"', "\\\"")
        )
    }

    /// Get the system prompt for intent detection.
    fn get_system_prompt(&self) -> String {
        r#"You are an intent classifier. Analyze user input and classify it into one of these intents:

1. "search" - User wants to search for information (weather, news, prices, facts, etc.)
   - Extract "location" if asking about weather for a specific place
   - Extract "query" for the search topic
   - If asking about weather but no location specified, add "location" to missing

2. "video" - User wants to analyze/summarize a video
   - Extract "url" if a video URL is provided
   - If no URL, add "url" to missing

3. "general" - General conversation, questions, tasks that don't need search/video

Respond ONLY with valid JSON in this exact format:
{"intent":"search|video|general","confidence":0.0-1.0,"params":{"key":"value"},"missing":["param"]}

Examples:
Input: "What's the weather like in Tokyo?"
Output: {"intent":"search","confidence":0.95,"params":{"location":"Tokyo","query":"weather"},"missing":[]}

Input: "¿Cómo está el clima?"
Output: {"intent":"search","confidence":0.9,"params":{"query":"weather"},"missing":["location"]}

Input: "Summarize this video"
Output: {"intent":"video","confidence":0.9,"params":{},"missing":["url"]}

Input: "Hello, how are you?"
Output: {"intent":"general","confidence":1.0,"params":{},"missing":[]}"#.to_string()
    }

    /// Parse the AI response into AiIntentResult.
    fn parse_response(&self, response: &str) -> Result<AiIntentResult> {
        // Try to extract JSON from response (handle markdown code blocks)
        let json_str = self.extract_json(response);

        // Parse JSON
        match serde_json::from_str::<AiIntentResult>(&json_str) {
            Ok(result) => Ok(result),
            Err(e) => {
                warn!(
                    response = %response,
                    error = %e,
                    "Failed to parse AI intent response, falling back to general"
                );
                Ok(AiIntentResult::default())
            }
        }
    }

    /// Extract JSON from response, handling markdown code blocks.
    fn extract_json(&self, response: &str) -> String {
        let response = response.trim();

        // Check for markdown code block
        if response.starts_with("```") {
            // Find the JSON content between ``` markers
            let lines: Vec<&str> = response.lines().collect();
            let mut json_lines = Vec::new();
            let mut in_block = false;

            for line in lines {
                if line.starts_with("```") {
                    if in_block {
                        break;
                    }
                    in_block = true;
                    continue;
                }
                if in_block {
                    json_lines.push(line);
                }
            }

            json_lines.join("\n")
        } else if response.starts_with('{') {
            // Already JSON
            response.to_string()
        } else {
            // Try to find JSON object in response
            if let Some(start) = response.find('{') {
                if let Some(end) = response.rfind('}') {
                    return response[start..=end].to_string();
                }
            }
            response.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_plain() {
        let detector = AiIntentDetector::new(Arc::new(MockProvider));
        let json = r#"{"intent":"search","confidence":0.9,"params":{},"missing":[]}"#;
        assert_eq!(detector.extract_json(json), json);
    }

    #[test]
    fn test_extract_json_markdown() {
        let detector = AiIntentDetector::new(Arc::new(MockProvider));
        let response = r#"```json
{"intent":"search","confidence":0.9,"params":{},"missing":[]}
```"#;
        let extracted = detector.extract_json(response);
        assert!(extracted.contains("search"));
    }

    #[test]
    fn test_extract_json_with_text() {
        let detector = AiIntentDetector::new(Arc::new(MockProvider));
        let response =
            r#"Here is the result: {"intent":"general","confidence":1.0,"params":{},"missing":[]}"#;
        let extracted = detector.extract_json(response);
        assert!(extracted.starts_with('{'));
        assert!(extracted.ends_with('}'));
    }

    #[test]
    fn test_contains_video_url() {
        let detector = AiIntentDetector::new(Arc::new(MockProvider));
        assert!(detector.contains_video_url("Check this https://youtube.com/watch?v=abc123"));
        assert!(detector.contains_video_url("https://youtu.be/abc123"));
        assert!(detector.contains_video_url("https://bilibili.com/video/BV123"));
        assert!(!detector.contains_video_url("Hello world"));
    }

    #[test]
    fn test_extract_url() {
        let detector = AiIntentDetector::new(Arc::new(MockProvider));
        let params =
            detector.extract_url("Summarize https://youtube.com/watch?v=dQw4w9WgXcQ please");
        assert_eq!(
            params.get("url"),
            Some(&"https://youtube.com/watch?v=dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn test_parse_response_valid() {
        let detector = AiIntentDetector::new(Arc::new(MockProvider));
        let response =
            r#"{"intent":"search","confidence":0.95,"params":{"location":"Tokyo"},"missing":[]}"#;
        let result = detector.parse_response(response).unwrap();
        assert_eq!(result.intent, "search");
        assert_eq!(result.confidence, 0.95);
        assert_eq!(result.params.get("location"), Some(&"Tokyo".to_string()));
    }

    #[test]
    fn test_parse_response_invalid() {
        let detector = AiIntentDetector::new(Arc::new(MockProvider));
        let response = "This is not JSON";
        let result = detector.parse_response(response).unwrap();
        assert_eq!(result.intent, "general");
        assert_eq!(result.confidence, 0.0);
    }

    // Mock provider for tests
    struct MockProvider;

    impl AiProvider for MockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            Box::pin(async {
                Ok(r#"{"intent":"general","confidence":1.0,"params":{},"missing":[]}"#.to_string())
            })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }
}
