//! Capability request types for AI-first intent detection.
//!
//! This module defines the structures for AI responses that may contain
//! capability invocation requests or clarification needs.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Clarification Types (for multi-turn conversation)
// ============================================================================

/// Reason why AI needs clarification from user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClarificationReason {
    /// AI doesn't have enough information to answer
    InsufficientInfo,
    /// User's request is ambiguous (multiple interpretations)
    Ambiguous,
    /// AI wants to confirm its understanding before proceeding
    ConfirmationNeeded,
    /// AI needs a specific parameter value
    MissingParameter,
}

impl std::fmt::Display for ClarificationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientInfo => write!(f, "insufficient_info"),
            Self::Ambiguous => write!(f, "ambiguous"),
            Self::ConfirmationNeeded => write!(f, "confirmation_needed"),
            Self::MissingParameter => write!(f, "missing_parameter"),
        }
    }
}

/// Information about why AI needs clarification.
///
/// When the AI determines it cannot provide a good answer without
/// additional information, it returns this structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarificationInfo {
    /// Why clarification is needed
    pub reason: ClarificationReason,

    /// The question/prompt to show the user
    pub prompt: String,

    /// Summary of what AI understood so far (for context)
    #[serde(default)]
    pub context_summary: String,

    /// Optional suggested answers (for select-style clarification)
    #[serde(default)]
    pub suggestions: Option<Vec<String>>,
}

impl ClarificationInfo {
    /// Create a new clarification request.
    pub fn new(reason: ClarificationReason, prompt: impl Into<String>) -> Self {
        Self {
            reason,
            prompt: prompt.into(),
            context_summary: String::new(),
            suggestions: None,
        }
    }

    /// Add context summary.
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context_summary = context.into();
        self
    }

    /// Add suggestions.
    pub fn with_suggestions(mut self, suggestions: Vec<String>) -> Self {
        self.suggestions = Some(suggestions);
        self
    }

    /// Check if this has suggestions (should show select UI).
    pub fn has_suggestions(&self) -> bool {
        self.suggestions.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
    }
}

// ============================================================================
// Capability Request Types
// ============================================================================

/// A request from the AI to invoke a capability.
///
/// When the AI determines it needs to use a capability (like search or video),
/// it returns a JSON response matching this structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRequest {
    /// The capability to invoke (e.g., "search", "video")
    pub capability: String,

    /// Parameters for the capability invocation
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,

    /// The user's original query (for context in second AI call)
    pub query: String,

    /// Optional reasoning for why this capability is needed (for debugging)
    #[serde(default)]
    pub reasoning: Option<String>,
}

impl CapabilityRequest {
    /// Create a new capability request.
    pub fn new(capability: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            capability: capability.into(),
            parameters: HashMap::new(),
            query: query.into(),
            reasoning: None,
        }
    }

    /// Add a parameter to the request.
    pub fn with_param(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.parameters.insert(key.into(), value.into());
        self
    }

    /// Add reasoning to the request.
    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reasoning = Some(reasoning.into());
        self
    }

    /// Get a string parameter by name.
    pub fn get_string_param(&self, name: &str) -> Option<String> {
        self.parameters.get(name).and_then(|v| v.as_str()).map(String::from)
    }

    /// Check if this is a search request.
    pub fn is_search(&self) -> bool {
        self.capability == "search"
    }

    /// Check if this is a video request.
    pub fn is_video(&self) -> bool {
        self.capability == "video"
    }

    /// Check if this is an MCP request.
    pub fn is_mcp(&self) -> bool {
        self.capability == "mcp"
    }
}

/// Parsed AI response - direct answer, capability request, or clarification need.
#[derive(Debug, Clone)]
pub enum AiResponse {
    /// AI responded directly without needing any capability
    Direct(String),

    /// AI requested a capability to be executed before responding
    CapabilityRequest(CapabilityRequest),

    /// AI needs clarification from user before it can respond
    NeedsClarification(ClarificationInfo),
}

impl AiResponse {
    /// Create a direct response.
    pub fn direct(content: impl Into<String>) -> Self {
        Self::Direct(content.into())
    }

    /// Create a capability request response.
    pub fn capability_request(request: CapabilityRequest) -> Self {
        Self::CapabilityRequest(request)
    }

    /// Create a clarification needed response.
    pub fn needs_clarification(info: ClarificationInfo) -> Self {
        Self::NeedsClarification(info)
    }

    /// Check if this is a direct response.
    pub fn is_direct(&self) -> bool {
        matches!(self, Self::Direct(_))
    }

    /// Check if this is a capability request.
    pub fn is_capability_request(&self) -> bool {
        matches!(self, Self::CapabilityRequest(_))
    }

    /// Check if this needs clarification.
    pub fn needs_user_clarification(&self) -> bool {
        matches!(self, Self::NeedsClarification(_))
    }

    /// Get the direct response content, if this is a direct response.
    pub fn as_direct(&self) -> Option<&str> {
        match self {
            Self::Direct(content) => Some(content),
            _ => None,
        }
    }

    /// Get the capability request, if this is a capability request.
    pub fn as_capability_request(&self) -> Option<&CapabilityRequest> {
        match self {
            Self::CapabilityRequest(req) => Some(req),
            _ => None,
        }
    }

    /// Get the clarification info, if this needs clarification.
    pub fn as_clarification(&self) -> Option<&ClarificationInfo> {
        match self {
            Self::NeedsClarification(info) => Some(info),
            _ => None,
        }
    }
}

