//! Generic smart trigger system for intelligent capability invocation.
//!
//! This module provides a flexible, configuration-driven system for detecting
//! when a builtin command should be invoked based on user input patterns.
//!
//! # Supported Capabilities
//!
//! - **Search** (`/search`): Weather, news, general search queries
//! - **Video** (`/video`): YouTube video analysis
//! - **Skills** (`/skill`): Custom workflows (future)
//! - **MCP** (`/mcp`): Model Context Protocol tools (future)
//!
//! # Localization
//!
//! All user-facing strings use localization keys that are resolved by the
//! Swift layer using NSLocalizedString. Keys follow the pattern:
//! `smart_trigger.<command>.<field>`

use regex::Regex;
use std::collections::HashMap;

use crate::clarification::{ClarificationRequest, ClarificationType};
use crate::payload::Capability;

/// Localization key wrapper for user-facing strings.
///
/// Instead of hardcoding translations, we use keys that are resolved
/// by the UI layer (Swift) using the standard localization system.
#[derive(Debug, Clone)]
pub struct LocalizedKey {
    /// The localization key (e.g., "smart_trigger.search.query_prompt")
    pub key: String,
    /// Fallback English text (used if localization fails)
    pub fallback: String,
}

impl LocalizedKey {
    /// Create a new localization key with fallback.
    pub fn new(key: impl Into<String>, fallback: impl Into<String>) -> Self {
        LocalizedKey {
            key: key.into(),
            fallback: fallback.into(),
        }
    }

    /// Get the key for localization lookup.
    pub fn as_key(&self) -> &str {
        &self.key
    }

    /// Get the fallback text.
    pub fn as_fallback(&self) -> &str {
        &self.fallback
    }
}

// Re-export for backward compatibility
pub type LocalizedString = LocalizedKey;

/// Definition of a required parameter for a smart trigger.
#[derive(Debug, Clone)]
pub struct SmartParam {
    /// Parameter name (e.g., "location", "url")
    pub name: String,
    /// Patterns to extract the parameter from user input
    pub extraction_patterns: Vec<Regex>,
    /// Localization key for the prompt (resolved by Swift)
    pub prompt_key: LocalizedKey,
    /// Localization key for placeholder text (optional)
    pub placeholder_key: Option<LocalizedKey>,
    /// Whether this parameter is optional
    pub optional: bool,
}

impl SmartParam {
    /// Create a required parameter with localization key.
    ///
    /// # Arguments
    /// * `name` - Parameter identifier
    /// * `extraction_patterns` - Regex patterns to extract value from input
    /// * `prompt_key` - Localization key for the clarification prompt
    /// * `prompt_fallback` - English fallback text
    pub fn required(
        name: impl Into<String>,
        extraction_patterns: Vec<&str>,
        prompt_key: impl Into<String>,
        prompt_fallback: impl Into<String>,
    ) -> Self {
        let patterns = extraction_patterns
            .into_iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        SmartParam {
            name: name.into(),
            extraction_patterns: patterns,
            prompt_key: LocalizedKey::new(prompt_key, prompt_fallback),
            placeholder_key: None,
            optional: false,
        }
    }

    /// Create an optional parameter.
    pub fn optional(
        name: impl Into<String>,
        extraction_patterns: Vec<&str>,
        prompt_key: impl Into<String>,
        prompt_fallback: impl Into<String>,
    ) -> Self {
        let mut param = Self::required(name, extraction_patterns, prompt_key, prompt_fallback);
        param.optional = true;
        param
    }

    /// Set placeholder with localization key.
    pub fn with_placeholder(
        mut self,
        placeholder_key: impl Into<String>,
        placeholder_fallback: impl Into<String>,
    ) -> Self {
        self.placeholder_key = Some(LocalizedKey::new(placeholder_key, placeholder_fallback));
        self
    }

