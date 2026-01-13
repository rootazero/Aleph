//! SemanticIntent - Enhanced intent representation with confidence and traceability
//!
//! Unlike the simple `Intent` enum in `payload/intent.rs`, `SemanticIntent` provides:
//! - Confidence scores for probabilistic matching
//! - Parameter extraction and validation
//! - Detection method traceability
//! - Reasoning chain for debugging

use crate::payload::{Capability, Intent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Enhanced semantic intent with confidence and metadata
#[derive(Debug, Clone)]
pub struct SemanticIntent {
    /// Intent category (extensible)
    pub category: IntentCategory,

    /// Specific intent type within category (e.g., "weather", "translation")
    pub intent_type: String,

    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,

    /// Extracted parameters from input
    pub params: HashMap<String, ParamValue>,

    /// Missing required parameters (for follow-up prompts)
    pub missing_params: Vec<String>,

    /// Detection method used (for traceability)
    pub detection_method: DetectionMethod,

    /// Reasoning chain (for debugging/transparency)
    pub reasoning: Option<String>,

    /// Capabilities to enable for this intent
    pub capabilities: Vec<Capability>,

    /// System prompt override (from matched rule)
    pub system_prompt: Option<String>,

    /// Provider name (from matched rule)
    pub provider_name: Option<String>,

    /// Cleaned input (after prefix stripping)
    pub cleaned_input: Option<String>,
}

impl SemanticIntent {
    /// Create a new SemanticIntent with minimal required fields
    pub fn new(category: IntentCategory, intent_type: impl Into<String>) -> Self {
        Self {
            category,
            intent_type: intent_type.into(),
            confidence: 1.0,
            params: HashMap::new(),
            missing_params: Vec::new(),
            detection_method: DetectionMethod::ExactCommand,
            reasoning: None,
            capabilities: Vec::new(),
            system_prompt: None,
            provider_name: None,
            cleaned_input: None,
        }
    }

    /// Create a general chat intent (default)
    pub fn general() -> Self {
        Self::new(IntentCategory::General, "general_chat")
    }

    /// Builder: set confidence
    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Builder: set detection method
    pub fn with_method(mut self, method: DetectionMethod) -> Self {
        self.detection_method = method;
        self
    }

    /// Builder: set parameters
    pub fn with_params(mut self, params: HashMap<String, ParamValue>) -> Self {
        self.params = params;
        self
    }

    /// Builder: add a parameter
    pub fn with_param(mut self, key: impl Into<String>, value: ParamValue) -> Self {
        self.params.insert(key.into(), value);
        self
    }

    /// Builder: set missing parameters
    pub fn with_missing_params(mut self, params: Vec<String>) -> Self {
        self.missing_params = params;
        self
    }

    /// Builder: set reasoning
    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reasoning = Some(reasoning.into());
        self
    }

    /// Builder: set capabilities
    pub fn with_capabilities(mut self, capabilities: Vec<Capability>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Builder: set system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Builder: set provider name
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider_name = Some(provider.into());
        self
    }

    /// Builder: set cleaned input
    pub fn with_cleaned_input(mut self, input: impl Into<String>) -> Self {
        self.cleaned_input = Some(input.into());
        self
    }

    /// Check if this intent has missing required parameters
    pub fn has_missing_params(&self) -> bool {
        !self.missing_params.is_empty()
    }

    /// Check if this is a builtin capability intent
    pub fn is_builtin(&self) -> bool {
        matches!(self.category, IntentCategory::Builtin(_))
    }

    /// Check if this is a skills workflow intent
    pub fn is_skills(&self) -> bool {
        matches!(self.category, IntentCategory::Skills(_))
    }

    /// Check if confidence meets threshold
    pub fn is_confident(&self, threshold: f64) -> bool {
        self.confidence >= threshold
    }

    /// Get parameter value as string
    pub fn get_param_str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(|v| v.as_str())
    }

    /// Convert to legacy Intent enum for backward compatibility
    pub fn to_legacy_intent(&self) -> Intent {
        match &self.category {
            IntentCategory::Builtin(cap) => match cap {
                BuiltinCapability::Search => Intent::BuiltinSearch,
                BuiltinCapability::Mcp => Intent::BuiltinMcp,
                BuiltinCapability::Video => Intent::Custom("youtube_analysis".to_string()),
            },
            IntentCategory::Command(name) => Intent::Custom(name.clone()),
            IntentCategory::Semantic(name) => Intent::Custom(name.clone()),
            IntentCategory::Skills(id) => Intent::Skills(id.clone()),
            IntentCategory::General => Intent::GeneralChat,
        }
    }
}

impl Default for SemanticIntent {
    fn default() -> Self {
        Self::general()
    }
}

