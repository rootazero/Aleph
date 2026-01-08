//! L3 AI Router - AI-powered tool routing with conversation context
//!
//! This module implements the L3 layer of the Dispatcher, using AI to:
//! - Determine which tool should handle a request
//! - Extract parameters from user input
//! - Provide confidence scoring for confirmation decisions
//!
//! # Architecture
//!
//! ```text
//! User Input
//!      ↓
//! L1 Regex Match (failed or low confidence)
//!      ↓
//! L2 Semantic Match (failed or low confidence)
//!      ↓
//! ┌─────────────────────────────────────────┐
//! │           L3 AI Router                  │
//! │                                         │
//! │  ┌───────────────┐  ┌───────────────┐  │
//! │  │ PromptBuilder │  │ AI Provider   │  │
//! │  │ (tool list)   │→ │ (inference)   │  │
//! │  └───────────────┘  └───────┬───────┘  │
//! │                             ↓          │
//! │  ┌───────────────────────────────────┐ │
//! │  │ L3RoutingResponse → RoutingMatch  │ │
//! │  └───────────────────────────────────┘ │
//! └─────────────────────────────────────────┘
//!      ↓
//! RoutingMatch { tool, confidence, params, layer: L3Inference }
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::dispatcher::{L3Router, ToolRegistry};
//!
//! let registry = ToolRegistry::new();
//! let provider = create_ai_provider();
//! let router = L3Router::new(provider);
//!
//! // Route with tool list
//! let tools = registry.list_all().await;
//! let result = router.route("search for the weather in Tokyo", &tools, None).await?;
//!
//! if let Some(response) = result {
//!     println!("Tool: {:?}, Confidence: {}", response.tool, response.confidence);
//! }
//! ```

use crate::dispatcher::{L3RoutingResponse, PromptBuilder, RoutingLayer, UnifiedTool};
use crate::error::{AetherError, Result};
use crate::providers::AiProvider;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// L3 AI Router for intelligent tool routing
///
/// Uses AI inference to determine the best tool for handling user requests
/// when L1/L2 layers fail or produce low-confidence matches.
pub struct L3Router {
    /// AI provider for routing inference
    provider: Arc<dyn AiProvider>,

    /// Timeout for L3 routing inference
    timeout: Duration,

    /// Minimum confidence threshold for accepting a match
    confidence_threshold: f32,

    /// Whether to use minimal prompts for lower latency
    use_minimal_prompts: bool,
}