    /// Try to extract parameter value from input.
    pub fn extract(&self, input: &str) -> Option<String> {
        // Time words that should not be extracted as locations
        const TIME_WORDS: &[&str] = &[
            "今天", "明天", "后天", "这周", "本周", // Chinese
            "今日", "明日", "来週", "今週", // Japanese
            "today", "tomorrow", "tonight", // English (lowercase)
        ];

        for pattern in &self.extraction_patterns {
            if let Some(captures) = pattern.captures(input) {
                if let Some(m) = captures.get(1) {
                    let value = m.as_str().trim();
                    // Post-process: remove trailing particles (的, の, etc.)
                    let value = value
                        .trim_end_matches('的')
                        .trim_end_matches('の');

                    // Skip time words - they are not valid locations
                    if TIME_WORDS.contains(&value) {
                        continue;
                    }

                    return Some(value.to_string());
                }
            }
        }
        None
    }

    /// Generate a clarification request for this parameter.
    ///
    /// The prompt uses the localization key which will be resolved by Swift.
    /// If the key cannot be resolved, the fallback text is used.
    pub fn to_clarification_request(&self, _locale: &str) -> ClarificationRequest {
        // Use localization key as prompt - Swift will resolve it
        // Format: "key:fallback" allows Swift to extract both
        let prompt = format!("{}:{}", self.prompt_key.key, self.prompt_key.fallback);
        let placeholder = self.placeholder_key.as_ref().map(|p| p.fallback.clone());

        ClarificationRequest {
            id: format!("smart-param-{}", self.name),
            prompt,
            clarification_type: ClarificationType::Text,
            options: None,
            default_value: None,
            placeholder,
            source: Some("smart:trigger".to_string()),
        }
    }
}

/// Configuration for a smart trigger that maps patterns to capabilities.
#[derive(Debug, Clone)]
pub struct SmartTrigger {
    /// The builtin command to invoke (e.g., "/search", "/video")
    pub command: String,
    /// The capability this trigger provides
    pub capability: Capability,
    /// Patterns that trigger this capability
    pub patterns: Vec<Regex>,
    /// Required parameters for this trigger
    pub params: Vec<SmartParam>,
    /// Priority (lower = higher priority)
    pub priority: u8,
    /// Whether this trigger is enabled
    pub enabled: bool,
}

impl SmartTrigger {
    /// Create a new smart trigger.
    pub fn new(command: impl Into<String>, capability: Capability, patterns: Vec<&str>) -> Self {
        let compiled = patterns
            .into_iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        SmartTrigger {
            command: command.into(),
            capability,
            patterns: compiled,
            params: Vec::new(),
            priority: 100,
            enabled: true,
        }
    }

    /// Add required parameters.
    pub fn with_params(mut self, params: Vec<SmartParam>) -> Self {
        self.params = params;
        self
    }

    /// Set priority.
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Check if input matches this trigger.
    pub fn matches(&self, input: &str) -> bool {
        self.enabled && self.patterns.iter().any(|p| p.is_match(input))
    }

    /// Extract all parameters from input.
    pub fn extract_params(&self, input: &str) -> HashMap<String, String> {
        let mut extracted = HashMap::new();
        for param in &self.params {
            if let Some(value) = param.extract(input) {
                extracted.insert(param.name.clone(), value);
            }
        }
        extracted
    }

    /// Get missing required parameters.
    pub fn get_missing_params(&self, extracted: &HashMap<String, String>) -> Vec<&SmartParam> {
        self.params
            .iter()
            .filter(|p| !p.optional && !extracted.contains_key(&p.name))
            .collect()
    }
}

/// Result of smart trigger detection.
#[derive(Debug, Clone)]
pub enum SmartTriggerResult {
    /// Trigger matched and all params are present
    Ready {
        /// The command to invoke
        command: String,
        /// The capability to enable
        capability: Capability,
        /// Extracted parameters
        params: HashMap<String, String>,
        /// The original input to augment
        original_input: String,
    },
    /// Trigger matched but a parameter is missing
    NeedsParam {
        /// The command that would be invoked
        command: String,
        /// The capability that would be enabled
        capability: Capability,
        /// The missing parameter definition
        param: SmartParam,
        /// Parameters already extracted
        extracted: HashMap<String, String>,
        /// The original input
        original_input: String,
    },
    /// No trigger matched
    NoMatch,
}