/// Raw JSON structure for parsing AI responses.
///
/// This is the structure the AI should return when requesting a capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RawCapabilityRequest {
    /// Marker field to identify capability requests
    #[serde(rename = "__capability_request__")]
    pub is_capability_request: bool,

    /// The capability to invoke
    pub capability: String,

    /// Parameters for the capability
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,

    /// The user's original query
    pub query: String,

    /// Optional reasoning
    #[serde(default)]
    pub reasoning: Option<String>,
}

impl From<RawCapabilityRequest> for CapabilityRequest {
    fn from(raw: RawCapabilityRequest) -> Self {
        Self {
            capability: raw.capability,
            parameters: raw.parameters,
            query: raw.query,
            reasoning: raw.reasoning,
        }
    }
}

/// Raw JSON structure for parsing AI clarification requests.
///
/// This is the structure the AI should return when it needs more information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RawClarificationRequest {
    /// Marker field to identify clarification requests
    #[serde(rename = "__needs_clarification__")]
    pub needs_clarification: bool,

    /// Reason for needing clarification
    #[serde(default = "default_reason")]
    pub reason: String,

    /// The prompt/question to show the user
    pub prompt: String,

    /// Summary of what AI understood
    #[serde(default)]
    pub context_summary: Option<String>,

    /// Optional suggested answers
    #[serde(default)]
    pub suggestions: Option<Vec<String>>,
}

fn default_reason() -> String {
    "insufficient_info".to_string()
}

impl From<RawClarificationRequest> for ClarificationInfo {
    fn from(raw: RawClarificationRequest) -> Self {
        let reason = match raw.reason.as_str() {
            "ambiguous" => ClarificationReason::Ambiguous,
            "confirmation_needed" => ClarificationReason::ConfirmationNeeded,
            "missing_parameter" => ClarificationReason::MissingParameter,
            _ => ClarificationReason::InsufficientInfo,
        };

        Self {
            reason,
            prompt: raw.prompt,
            context_summary: raw.context_summary.unwrap_or_default(),
            suggestions: raw.suggestions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_request_builder() {
        let req = CapabilityRequest::new("search", "weather in Tokyo")
            .with_param("query", "Tokyo weather")
            .with_reasoning("User asked about current weather");

        assert_eq!(req.capability, "search");
        assert_eq!(req.query, "weather in Tokyo");
        assert!(req.is_search());
        assert_eq!(req.get_string_param("query"), Some("Tokyo weather".to_string()));
        assert!(req.reasoning.is_some());
    }

    #[test]
    fn test_ai_response_direct() {
        let response = AiResponse::direct("Hello, world!");
        assert!(response.is_direct());
        assert!(!response.is_capability_request());
        assert_eq!(response.as_direct(), Some("Hello, world!"));
    }

    #[test]
    fn test_ai_response_capability_request() {
        let req = CapabilityRequest::new("video", "summarize video");
        let response = AiResponse::capability_request(req);
        assert!(!response.is_direct());
        assert!(response.is_capability_request());
        assert!(response.as_capability_request().unwrap().is_video());
    }

    #[test]
    fn test_raw_capability_request_parsing() {
        let json = r#"{
            "__capability_request__": true,
            "capability": "search",
            "parameters": {"query": "weather"},
            "query": "What's the weather?",
            "reasoning": "User needs current weather data"
        }"#;

        let raw: RawCapabilityRequest = serde_json::from_str(json).unwrap();
        assert!(raw.is_capability_request);
        assert_eq!(raw.capability, "search");

        let req: CapabilityRequest = raw.into();
        assert!(req.is_search());
    }

    #[test]
    fn test_clarification_info_builder() {
        let info = ClarificationInfo::new(ClarificationReason::Ambiguous, "Which city?")
            .with_context("User asked about weather")
            .with_suggestions(vec!["Beijing".to_string(), "Shanghai".to_string()]);

        assert_eq!(info.reason, ClarificationReason::Ambiguous);
        assert_eq!(info.prompt, "Which city?");
        assert!(info.has_suggestions());
        assert_eq!(info.suggestions.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_ai_response_needs_clarification() {
        let info = ClarificationInfo::new(ClarificationReason::InsufficientInfo, "What language?");
        let response = AiResponse::needs_clarification(info);

        assert!(!response.is_direct());
        assert!(!response.is_capability_request());
        assert!(response.needs_user_clarification());

        let clarification = response.as_clarification().unwrap();
        assert_eq!(clarification.prompt, "What language?");
    }

    #[test]
    fn test_raw_clarification_request_parsing() {
        let json = r#"{
            "__needs_clarification__": true,
            "reason": "ambiguous",
            "prompt": "您是想了解北京还是上海的天气？",
            "context_summary": "用户询问天气但未指定城市",
            "suggestions": ["北京", "上海"]
        }"#;

        let raw: RawClarificationRequest = serde_json::from_str(json).unwrap();
        assert!(raw.needs_clarification);
        assert_eq!(raw.reason, "ambiguous");

        let info: ClarificationInfo = raw.into();
        assert_eq!(info.reason, ClarificationReason::Ambiguous);
        assert!(info.has_suggestions());
    }

    #[test]
    fn test_clarification_reason_display() {
        assert_eq!(ClarificationReason::InsufficientInfo.to_string(), "insufficient_info");
        assert_eq!(ClarificationReason::Ambiguous.to_string(), "ambiguous");
        assert_eq!(ClarificationReason::ConfirmationNeeded.to_string(), "confirmation_needed");
        assert_eq!(ClarificationReason::MissingParameter.to_string(), "missing_parameter");
    }
}