impl L3Router {
    /// Create a new L3 Router with the given AI provider
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            provider,
            timeout: Duration::from_millis(5000), // 5 second default timeout
            confidence_threshold: 0.3,
            use_minimal_prompts: false,
        }
    }

    /// Set the timeout for L3 routing
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the minimum confidence threshold
    pub fn with_confidence_threshold(mut self, threshold: f32) -> Self {
        self.confidence_threshold = threshold;
        self
    }

    /// Enable minimal prompts for lower latency
    pub fn with_minimal_prompts(mut self, minimal: bool) -> Self {
        self.use_minimal_prompts = minimal;
        self
    }

    /// Route user input using AI inference
    ///
    /// # Arguments
    ///
    /// * `input` - User input to route
    /// * `tools` - Available tools for routing
    /// * `conversation_context` - Optional conversation history for context
    ///
    /// # Returns
    ///
    /// * `Ok(Some(response))` - Successfully determined routing with sufficient confidence
    /// * `Ok(None)` - No tool matched or confidence too low
    /// * `Err` - Routing failed (timeout, AI error, etc.)
    pub async fn route(
        &self,
        input: &str,
        tools: &[UnifiedTool],
        conversation_context: Option<&str>,
    ) -> Result<Option<L3RoutingResponse>> {
        // Skip if no tools available
        if tools.is_empty() {
            debug!("L3 Router: No tools available, skipping");
            return Ok(None);
        }

        // Skip very short inputs
        if input.trim().len() < 3 {
            debug!("L3 Router: Input too short, skipping");
            return Ok(None);
        }

        info!(
            input_length = input.len(),
            tool_count = tools.len(),
            has_context = conversation_context.is_some(),
            "L3 Router: Starting AI routing"
        );

        // Build the routing prompt
        let system_prompt = if self.use_minimal_prompts {
            PromptBuilder::build_l3_routing_prompt_minimal(tools)
        } else {
            PromptBuilder::build_l3_routing_prompt(tools, conversation_context)
        };

        // Build user prompt
        let user_prompt = format!(
            "[USER INPUT]\n{}\n\n[TASK]\nAnalyze the input and return a JSON routing decision.",
            input
        );

        // Combine for providers that may ignore system prompt
        let combined_prompt = format!(
            "[L3 ROUTING - Return JSON ONLY]\n\n{}\n\n---\n\n{}",
            system_prompt, user_prompt
        );

        // Call AI provider with timeout
        let response = tokio::time::timeout(
            self.timeout,
            self.provider.process(&combined_prompt, None),
        )
        .await
        .map_err(|_| {
            warn!(
                "L3 Router: Timeout after {}ms",
                self.timeout.as_millis()
            );
            AetherError::Timeout {
                suggestion: Some(format!(
                    "L3 routing timed out after {}ms",
                    self.timeout.as_millis()
                )),
            }
        })??;

        debug!(
            response_preview = %response.chars().take(200).collect::<String>(),
            "L3 Router: Received AI response"
        );

        // Parse the response
        let routing = match PromptBuilder::parse_l3_response(&response) {
            Some(r) => r,
            None => {
                warn!(
                    response = %response,
                    "L3 Router: Failed to parse response, falling back to None"
                );
                return Ok(None);
            }
        };

        info!(
            tool = ?routing.tool,
            confidence = routing.confidence,
            reason = %routing.reason,
            "L3 Router: Routing decision"
        );

        // Check confidence threshold
        if routing.has_match() && routing.confidence >= self.confidence_threshold {
            Ok(Some(routing))
        } else {
            debug!(
                confidence = routing.confidence,
                threshold = self.confidence_threshold,
                "L3 Router: Confidence below threshold, returning None"
            );
            Ok(None)
        }
    }

    /// Route with extended options
    pub async fn route_with_options(
        &self,
        input: &str,
        tools: &[UnifiedTool],
        options: L3RoutingOptions,
    ) -> Result<Option<L3RoutingResponse>> {
        // Apply options
        let conversation_context = options.conversation_context.as_deref();

        // Build custom prompt if entity hints provided
        let system_prompt = if !options.entity_hints.is_empty() {
            let base_prompt = if self.use_minimal_prompts {
                PromptBuilder::build_l3_routing_prompt_minimal(tools)
            } else {
                PromptBuilder::build_l3_routing_prompt(tools, conversation_context)
            };

            // Inject entity hints for pronoun resolution
            let entity_section = format!(
                "\n\n## Entity Context\n\nRecently mentioned entities that pronouns may refer to:\n{}",
                options.entity_hints.iter()
                    .map(|e| format!("- {}", e))
                    .collect::<Vec<_>>()
                    .join("\n")
            );

            format!("{}{}", base_prompt, entity_section)
        } else if self.use_minimal_prompts {
            PromptBuilder::build_l3_routing_prompt_minimal(tools)
        } else {
            PromptBuilder::build_l3_routing_prompt(tools, conversation_context)
        };

        let user_prompt = format!(
            "[USER INPUT]\n{}\n\n[TASK]\nAnalyze the input and return a JSON routing decision.",
            input
        );

        let combined_prompt = format!(
            "[L3 ROUTING - Return JSON ONLY]\n\n{}\n\n---\n\n{}",
            system_prompt, user_prompt
        );

        // Apply custom timeout if specified
        let timeout = options.timeout.unwrap_or(self.timeout);

        let response = tokio::time::timeout(
            timeout,
            self.provider.process(&combined_prompt, None),
        )
        .await
        .map_err(|_| {
            warn!("L3 Router: Timeout after {}ms", timeout.as_millis());
            AetherError::Timeout {
                suggestion: Some(format!(
                    "L3 routing timed out after {}ms",
                    timeout.as_millis()
                )),
            }
        })??;

        let routing = match PromptBuilder::parse_l3_response(&response) {
            Some(r) => r,
            None => return Ok(None),
        };

        let threshold = options.confidence_threshold.unwrap_or(self.confidence_threshold);
        if routing.has_match() && routing.confidence >= threshold {
            Ok(Some(routing))
        } else {
            Ok(None)
        }
    }

    /// Extract parameters for a specific tool from user input
    ///
    /// Used when the tool is already determined but parameters need extraction.
    pub async fn extract_parameters(
        &self,
        input: &str,
        tool: &UnifiedTool,
    ) -> Result<serde_json::Value> {
        let prompt = PromptBuilder::build_parameter_extraction_prompt(tool, input);

        let response = tokio::time::timeout(
            self.timeout,
            self.provider.process(&prompt, None),
        )
        .await
        .map_err(|_| {
            AetherError::Timeout {
                suggestion: Some("Parameter extraction timed out".to_string()),
            }
        })??;

        // Try to parse as JSON
        extract_json_object(&response)
            .ok_or_else(|| AetherError::provider("Failed to parse parameters from AI response"))
    }

    /// Get the routing layer identifier
    pub fn routing_layer(&self) -> RoutingLayer {
        RoutingLayer::L3Inference
    }
}