/// Intent category (extensible via config)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IntentCategory {
    /// Built-in capabilities (search, video, mcp)
    Builtin(BuiltinCapability),

    /// User-defined command (/translate, /code, etc.)
    Command(String),

    /// AI-detected semantic category (weather, news, translation, etc.)
    Semantic(String),

    /// Skills workflow (complex multi-step)
    Skills(String),

    /// General conversation (no special handling)
    General,
}

impl IntentCategory {
    /// Create a builtin search category
    pub fn search() -> Self {
        Self::Builtin(BuiltinCapability::Search)
    }

    /// Create a builtin video category
    pub fn video() -> Self {
        Self::Builtin(BuiltinCapability::Video)
    }

    /// Create a builtin MCP category
    pub fn mcp() -> Self {
        Self::Builtin(BuiltinCapability::Mcp)
    }

    /// Create a command category
    pub fn command(name: impl Into<String>) -> Self {
        Self::Command(name.into())
    }

    /// Create a semantic category
    pub fn semantic(name: impl Into<String>) -> Self {
        Self::Semantic(name.into())
    }

    /// Create a skills category
    pub fn skills(id: impl Into<String>) -> Self {
        Self::Skills(id.into())
    }
}

impl std::fmt::Display for IntentCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntentCategory::Builtin(cap) => write!(f, "builtin:{}", cap),
            IntentCategory::Command(name) => write!(f, "command:{}", name),
            IntentCategory::Semantic(name) => write!(f, "semantic:{}", name),
            IntentCategory::Skills(id) => write!(f, "skills:{}", id),
            IntentCategory::General => write!(f, "general"),
        }
    }
}

/// Built-in capability types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinCapability {
    /// Web search
    Search,
    /// Video transcript extraction
    Video,
    /// MCP tool calls
    Mcp,
}

impl std::fmt::Display for BuiltinCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuiltinCapability::Search => write!(f, "search"),
            BuiltinCapability::Video => write!(f, "video"),
            BuiltinCapability::Mcp => write!(f, "mcp"),
        }
    }
}

/// Detection method for traceability
#[derive(Debug, Clone, PartialEq)]
pub enum DetectionMethod {
    /// Exact command match (e.g., /search, /translate)
    ExactCommand,

    /// Regex pattern match
    RegexPattern,

    /// Keyword match with score
    KeywordMatch {
        /// Total keyword score
        score: f64,
        /// Matched keywords
        matched_keywords: Vec<String>,
    },

    /// Context-based inference
    ContextInference {
        /// Source of inference (e.g., "pending_param", "app_context", "time_context")
        source: String,
        /// Additional details
        details: Option<String>,
    },

    /// AI-driven detection
    AiDetection {
        /// Model used
        model: String,
        /// AI confidence
        confidence: f64,
    },

    /// Multiple methods combined
    Combined {
        /// Primary method
        primary: Box<DetectionMethod>,
        /// Secondary methods
        secondary: Vec<DetectionMethod>,
    },
}

impl DetectionMethod {
    /// Create a keyword match method
    pub fn keyword(score: f64, matched: Vec<String>) -> Self {
        Self::KeywordMatch {
            score,
            matched_keywords: matched,
        }
    }

    /// Create a context inference method
    pub fn context(source: impl Into<String>) -> Self {
        Self::ContextInference {
            source: source.into(),
            details: None,
        }
    }

    /// Create an AI detection method
    pub fn ai(model: impl Into<String>, confidence: f64) -> Self {
        Self::AiDetection {
            model: model.into(),
            confidence,
        }
    }

    /// Get method name for logging
    pub fn name(&self) -> &str {
        match self {
            DetectionMethod::ExactCommand => "exact_command",
            DetectionMethod::RegexPattern => "regex_pattern",
            DetectionMethod::KeywordMatch { .. } => "keyword_match",
            DetectionMethod::ContextInference { .. } => "context_inference",
            DetectionMethod::AiDetection { .. } => "ai_detection",
            DetectionMethod::Combined { .. } => "combined",
        }
    }
}

impl std::fmt::Display for DetectionMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetectionMethod::ExactCommand => write!(f, "exact_command"),
            DetectionMethod::RegexPattern => write!(f, "regex_pattern"),
            DetectionMethod::KeywordMatch { score, .. } => {
                write!(f, "keyword_match(score={:.2})", score)
            }
            DetectionMethod::ContextInference { source, .. } => {
                write!(f, "context_inference({})", source)
            }
            DetectionMethod::AiDetection { model, confidence } => {
                write!(f, "ai_detection({}:{:.2})", model, confidence)
            }
            DetectionMethod::Combined { primary, .. } => {
                write!(f, "combined({})", primary)
            }
        }
    }
}

/// Parameter value with type information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    /// String value
    String(String),
    /// Numeric value
    Number(f64),
    /// Boolean value
    Boolean(bool),
    /// List of strings
    List(Vec<String>),
    /// Nested object
    Object(HashMap<String, String>),
    /// Null/missing value
    Null,
}

