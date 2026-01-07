//! Intent detection module for smart conversation flow.
//!
//! This module provides intelligent detection of user intent and automatic
//! capability invocation (search, video, skills, mcp) based on input patterns.
//!
//! # Architecture
//!
//! The module has three layers:
//! - **AiIntentDetector**: AI-powered detection for language-agnostic classification
//! - **SmartTrigger**: Regex-based detection for builtin commands (fallback)
//! - **IntentDetector**: Legacy intent detection (backward compatibility)
//!
//! # Detection Flow
//!
//! ```text
//! User Input (any language)
//!     ↓
//! [1] Quick pre-check (URLs, obvious patterns)
//!     ↓
//! [2] AI Intent Detection (if enabled)
//!     ↓
//! [3] Regex fallback (SmartTrigger)
//!     ↓
//! Result: intent, params, missing_params
//! ```
//!
//! # Example
//!
//! ```ignore
//! // AI-powered detection (recommended for multi-language support)
//! let ai_detector = AiIntentDetector::new(provider);
//! let result = ai_detector.detect("¿Cómo está el clima en Madrid?").await?;
//!
//! // Regex-based detection (fallback)
//! let regex_detector = SmartTriggerDetector::new();
//! match regex_detector.detect("weather in Tokyo") {
//!     SmartTriggerResult::Ready { command, params, .. } => { /* ... */ }
//!     SmartTriggerResult::NeedsParam { param, .. } => { /* ... */ }
//!     SmartTriggerResult::NoMatch => { /* ... */ }
//! }
//! ```

pub mod ai_detector;
pub mod patterns;
pub mod smart_trigger;

use std::collections::HashMap;

pub use ai_detector::{AiIntentDetector, AiIntentResult};
pub use patterns::{
    builtin_intents, ClarificationTemplate, IntentConfig, IntentType, ParamDef,
};
pub use smart_trigger::{
    augment_with_param, builtin_triggers, enhance_query, LocalizedString, SmartParam,
    SmartTrigger, SmartTriggerDetector, SmartTriggerResult,
};

use crate::clarification::ClarificationRequest;
use crate::payload::Capability;

/// Result of intent detection.
#[derive(Debug, Clone)]
pub struct DetectedIntent {
    /// The type of intent detected
    pub intent_type: IntentType,
    /// Capability required for this intent (e.g., Search for weather)
    pub capability: Option<Capability>,
    /// Parameters extracted from user input
    pub extracted_params: HashMap<String, String>,
    /// Parameters that are required but missing
    pub missing_params: Vec<ParamDef>,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
}

impl DetectedIntent {
    /// Check if all required parameters are present.
    pub fn has_all_params(&self) -> bool {
        self.missing_params.is_empty()
    }

    /// Get the first missing parameter's clarification request.
    pub fn get_clarification_request(&self) -> Option<ClarificationRequest> {
        self.missing_params
            .first()
            .map(|p| p.to_clarification_request())
    }

    /// Inject a parameter value and return updated intent.
    pub fn with_param(mut self, name: &str, value: String) -> Self {
        self.extracted_params.insert(name.to_string(), value);
        self.missing_params.retain(|p| p.name != name);
        self
    }
}

/// Intent detector that analyzes user input.
pub struct IntentDetector {
    /// Registered intent configurations
    intents: Vec<IntentConfig>,
    /// Whether intent detection is enabled
    enabled: bool,
}

impl IntentDetector {
    /// Create a new intent detector with built-in intents.
    pub fn new() -> Self {
        IntentDetector {
            intents: builtin_intents(),
            enabled: true,
        }
    }

    /// Create an intent detector with custom intents.
    pub fn with_intents(intents: Vec<IntentConfig>) -> Self {
        IntentDetector {
            intents,
            enabled: true,
        }
    }

    /// Enable or disable intent detection.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Check if intent detection is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Add a custom intent configuration.
    pub fn add_intent(&mut self, intent: IntentConfig) {
        self.intents.push(intent);
        // Sort by priority
        self.intents.sort_by_key(|i| i.priority);
    }

    /// Detect intent from user input.
    ///
    /// Returns `Some(DetectedIntent)` if an intent is detected,
    /// `None` for general chat with no specific intent.
    pub fn detect(&self, input: &str) -> Option<DetectedIntent> {
        if !self.enabled {
            return None;
        }

        // Sort intents by priority and find first match
        let mut sorted_intents = self.intents.clone();
        sorted_intents.sort_by_key(|i| i.priority);

        for intent_config in &sorted_intents {
            if intent_config.matches(input) {
                // Extract parameters
                let extracted = intent_config.extract_params(input);
                let missing = intent_config.get_missing_params(input, &extracted);

                return Some(DetectedIntent {
                    intent_type: intent_config.intent_type.clone(),
                    capability: intent_config.capability.clone(),
                    extracted_params: extracted,
                    missing_params: missing.into_iter().cloned().collect(),
                    confidence: 0.8, // TODO: Calculate based on pattern match quality
                });
            }
        }

        None
    }