/// Options for L3 routing
#[derive(Debug, Clone, Default)]
pub struct L3RoutingOptions {
    /// Conversation context for better routing
    pub conversation_context: Option<String>,

    /// Entity hints for pronoun resolution
    pub entity_hints: Vec<String>,

    /// Custom timeout override
    pub timeout: Option<Duration>,

    /// Custom confidence threshold override
    pub confidence_threshold: Option<f32>,
}

impl L3RoutingOptions {
    /// Create new options with conversation context
    pub fn with_context(context: impl Into<String>) -> Self {
        Self {
            conversation_context: Some(context.into()),
            ..Default::default()
        }
    }

    /// Add entity hints for pronoun resolution
    pub fn with_entity_hints(mut self, hints: Vec<String>) -> Self {
        self.entity_hints = hints;
        self
    }

    /// Set custom timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set custom confidence threshold
    pub fn with_confidence_threshold(mut self, threshold: f32) -> Self {
        self.confidence_threshold = Some(threshold);
        self
    }
}

/// L3 routing result with full metadata
#[derive(Debug, Clone)]
pub struct L3RoutingResult {
    /// The routing response from AI
    pub response: L3RoutingResponse,

    /// The routing layer (always L3Inference)
    pub routing_layer: RoutingLayer,

    /// Whether confirmation is recommended
    pub needs_confirmation: bool,

    /// Matched tool details (if any)
    pub matched_tool: Option<UnifiedTool>,
}

impl L3RoutingResult {
    /// Create from L3RoutingResponse
    pub fn from_response(
        response: L3RoutingResponse,
        tools: &[UnifiedTool],
        confirmation_threshold: f32,
    ) -> Self {
        let matched_tool = response.tool.as_ref().and_then(|name| {
            tools.iter().find(|t| t.name == *name).cloned()
        });

        let needs_confirmation = response.needs_confirmation(confirmation_threshold);

        Self {
            response,
            routing_layer: RoutingLayer::L3Inference,
            needs_confirmation,
            matched_tool,
        }
    }

    /// Check if a tool was matched
    pub fn has_match(&self) -> bool {
        self.response.has_match()
    }

    /// Get the tool name
    pub fn tool_name(&self) -> Option<&str> {
        self.response.tool.as_deref()
    }

    /// Get the confidence score
    pub fn confidence(&self) -> f32 {
        self.response.confidence
    }

