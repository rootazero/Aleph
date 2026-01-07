//! Pre-defined intent patterns for smart conversation flow.
//!
//! This module defines the intent types and their matching patterns.
//! Each intent can have required parameters that trigger clarification
//! when missing from user input.

use regex::Regex;
use std::collections::HashMap;

use crate::clarification::{ClarificationOption, ClarificationRequest};
use crate::payload::Capability;

/// Types of intents that can be detected from user input.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IntentType {
    /// Weather-related queries (天气, weather, 气温)
    Weather,
    /// Translation requests (翻译, translate)
    Translation,
    /// Code-related questions (代码, code, 编程)
    CodeHelp,
    /// General chat (no specific intent detected)
    General,
}

impl std::fmt::Display for IntentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntentType::Weather => write!(f, "weather"),
            IntentType::Translation => write!(f, "translation"),
            IntentType::CodeHelp => write!(f, "code_help"),
            IntentType::General => write!(f, "general"),
        }
    }
}

/// Template for generating clarification requests.
#[derive(Debug, Clone)]
pub enum ClarificationTemplate {
    /// Select from a list of options
    Select {
        prompt: String,
        options: Vec<(String, String)>, // (value, label)
    },
    /// Free-form text input
    Text {
        prompt: String,
        placeholder: Option<String>,
    },
}

impl ClarificationTemplate {
    /// Create a select clarification template.
    pub fn select(prompt: impl Into<String>, options: Vec<(&str, &str)>) -> Self {
        ClarificationTemplate::Select {
            prompt: prompt.into(),
            options: options
                .into_iter()
                .map(|(v, l)| (v.to_string(), l.to_string()))
                .collect(),
        }
    }

    /// Create a text input clarification template.
    pub fn text(prompt: impl Into<String>, placeholder: Option<&str>) -> Self {
        ClarificationTemplate::Text {
            prompt: prompt.into(),
            placeholder: placeholder.map(|s| s.to_string()),
        }
    }

    /// Convert template to a ClarificationRequest.
    pub fn to_request(&self, id: &str, source: Option<&str>) -> ClarificationRequest {
        match self {
            ClarificationTemplate::Select { prompt, options } => {
                let opts: Vec<ClarificationOption> = options
                    .iter()
                    .map(|(v, l)| ClarificationOption::new(v, l))
                    .collect();
                let mut req = ClarificationRequest::select(id, prompt, opts);
                if let Some(src) = source {
                    req = req.with_source(src);
                }
                req
            }
            ClarificationTemplate::Text {
                prompt,
                placeholder,
            } => {
                let mut req =
                    ClarificationRequest::text(id, prompt, placeholder.as_deref());
                if let Some(src) = source {
                    req = req.with_source(src);
                }
                req
            }
        }
    }
}

/// Definition of a required parameter for an intent.
#[derive(Debug, Clone)]
pub struct ParamDef {
    /// Parameter name (e.g., "location", "target_language")
    pub name: String,
    /// Patterns to extract the parameter from user input
    pub extraction_patterns: Vec<Regex>,
    /// Clarification template when parameter is missing
    pub clarification: ClarificationTemplate,
    /// Whether this parameter is optional
    pub optional: bool,
}

impl ParamDef {
    /// Create a new required parameter definition.
    pub fn required(
        name: impl Into<String>,
        extraction_patterns: Vec<&str>,
        clarification: ClarificationTemplate,
    ) -> Self {
        let patterns = extraction_patterns
            .into_iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        ParamDef {
            name: name.into(),
            extraction_patterns: patterns,
            clarification,
            optional: false,
        }
    }

    /// Create an optional parameter definition.
    pub fn optional(
        name: impl Into<String>,
        extraction_patterns: Vec<&str>,
        clarification: ClarificationTemplate,
    ) -> Self {
        let mut param = Self::required(name, extraction_patterns, clarification);
        param.optional = true;
        param
    }

    /// Try to extract parameter value from input.
    pub fn extract(&self, input: &str) -> Option<String> {
        for pattern in &self.extraction_patterns {
            if let Some(captures) = pattern.captures(input) {
                // Return the first captured group, or the whole match
                if let Some(m) = captures.get(1) {
                    return Some(m.as_str().trim().to_string());
                }
            }
        }
        None
    }

    /// Generate a clarification request for this parameter.
    pub fn to_clarification_request(&self) -> ClarificationRequest {
        self.clarification
            .to_request(&format!("param-{}", self.name), Some("intent:param"))
    }
}

/// Configuration for an intent pattern.
#[derive(Debug, Clone)]
pub struct IntentConfig {
    /// Type of intent
    pub intent_type: IntentType,
    /// Patterns that trigger this intent
    pub patterns: Vec<Regex>,
    /// Capability required for this intent (e.g., Search for weather)
    pub capability: Option<Capability>,
    /// Required parameters for this intent
    pub required_params: Vec<ParamDef>,
    /// Priority (lower = higher priority)
    pub priority: u8,
}

impl IntentConfig {
    /// Create a new intent configuration.
    pub fn new(intent_type: IntentType, patterns: Vec<&str>) -> Self {
        let compiled_patterns = patterns
            .into_iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        IntentConfig {
            intent_type,
            patterns: compiled_patterns,
            capability: None,
            required_params: Vec::new(),
            priority: 100,
        }
    }