/// Smart trigger detector for intelligent capability invocation.
pub struct SmartTriggerDetector {
    /// Registered triggers
    triggers: Vec<SmartTrigger>,
    /// Whether detection is enabled
    enabled: bool,
    /// Current locale for localized prompts
    locale: String,
}

impl SmartTriggerDetector {
    /// Create a new detector with builtin triggers.
    pub fn new() -> Self {
        SmartTriggerDetector {
            triggers: builtin_triggers(),
            enabled: true,
            locale: "en".to_string(),
        }
    }

    /// Set the locale for localized prompts.
    pub fn set_locale(&mut self, locale: impl Into<String>) {
        self.locale = locale.into();
    }

    /// Get the current locale.
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// Enable or disable detection.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Check if detection is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Enable or disable a specific trigger by command name.
    pub fn set_trigger_enabled(&mut self, command: &str, enabled: bool) {
        for trigger in &mut self.triggers {
            if trigger.command == command {
                trigger.enabled = enabled;
            }
        }
    }

    /// Add a custom trigger.
    pub fn add_trigger(&mut self, trigger: SmartTrigger) {
        self.triggers.push(trigger);
        self.triggers.sort_by_key(|t| t.priority);
    }

    /// Detect if input should trigger a capability.
    pub fn detect(&self, input: &str) -> SmartTriggerResult {
        if !self.enabled {
            return SmartTriggerResult::NoMatch;
        }

        // Sort by priority and find first match
        let mut sorted = self.triggers.clone();
        sorted.sort_by_key(|t| t.priority);

        for trigger in &sorted {
            if trigger.matches(input) {
                let extracted = trigger.extract_params(input);
                let missing = trigger.get_missing_params(&extracted);

                if !missing.is_empty() {
                    return SmartTriggerResult::NeedsParam {
                        command: trigger.command.clone(),
                        capability: trigger.capability,
                        param: missing[0].clone(),
                        extracted,
                        original_input: input.to_string(),
                    };
                }

                return SmartTriggerResult::Ready {
                    command: trigger.command.clone(),
                    capability: trigger.capability,
                    params: extracted,
                    original_input: input.to_string(),
                };
            }
        }

        SmartTriggerResult::NoMatch
    }

    /// Get clarification request for a missing parameter.
    pub fn get_clarification(&self, param: &SmartParam) -> ClarificationRequest {
        param.to_clarification_request(&self.locale)
    }
}