    /// Get the extracted parameters
    pub fn parameters(&self) -> &serde_json::Value {
        &self.response.parameters
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Extract JSON object from response that may contain extra text
fn extract_json_object(response: &str) -> Option<serde_json::Value> {
    let response = response.trim();

    // Try direct parse first
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(response) {
        return Some(value);
    }

    // Try to extract from markdown code block
    if let Some(start) = response.find("```json") {
        let json_start = start + 7;
        if let Some(end) = response[json_start..].find("```") {
            let json_str = response[json_start..json_start + end].trim();
            if let Ok(value) = serde_json::from_str(json_str) {
                return Some(value);
            }
        }
    }

    // Try to find JSON object
    if let Some(start) = response.find('{') {
        if let Some(end) = response.rfind('}') {
            if end > start {
                let json_str = &response[start..=end];
                if let Ok(value) = serde_json::from_str(json_str) {
                    return Some(value);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;
    use serde_json::json;

    // Mock provider for testing
    struct MockProvider {
        response: String,
    }

    impl MockProvider {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
            }
        }
    }

    impl AiProvider for MockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>>
        {
            let response = self.response.clone();
            Box::pin(async move { Ok(response) })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    fn create_test_tools() -> Vec<UnifiedTool> {
        vec![
            UnifiedTool::new(
                "native:search",
                "search",
                "Search the web for information",
                ToolSource::Native,
            )
            .with_parameters_schema(json!({
                "properties": {
                    "query": { "type": "string" }
                }
            })),
            UnifiedTool::new(
                "native:video",
                "video",
                "Analyze video content",
                ToolSource::Native,
            ),
        ]
    }

    #[tokio::test]
    async fn test_l3_router_successful_match() {
        let response = r#"{"tool": "search", "confidence": 0.9, "parameters": {"query": "weather"}, "reason": "User wants to search"}"#;
        let provider = Arc::new(MockProvider::new(response));
        let router = L3Router::new(provider);

        let tools = create_test_tools();
        let result = router.route("search for weather", &tools, None).await.unwrap();

        assert!(result.is_some());
        let routing = result.unwrap();
        assert_eq!(routing.tool, Some("search".to_string()));
        assert_eq!(routing.confidence, 0.9);
        assert!(routing.has_match());
    }

    #[tokio::test]
    async fn test_l3_router_no_match() {
        let response = r#"{"tool": null, "confidence": 0.0, "parameters": {}, "reason": "No matching tool"}"#;
        let provider = Arc::new(MockProvider::new(response));
        let router = L3Router::new(provider);

        let tools = create_test_tools();
        let result = router.route("hello world", &tools, None).await.unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_l3_router_low_confidence() {
        let response = r#"{"tool": "search", "confidence": 0.2, "parameters": {}, "reason": "Weak match"}"#;
        let provider = Arc::new(MockProvider::new(response));
        let router = L3Router::new(provider).with_confidence_threshold(0.5);

        let tools = create_test_tools();
        let result = router.route("maybe search?", &tools, None).await.unwrap();

        // Should return None because confidence < threshold
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_l3_router_empty_tools() {
        let provider = Arc::new(MockProvider::new(""));
        let router = L3Router::new(provider);

        let result = router.route("test", &[], None).await.unwrap();

        // Should return None for empty tools
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_l3_router_short_input() {
        let provider = Arc::new(MockProvider::new(""));
        let router = L3Router::new(provider);

        let tools = create_test_tools();
        let result = router.route("hi", &tools, None).await.unwrap();

        // Should skip very short inputs
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_l3_router_with_context() {
        let response = r#"{"tool": "search", "confidence": 0.95, "parameters": {"query": "weather in Tokyo"}, "reason": "Continuing weather query"}"#;
        let provider = Arc::new(MockProvider::new(response));
        let router = L3Router::new(provider);

        let tools = create_test_tools();
        let context = "User asked about weather earlier";
        let result = router.route("what about Tokyo?", &tools, Some(context)).await.unwrap();

        assert!(result.is_some());
        let routing = result.unwrap();
        assert_eq!(routing.confidence, 0.95);
    }

    #[tokio::test]
    async fn test_l3_router_with_options() {
        let response = r#"{"tool": "video", "confidence": 0.85, "parameters": {"url": "youtube.com/watch?v=abc"}, "reason": "User wants video analysis"}"#;
        let provider = Arc::new(MockProvider::new(response));
        let router = L3Router::new(provider);

        let tools = create_test_tools();
        let options = L3RoutingOptions::with_context("Previous video discussion")
            .with_entity_hints(vec!["youtube video about cooking".to_string()]);

        let result = router.route_with_options("analyze that", &tools, options).await.unwrap();

        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_l3_routing_result() {
        let response = L3RoutingResponse {
            tool: Some("search".to_string()),
            confidence: 0.7,
            parameters: json!({"query": "test"}),
            reason: "Test".to_string(),
        };

        let tools = create_test_tools();
        let result = L3RoutingResult::from_response(response, &tools, 0.8);

        assert!(result.has_match());
        assert!(result.needs_confirmation); // 0.7 < 0.8
        assert!(result.matched_tool.is_some());
        assert_eq!(result.tool_name(), Some("search"));
        assert_eq!(result.confidence(), 0.7);
    }

    #[tokio::test]
    async fn test_extract_parameters() {
        let response = r#"{"query": "latest news", "limit": 10}"#;
        let provider = Arc::new(MockProvider::new(response));
        let router = L3Router::new(provider);

        let tool = UnifiedTool::new(
            "native:search",
            "search",
            "Search the web",
            ToolSource::Native,
        );

        let params = router.extract_parameters("find latest news", &tool).await.unwrap();

        assert_eq!(params["query"], "latest news");
        assert_eq!(params["limit"], 10);
    }

    #[test]
    fn test_extract_json_object() {
        // Raw JSON
        let raw = r#"{"key": "value"}"#;
        let parsed = extract_json_object(raw).unwrap();
        assert_eq!(parsed["key"], "value");

        // With markdown
        let markdown = "```json\n{\"key\": \"value\"}\n```";
        let parsed = extract_json_object(markdown).unwrap();
        assert_eq!(parsed["key"], "value");

        // With extra text
        let mixed = "Here is the result: {\"key\": \"value\"} done.";
        let parsed = extract_json_object(mixed).unwrap();
        assert_eq!(parsed["key"], "value");

        // Invalid
        assert!(extract_json_object("not json").is_none());
    }

    #[test]
    fn test_l3_routing_options() {
        let options = L3RoutingOptions::with_context("test context")
            .with_entity_hints(vec!["entity1".to_string()])
            .with_timeout(Duration::from_secs(10))
            .with_confidence_threshold(0.5);

        assert_eq!(options.conversation_context, Some("test context".to_string()));
        assert_eq!(options.entity_hints, vec!["entity1".to_string()]);
        assert_eq!(options.timeout, Some(Duration::from_secs(10)));
        assert_eq!(options.confidence_threshold, Some(0.5));
    }
}