    /// Set the capability for this intent.
    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.capability = Some(capability);
        self
    }

    /// Add required parameters.
    pub fn with_params(mut self, params: Vec<ParamDef>) -> Self {
        self.required_params = params;
        self
    }

    /// Set priority.
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Check if input matches this intent.
    pub fn matches(&self, input: &str) -> bool {
        self.patterns.iter().any(|p| p.is_match(input))
    }

    /// Extract all parameters from input.
    pub fn extract_params(&self, input: &str) -> HashMap<String, String> {
        let mut params = HashMap::new();
        for param_def in &self.required_params {
            if let Some(value) = param_def.extract(input) {
                params.insert(param_def.name.clone(), value);
            }
        }
        params
    }

    /// Get missing required parameters.
    pub fn get_missing_params(
        &self,
        _input: &str,
        extracted: &HashMap<String, String>,
    ) -> Vec<&ParamDef> {
        self.required_params
            .iter()
            .filter(|p| !p.optional && !extracted.contains_key(&p.name))
            .collect()
    }
}

/// Built-in intent patterns.
pub fn builtin_intents() -> Vec<IntentConfig> {
    vec![
        // Weather intent
        IntentConfig::new(
            IntentType::Weather,
            vec![
                r"(?i)(天气|weather|气温|温度|下雨|晴天|阴天|刮风|下雪)",
                r"(?i)(今天|明天|后天|这周|本周).*(天气|气温)",
            ],
        )
        .with_capability(Capability::Search)
        .with_params(vec![ParamDef::required(
            "location",
            vec![
                // Match city name before time words and weather keywords
                // Must start from beginning of string to avoid matching partial words
                // e.g., "上海今天天气" -> "上海", "北京的天气" -> "北京"
                r"^([^今明后这的天气怎样\s]{2,})(?:今天|明天|后天|这周|的)?(?:的天气|天气|的气温|气温)",
                // Match explicit "在X" or "X的" patterns
                r"^在(.{2,}?)(?:的天气|天气)",
                r"^(.{2,}?)的(?:天气|气温)",
            ],
            ClarificationTemplate::select(
                "请选择城市",
                vec![
                    ("北京", "北京"),
                    ("上海", "上海"),
                    ("深圳", "深圳"),
                    ("杭州", "杭州"),
                    ("广州", "广州"),
                ],
            ),
        )])
        .with_priority(10),
        // Translation intent
        IntentConfig::new(
            IntentType::Translation,
            vec![
                r"(?i)(翻译|translate|转换|convert)",
                r"(?i)(用|以)(.+?)(说|写|表达)",
            ],
        )
        .with_params(vec![ParamDef::optional(
            "target_language",
            vec![
                r"(?:翻译成|translate to|转换为|译成)\s*([中文|英文|日文|韩文|法文|德文|西班牙文]+)",
                r"(?:用|以)\s*([中文|英文|日文|韩文|法文|德文|西班牙文]+)",
            ],
            ClarificationTemplate::select(
                "翻译成哪种语言?",
                vec![
                    ("英文", "English"),
                    ("中文", "中文"),
                    ("日文", "日本語"),
                    ("韩文", "한국어"),
                    ("法文", "Français"),
                ],
            ),
        )])
        .with_priority(20),
        // Code help intent
        IntentConfig::new(
            IntentType::CodeHelp,
            vec![
                r"(?i)(代码|code|编程|program|bug|错误|报错)",
                r"(?i)(怎么写|how to|实现|implement)",
            ],
        )
        .with_params(vec![ParamDef::optional(
            "language",
            vec![
                r"(?i)(rust|python|javascript|typescript|swift|java|go|c\+\+|ruby)",
                r"(?:用|使用)\s*([a-zA-Z]+)",
            ],
            ClarificationTemplate::select(
                "使用哪种编程语言?",
                vec![
                    ("rust", "Rust"),
                    ("python", "Python"),
                    ("javascript", "JavaScript"),
                    ("typescript", "TypeScript"),
                    ("swift", "Swift"),
                ],
            ),
        )])
        .with_priority(30),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weather_intent_detection() {
        let intents = builtin_intents();
        let weather = &intents[0];

        assert!(weather.matches("今天天气怎么样"));
        assert!(weather.matches("北京的天气"));
        assert!(weather.matches("明天会下雨吗"));
        assert!(!weather.matches("你好"));
    }

    #[test]
    fn test_weather_location_extraction() {
        let intents = builtin_intents();
        let weather = &intents[0];

        let params = weather.extract_params("北京的天气怎么样");
        assert_eq!(params.get("location"), Some(&"北京".to_string()));

        let params = weather.extract_params("上海今天天气");
        assert_eq!(params.get("location"), Some(&"上海".to_string()));
    }

    #[test]
    fn test_translation_intent_detection() {
        let intents = builtin_intents();
        let translation = &intents[1];

        assert!(translation.matches("帮我翻译这段话"));
        assert!(translation.matches("translate this"));
        assert!(!translation.matches("你好"));
    }

    #[test]
    fn test_clarification_template() {
        let template = ClarificationTemplate::select(
            "Choose city",
            vec![("bj", "Beijing"), ("sh", "Shanghai")],
        );

        let request = template.to_request("test-id", Some("test"));
        assert_eq!(request.id, "test-id");
        assert_eq!(request.prompt, "Choose city");
        assert_eq!(request.source, Some("test".to_string()));
    }
}
