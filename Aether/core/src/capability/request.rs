//! Capability request types for AI-first intent detection.
//!
//! This module defines the structures for AI responses that may contain
//! capability invocation requests.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// Parsed AI response - either a direct answer or a capability request.
#[derive(Debug, Clone)]
pub enum AiResponse {
    /// AI responded directly without needing any capability
    Direct(String),

    /// AI requested a capability to be executed before responding
    CapabilityRequest(CapabilityRequest),
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

    /// Check if this is a direct response.
    pub fn is_direct(&self) -> bool {
        matches!(self, Self::Direct(_))
    }

    /// Check if this is a capability request.
    pub fn is_capability_request(&self) -> bool {
        matches!(self, Self::CapabilityRequest(_))
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
}