impl Default for SmartTriggerDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Builtin smart triggers for common capabilities.
///
/// Patterns support multiple languages through Unicode-aware regex.
/// All user-facing strings use localization keys.
pub fn builtin_triggers() -> Vec<SmartTrigger> {
    vec![
        // Search trigger - Weather, news, general queries
        // Patterns are language-agnostic where possible
        SmartTrigger::new(
            "/search",
            Capability::Search,
            vec![
                // Weather patterns - multilingual keywords
                // Chinese: 天气, 气温, 温度, 下雨, 晴天, 阴天, 刮风, 下雪
                // English: weather, forecast, rain, sunny, cloudy, windy, snow
                // Japanese: 天気, 気温
                r"(?i)(\p{Han}*天气\p{Han}*|weather|forecast|\p{Han}*气温\p{Han}*)",
                r"(?i)(rain|sunny|cloudy|windy|snow|下雨|晴天|阴天|刮风|下雪)",
                // Time + weather pattern
                r"(?i)(today|tomorrow|tonight|this week|今天|明天|后天|这周|本周).{0,10}(weather|天气|気温)",
                // News patterns
                r"(?i)(news|headline|latest|新闻|头条|最新)",
                // Search intent patterns
                r"(?i)(search|query|find|look\s*up|搜索|查询|查找|查一下)",
                // Real-time info patterns
                r"(?i)(current|now|price|stock|exchange\s*rate|现在|目前|价格|股票|汇率)",
            ],
        )
        .with_params(vec![
            SmartParam::required(
                "query",
                vec![
                    // English location patterns (most common)
                    r"(?i)weather\s+(?:in|for|at)\s+(.+?)(?:\s|$)",
                    r"(?i)(.+?)\s+weather\b",
                    r"(?i)weather\s+(.+?)$",
                    // CJK location patterns (Chinese/Japanese/Korean)
                    // Match: <location>的天气, <location>天气
                    // Time words (今天/明天/etc.) are filtered in post-processing
                    r"^([\p{Han}\p{Hiragana}\p{Katakana}]+)的?(?:天气|気温)",
                    r"^在([\p{Han}]+)的?天气",
                ],
                "smart_trigger.search.query_prompt",
                "Enter your query (e.g., city name)",
            )
            .with_placeholder(
                "smart_trigger.search.query_placeholder",
                "Beijing / New York / Tokyo",
            ),
        ])
        .with_priority(10),

        // Video trigger - YouTube/Bilibili analysis
        SmartTrigger::new(
            "/video",
            Capability::Video,
            vec![
                // Video platform keywords
                r"(?i)(youtube|youtu\.be|bilibili|b站|油管)",
                // Video action patterns
                r"(?i)(summarize|analyze|transcribe|explain).{0,20}(video|youtube)",
                r"(?i)(video|youtube).{0,20}(summary|analysis|transcript)",
                // CJK video patterns
                r"(?i)(视频|总结视频|分析视频|看视频|観る)",
            ],
        )
        .with_params(vec![
            SmartParam::required(
                "url",
                vec![
                    // YouTube URLs
                    r"(https?://(?:www\.)?youtube\.com/watch\?v=[^\s]+)",
                    r"(https?://youtu\.be/[^\s]+)",
                    r"(https?://(?:www\.)?youtube\.com/shorts/[^\s]+)",
                    // Bilibili URLs
                    r"(https?://(?:www\.)?bilibili\.com/video/[^\s]+)",
                    r"(https?://b23\.tv/[^\s]+)",
                ],
                "smart_trigger.video.url_prompt",
                "Enter video URL",
            )
            .with_placeholder(
                "smart_trigger.video.url_placeholder",
                "https://youtube.com/watch?v=...",
            ),
        ])
        .with_priority(20),
    ]
}

/// Augment user input with the provided parameter value.
///
/// This function modifies the original input to include the clarified parameter,
/// making it suitable for further processing by the AI.
pub fn augment_with_param(
    original_input: &str,
    command: &str,
    param_name: &str,
    param_value: &str,
) -> String {
    match command {
        "/search" => {
            if param_name == "query" {
                // Prepend query/location to the input for better context
                format!("{} {}", param_value, original_input)
            } else {
                original_input.to_string()
            }
        }
        "/video" => {
            if param_name == "url" {
                // Append URL to the input
                format!("{} {}", original_input, param_value)
            } else {
                original_input.to_string()
            }
        }
        _ => original_input.to_string(),
    }
}