impl ParamValue {
    /// Get as string reference
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ParamValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Get as owned string
    pub fn to_string_value(&self) -> Option<String> {
        match self {
            ParamValue::String(s) => Some(s.clone()),
            ParamValue::Number(n) => Some(n.to_string()),
            ParamValue::Boolean(b) => Some(b.to_string()),
            _ => None,
        }
    }

    /// Get as number
    pub fn as_number(&self) -> Option<f64> {
        match self {
            ParamValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Get as boolean
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ParamValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Get as list
    pub fn as_list(&self) -> Option<&Vec<String>> {
        match self {
            ParamValue::List(l) => Some(l),
            _ => None,
        }
    }

    /// Check if null
    pub fn is_null(&self) -> bool {
        matches!(self, ParamValue::Null)
    }
}

impl From<String> for ParamValue {
    fn from(s: String) -> Self {
        ParamValue::String(s)
    }
}

impl From<&str> for ParamValue {
    fn from(s: &str) -> Self {
        ParamValue::String(s.to_string())
    }
}

impl From<f64> for ParamValue {
    fn from(n: f64) -> Self {
        ParamValue::Number(n)
    }
}

impl From<i64> for ParamValue {
    fn from(n: i64) -> Self {
        ParamValue::Number(n as f64)
    }
}

impl From<bool> for ParamValue {
    fn from(b: bool) -> Self {
        ParamValue::Boolean(b)
    }
}

impl From<Vec<String>> for ParamValue {
    fn from(l: Vec<String>) -> Self {
        ParamValue::List(l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semantic_intent_builder() {
        let intent = SemanticIntent::new(IntentCategory::search(), "weather")
            .with_confidence(0.95)
            .with_method(DetectionMethod::ai("gpt-4o", 0.95))
            .with_param("location", ParamValue::from("Beijing"))
            .with_reasoning("User asked about weather, location extracted from input");

        assert_eq!(intent.intent_type, "weather");
        assert_eq!(intent.confidence, 0.95);
        assert!(intent.is_builtin());
        assert_eq!(intent.get_param_str("location"), Some("Beijing"));
    }

    #[test]
    fn test_semantic_intent_general() {
        let intent = SemanticIntent::general();

        assert_eq!(intent.category, IntentCategory::General);
        assert_eq!(intent.confidence, 1.0);
        assert!(!intent.is_builtin());
        assert!(!intent.is_skills());
    }

    #[test]
    fn test_intent_category_display() {
        assert_eq!(IntentCategory::search().to_string(), "builtin:search");
        assert_eq!(
            IntentCategory::command("translate").to_string(),
            "command:translate"
        );
        assert_eq!(
            IntentCategory::semantic("weather").to_string(),
            "semantic:weather"
        );
        assert_eq!(
            IntentCategory::skills("pdf").to_string(),
            "skills:pdf"
        );
        assert_eq!(IntentCategory::General.to_string(), "general");
    }

    #[test]
    fn test_detection_method_display() {
        assert_eq!(DetectionMethod::ExactCommand.to_string(), "exact_command");
        assert_eq!(
            DetectionMethod::keyword(0.85, vec!["weather".to_string()]).to_string(),
            "keyword_match(score=0.85)"
        );
        assert_eq!(
            DetectionMethod::context("pending_param").to_string(),
            "context_inference(pending_param)"
        );
        assert_eq!(
            DetectionMethod::ai("gpt-4o", 0.9).to_string(),
            "ai_detection(gpt-4o:0.90)"
        );
    }

    #[test]
    fn test_param_value_conversions() {
        let s: ParamValue = "test".into();
        assert_eq!(s.as_str(), Some("test"));

        let n: ParamValue = 42.0.into();
        assert_eq!(n.as_number(), Some(42.0));

        let b: ParamValue = true.into();
        assert_eq!(b.as_bool(), Some(true));

        let l: ParamValue = vec!["a".to_string(), "b".to_string()].into();
        assert_eq!(l.as_list().map(|v| v.len()), Some(2));
    }

    #[test]
    fn test_to_legacy_intent() {
        let search = SemanticIntent::new(IntentCategory::search(), "search");
        assert_eq!(search.to_legacy_intent(), Intent::BuiltinSearch);

        let command = SemanticIntent::new(IntentCategory::command("translate"), "translate");
        assert_eq!(
            command.to_legacy_intent(),
            Intent::Custom("translate".to_string())
        );

        let skills = SemanticIntent::new(IntentCategory::skills("pdf"), "pdf");
        assert_eq!(skills.to_legacy_intent(), Intent::Skills("pdf".to_string()));

        let general = SemanticIntent::general();
        assert_eq!(general.to_legacy_intent(), Intent::GeneralChat);
    }

    #[test]
    fn test_confidence_clamp() {
        let intent = SemanticIntent::general().with_confidence(1.5);
        assert_eq!(intent.confidence, 1.0);

        let intent = SemanticIntent::general().with_confidence(-0.5);
        assert_eq!(intent.confidence, 0.0);
    }
}