    /// Detect intent and return a list of all matching intents (for debugging).
    pub fn detect_all(&self, input: &str) -> Vec<DetectedIntent> {
        if !self.enabled {
            return Vec::new();
        }

        self.intents
            .iter()
            .filter(|config| config.matches(input))
            .map(|config| {
                let extracted = config.extract_params(input);
                let missing = config.get_missing_params(input, &extracted);

                DetectedIntent {
                    intent_type: config.intent_type.clone(),
                    capability: config.capability.clone(),
                    extracted_params: extracted,
                    missing_params: missing.into_iter().cloned().collect(),
                    confidence: 0.8,
                }
            })
            .collect()
    }
}

impl Default for IntentDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Augment user input with extracted/clarified parameters.
pub fn augment_input(
    original_input: &str,
    intent: &DetectedIntent,
    param_name: &str,
    param_value: &str,
) -> String {
    match intent.intent_type {
        IntentType::Weather => {
            if param_name == "location" {
                // Prepend location to weather query
                format!("{} {}", param_value, original_input)
            } else {
                original_input.to_string()
            }
        }
        IntentType::Translation => {
            if param_name == "target_language" {
                // Append target language
                format!("{} 翻译成{}", original_input, param_value)
            } else {
                original_input.to_string()
            }
        }
        _ => original_input.to_string(),
    }
}

/// Generate an enhanced search query for specific intents.
pub fn enhance_search_query(input: &str, intent: &DetectedIntent) -> String {
    match intent.intent_type {
        IntentType::Weather => {
            if let Some(location) = intent.extracted_params.get("location") {
                format!("{} 天气预报 今天 实时", location)
            } else {
                format!("{} 天气预报", input)
            }
        }
        _ => input.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intent_detector_weather() {
        let detector = IntentDetector::new();

        // Weather without location
        let result = detector.detect("今天天气怎么样");
        assert!(result.is_some());

        let detected = result.unwrap();
        assert_eq!(detected.intent_type, IntentType::Weather);
        assert!(detected.capability.is_some());
        assert_eq!(detected.capability.unwrap(), Capability::Search);
        assert!(!detected.has_all_params()); // location is missing
    }

    #[test]
    fn test_intent_detector_weather_with_location() {
        let detector = IntentDetector::new();

        // Weather with location
        let result = detector.detect("北京的天气怎么样");
        assert!(result.is_some());

        let detected = result.unwrap();
        assert_eq!(detected.intent_type, IntentType::Weather);
        assert_eq!(
            detected.extracted_params.get("location"),
            Some(&"北京".to_string())
        );
        assert!(detected.has_all_params()); // location is present
    }

    #[test]
    fn test_intent_detector_translation() {
        let detector = IntentDetector::new();

        let result = detector.detect("帮我翻译这段话");
        assert!(result.is_some());

        let detected = result.unwrap();
        assert_eq!(detected.intent_type, IntentType::Translation);
    }

    #[test]
    fn test_intent_detector_no_intent() {
        let detector = IntentDetector::new();

        let result = detector.detect("你好");
        assert!(result.is_none());
    }

    #[test]
    fn test_augment_input_weather() {
        let detected = DetectedIntent {
            intent_type: IntentType::Weather,
            capability: Some(Capability::Search),
            extracted_params: HashMap::new(),
            missing_params: vec![],
            confidence: 0.8,
        };

        let augmented = augment_input("今天天气怎么样", &detected, "location", "上海");
        assert_eq!(augmented, "上海 今天天气怎么样");
    }

    #[test]
    fn test_enhance_search_query() {
        let mut extracted = HashMap::new();
        extracted.insert("location".to_string(), "深圳".to_string());

        let detected = DetectedIntent {
            intent_type: IntentType::Weather,
            capability: Some(Capability::Search),
            extracted_params: extracted,
            missing_params: vec![],
            confidence: 0.8,
        };

        let enhanced = enhance_search_query("今天天气", &detected);
        assert!(enhanced.contains("深圳"));
        assert!(enhanced.contains("天气预报"));
    }

    #[test]
    fn test_detected_intent_with_param() {
        let detected = DetectedIntent {
            intent_type: IntentType::Weather,
            capability: Some(Capability::Search),
            extracted_params: HashMap::new(),
            missing_params: vec![ParamDef::required(
                "location",
                vec![],
                ClarificationTemplate::text("Enter location", None),
            )],
            confidence: 0.8,
        };

        assert!(!detected.has_all_params());

        let updated = detected.with_param("location", "北京".to_string());
        assert!(updated.has_all_params());
        assert_eq!(
            updated.extracted_params.get("location"),
            Some(&"北京".to_string())
        );
    }

    #[test]
    fn test_intent_detector_disabled() {
        let mut detector = IntentDetector::new();
        detector.set_enabled(false);

        let result = detector.detect("今天天气怎么样");
        assert!(result.is_none());
    }
}