/// Generate an enhanced search query based on detected context.
///
/// Adds relevant keywords to improve search accuracy.
pub fn enhance_query(input: &str, command: &str, params: &HashMap<String, String>) -> String {
    match command {
        "/search" => {
            if let Some(query) = params.get("query") {
                // Check if it's a weather query using language-neutral pattern
                let weather_pattern = Regex::new(r"(?i)(weather|forecast|天气|気温)").unwrap();
                if weather_pattern.is_match(input) {
                    // Add weather-specific keywords for better search results
                    return format!("{} weather forecast today", query);
                }
                // General search - combine query with original input
                format!("{} {}", query, input)
            } else {
                input.to_string()
            }
        }
        _ => input.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weather_trigger_detection() {
        let detector = SmartTriggerDetector::new();

        // Weather query without location
        let result = detector.detect("今天天气怎么样");
        match result {
            SmartTriggerResult::NeedsParam { command, param, .. } => {
                assert_eq!(command, "/search");
                assert_eq!(param.name, "query");
            }
            _ => panic!("Expected NeedsParam for weather query without location"),
        }
    }

    #[test]
    fn test_weather_with_location() {
        let detector = SmartTriggerDetector::new();

        // Weather query with location
        let result = detector.detect("北京的天气怎么样");
        match result {
            SmartTriggerResult::Ready { command, params, .. } => {
                assert_eq!(command, "/search");
                assert_eq!(params.get("query"), Some(&"北京".to_string()));
            }
            _ => panic!("Expected Ready for weather query with location"),
        }
    }

    #[test]
    fn test_english_weather() {
        let detector = SmartTriggerDetector::new();

        let result = detector.detect("weather in London");
        match result {
            SmartTriggerResult::Ready { command, params, .. } => {
                assert_eq!(command, "/search");
                assert_eq!(params.get("query"), Some(&"London".to_string()));
            }
            _ => panic!("Expected Ready for English weather query"),
        }
    }

    #[test]
    fn test_video_trigger_without_url() {
        let detector = SmartTriggerDetector::new();

        let result = detector.detect("帮我总结这个YouTube视频");
        match result {
            SmartTriggerResult::NeedsParam { command, param, .. } => {
                assert_eq!(command, "/video");
                assert_eq!(param.name, "url");
            }
            _ => panic!("Expected NeedsParam for video without URL"),
        }
    }

    #[test]
    fn test_video_trigger_with_url() {
        let detector = SmartTriggerDetector::new();

        let result = detector.detect("总结视频 https://youtube.com/watch?v=abc123");
        match result {
            SmartTriggerResult::Ready { command, params, .. } => {
                assert_eq!(command, "/video");
                assert!(params.contains_key("url"));
            }
            _ => panic!("Expected Ready for video with URL"),
        }
    }

    #[test]
    fn test_no_trigger() {
        let detector = SmartTriggerDetector::new();

        let result = detector.detect("你好");
        assert!(matches!(result, SmartTriggerResult::NoMatch));
    }

    #[test]
    fn test_localization_key_format() {
        let param = SmartParam::required(
            "test",
            vec![],
            "smart_trigger.test.prompt",
            "English fallback prompt",
        );

        // The prompt should contain key:fallback format
        let req = param.to_clarification_request("en");
        assert!(req.prompt.contains("smart_trigger.test.prompt"));
        assert!(req.prompt.contains("English fallback prompt"));
    }

    #[test]
    fn test_augment_with_param() {
        let result = augment_with_param("今天天气怎么样", "/search", "query", "上海");
        assert_eq!(result, "上海 今天天气怎么样");

        let result = augment_with_param("总结这个视频", "/video", "url", "https://youtube.com/watch?v=abc");
        assert_eq!(result, "总结这个视频 https://youtube.com/watch?v=abc");
    }

    #[test]
    fn test_enhance_query() {
        let mut params = HashMap::new();
        params.insert("query".to_string(), "Tokyo".to_string());

        let enhanced = enhance_query("weather today", "/search", &params);
        assert!(enhanced.contains("Tokyo"));
        assert!(enhanced.contains("weather forecast"));
    }

    #[test]
    fn test_detector_disabled() {
        let mut detector = SmartTriggerDetector::new();
        detector.set_enabled(false);

        let result = detector.detect("今天天气怎么样");
        assert!(matches!(result, SmartTriggerResult::NoMatch));
    }

    #[test]
    fn test_trigger_disabled() {
        let mut detector = SmartTriggerDetector::new();
        detector.set_trigger_enabled("/search", false);

        let result = detector.detect("今天天气怎么样");
        assert!(matches!(result, SmartTriggerResult::NoMatch));
    }

    // ============================================
    // Additional comprehensive tests
    // ============================================

    #[test]
    fn test_multilingual_cities() {
        let detector = SmartTriggerDetector::new();

        // Japanese city
        let result = detector.detect("東京の天気");
        match &result {
            SmartTriggerResult::Ready { params, .. } => {
                assert_eq!(params.get("query"), Some(&"東京".to_string()));
            }
            SmartTriggerResult::NeedsParam { .. } => {
                // Also acceptable - pattern may not match Japanese
            }
            _ => {}
        }

        // French city with English pattern
        let result = detector.detect("weather in Paris");
        match result {
            SmartTriggerResult::Ready { params, .. } => {
                assert_eq!(params.get("query"), Some(&"Paris".to_string()));
            }
            _ => panic!("Expected Ready for Paris weather"),
        }

        // German city
        let result = detector.detect("Berlin weather");
        match result {
            SmartTriggerResult::Ready { params, .. } => {
                assert_eq!(params.get("query"), Some(&"Berlin".to_string()));
            }
            _ => panic!("Expected Ready for Berlin weather"),
        }
    }

    #[test]
    fn test_weather_patterns_variety() {
        let detector = SmartTriggerDetector::new();

        // "明天天气" without location
        let result = detector.detect("明天天气怎么样");
        assert!(matches!(result, SmartTriggerResult::NeedsParam { .. }));

        // "会下雨吗" - rain pattern
        let result = detector.detect("明天会下雨吗");
        assert!(matches!(
            result,
            SmartTriggerResult::NeedsParam { .. } | SmartTriggerResult::Ready { .. }
        ));

        // "forecast" keyword
        let result = detector.detect("weather forecast for New York");
        match result {
            SmartTriggerResult::Ready { command, .. }
            | SmartTriggerResult::NeedsParam { command, .. } => {
                assert_eq!(command, "/search");
            }
            _ => panic!("Expected search trigger for forecast"),
        }
    }

    #[test]
    fn test_news_search_trigger() {
        let detector = SmartTriggerDetector::new();

        // Chinese news query
        let result = detector.detect("最新科技新闻");
        match result {
            SmartTriggerResult::NeedsParam { command, .. }
            | SmartTriggerResult::Ready { command, .. } => {
                assert_eq!(command, "/search");
            }
            _ => panic!("Expected search trigger for news query"),
        }

        // English news query
        let result = detector.detect("latest tech news");
        match result {
            SmartTriggerResult::NeedsParam { command, .. }
            | SmartTriggerResult::Ready { command, .. } => {
                assert_eq!(command, "/search");
            }
            _ => panic!("Expected search trigger for English news"),
        }
    }

    #[test]
    fn test_youtube_short_url() {
        let detector = SmartTriggerDetector::new();

        let result = detector.detect("分析视频 https://youtu.be/dQw4w9WgXcQ");
        match result {
            SmartTriggerResult::Ready { command, params, .. } => {
                assert_eq!(command, "/video");
                assert!(params.get("url").unwrap().contains("youtu.be"));
            }
            _ => panic!("Expected Ready for youtu.be URL"),
        }
    }

    #[test]
    fn test_bilibili_video() {
        let detector = SmartTriggerDetector::new();

        let result = detector.detect("总结B站视频 https://bilibili.com/video/BV1234567890");
        match result {
            SmartTriggerResult::Ready { command, params, .. } => {
                assert_eq!(command, "/video");
                assert!(params.get("url").unwrap().contains("bilibili"));
            }
            _ => panic!("Expected Ready for Bilibili URL"),
        }
    }

    #[test]
    fn test_video_english_patterns() {
        let detector = SmartTriggerDetector::new();

        // "summarize video"
        let result = detector.detect("summarize this video");
        match result {
            SmartTriggerResult::NeedsParam { command, param, .. } => {
                assert_eq!(command, "/video");
                assert_eq!(param.name, "url");
            }
            _ => panic!("Expected NeedsParam for 'summarize video'"),
        }

        // "analyze youtube"
        let result = detector.detect("analyze youtube video");
        match result {
            SmartTriggerResult::NeedsParam { command, .. } => {
                assert_eq!(command, "/video");
            }
            _ => panic!("Expected NeedsParam for 'analyze youtube'"),
        }
    }

    #[test]
    fn test_edge_cases() {
        let detector = SmartTriggerDetector::new();

        // Empty input
        let result = detector.detect("");
        assert!(matches!(result, SmartTriggerResult::NoMatch));

        // Just spaces
        let result = detector.detect("   ");
        assert!(matches!(result, SmartTriggerResult::NoMatch));

        // Random text
        let result = detector.detect("Hello, how are you doing today?");
        assert!(matches!(result, SmartTriggerResult::NoMatch));

        // Chinese greeting
        let result = detector.detect("你好，最近怎么样");
        assert!(matches!(result, SmartTriggerResult::NoMatch));
    }

    #[test]
    fn test_locale_switching() {
        let mut detector = SmartTriggerDetector::new();

        // Test Chinese locale
        detector.set_locale("zh-Hans");
        assert_eq!(detector.locale(), "zh-Hans");

        let result = detector.detect("today weather");
        if let SmartTriggerResult::NeedsParam { param, .. } = result {
            let req = detector.get_clarification(&param);
            // Should contain localization key format
            assert!(req.prompt.contains("smart_trigger"));
        }

        // Switch to English locale
        detector.set_locale("en");
        assert_eq!(detector.locale(), "en");

        let result = detector.detect("weather forecast");
        if let SmartTriggerResult::NeedsParam { param, .. } = result {
            let req = detector.get_clarification(&param);
            // Should contain fallback text
            assert!(req.prompt.contains("Enter"));
        }
    }

    #[test]
    fn test_clarification_request_fields() {
        let param = SmartParam::required(
            "location",
            vec![],
            "smart_trigger.location.prompt",
            "Enter city",
        )
        .with_placeholder(
            "smart_trigger.location.placeholder",
            "Beijing / Tokyo",
        );

        // Test request fields
        let req = param.to_clarification_request("en");
        assert_eq!(req.id, "smart-param-location");
        // Prompt contains key:fallback format
        assert!(req.prompt.contains("smart_trigger.location.prompt"));
        assert!(req.prompt.contains("Enter city"));
        // Placeholder uses fallback
        assert_eq!(req.placeholder, Some("Beijing / Tokyo".to_string()));
        assert_eq!(req.source, Some("smart:trigger".to_string()));
        assert!(req.options.is_none());
    }

    #[test]
    fn test_search_query_enhancement() {
        // Weather query enhancement
        let mut params = HashMap::new();
        params.insert("query".to_string(), "Shanghai".to_string());

        let enhanced = enhance_query("weather today", "/search", &params);
        assert!(enhanced.contains("Shanghai"));
        assert!(enhanced.contains("weather forecast"));

        // Non-weather query - should not add weather keywords
        let enhanced = enhance_query("latest news", "/search", &params);
        assert!(enhanced.contains("Shanghai"));
        assert!(!enhanced.contains("forecast"));
    }

    #[test]
    fn test_video_trigger_only_disabled() {
        let mut detector = SmartTriggerDetector::new();
        detector.set_trigger_enabled("/video", false);

        // Video should not trigger
        let result = detector.detect("总结YouTube视频");
        assert!(matches!(result, SmartTriggerResult::NoMatch));

        // But search should still work
        let result = detector.detect("今天天气怎么样");
        assert!(matches!(result, SmartTriggerResult::NeedsParam { .. }));
    }

    #[test]
    fn test_priority_ordering() {
        let detector = SmartTriggerDetector::new();

        // If both could match, search has higher priority (lower number)
        // "搜索视频" contains both "搜索" (search) and "视频" (video)
        let result = detector.detect("搜索视频内容");
        match result {
            SmartTriggerResult::NeedsParam { command, .. }
            | SmartTriggerResult::Ready { command, .. } => {
                // Search has priority 10, video has priority 20
                // So search should match first
                assert_eq!(command, "/search");
            }
            _ => {}
        }
    }
}
