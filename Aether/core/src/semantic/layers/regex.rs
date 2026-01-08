//! Regex matching layer (Layer 1b).
//!
//! This layer handles pattern-based regex matching for non-command rules.
//! Regex rules have priority 1 and can be terminal if confidence is high enough.

use crate::config::RoutingRuleConfig;
use crate::dispatcher::RoutingLayer;
use crate::error::Result;
use crate::payload::Capability;
use crate::semantic::context::MatchingContext;
use crate::semantic::intent::{DetectionMethod, IntentCategory, SemanticIntent};
use crate::semantic::layer::{LayerEnabledFlag, MatchingLayer};
use crate::semantic::matcher::MatchResult;
use async_trait::async_trait;
use regex::Regex;
use tracing::debug;

/// Regex matching layer
///
/// Matches input against regex patterns (non-command rules).
/// This layer has priority 1 and produces matches with configurable confidence.
pub struct RegexLayer {
    /// Compiled regex rules
    rules: Vec<CompiledRegexRule>,
    /// Enabled flag
    enabled: LayerEnabledFlag,
    /// Default confidence for regex matches
    default_confidence: f64,
}

impl RegexLayer {
    /// Create a new empty regex layer
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            enabled: LayerEnabledFlag::new(true),
            default_confidence: 0.9,
        }
    }

    /// Create with custom default confidence
    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.default_confidence = confidence;
        self
    }

    /// Create from routing rule configs (filters for non-command rules)
    pub fn from_rules(rules: &[RoutingRuleConfig]) -> Result<Self> {
        let mut layer = Self::new();

        for (index, rule) in rules.iter().enumerate() {
            if !rule.is_command_rule() {
                layer.add_rule(rule, index)?;
            }
        }

        Ok(layer)
    }

    /// Add a regex rule
    pub fn add_rule(&mut self, config: &RoutingRuleConfig, index: usize) -> Result<()> {
        let compiled = CompiledRegexRule::from_config(config, index)?;
        self.rules.push(compiled);
        Ok(())
    }

    /// Get rule count
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

impl Default for RegexLayer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MatchingLayer for RegexLayer {
    fn layer_id(&self) -> &str {
        "regex"
    }

    fn priority(&self) -> u32 {
        1 // Second priority after commands
    }

    fn is_enabled(&self) -> bool {
        self.enabled.is_enabled()
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.set_enabled(enabled);
    }

    fn is_terminal(&self) -> bool {
        true // Regex matches are terminal if confidence is high
    }

    fn confidence_threshold(&self) -> f64 {
        0.8 // Require reasonably high confidence
    }

    async fn try_match(&self, ctx: &MatchingContext) -> Option<MatchResult> {
        let input = &ctx.raw_input;

        for rule in &self.rules {
            if rule.regex.is_match(input) {
                let confidence = rule.confidence.unwrap_or(self.default_confidence);

                let intent = SemanticIntent::new(rule.category.clone(), rule.intent_type.clone())
                    .with_confidence(confidence)
                    .with_method(DetectionMethod::RegexPattern)
                    .with_capabilities(rule.capabilities.clone());

                let intent = if let Some(ref prompt) = rule.system_prompt {
                    intent.with_system_prompt(prompt.clone())
                } else {
                    intent
                };

                let intent = if let Some(ref provider) = rule.provider_name {
                    intent.with_provider(provider.clone())
                } else {
                    intent
                };

                debug!(
                    pattern = %rule.pattern,
                    intent = %rule.intent_type,
                    confidence = confidence,
                    "Regex layer matched"
                );

                return Some(
                    MatchResult::new(intent, confidence, RoutingLayer::L1Rule)
                        .with_rule_index(rule.index)
                );
            }
        }

        None
    }
}

/// Compiled regex rule for fast matching
#[derive(Debug, Clone)]
struct CompiledRegexRule {
    /// Compiled regex
    regex: Regex,
    /// Original pattern
    pattern: String,
    /// Rule index in config
    index: usize,
    /// Intent category
    category: IntentCategory,
    /// Intent type string
    intent_type: String,
    /// Provider name
    provider_name: Option<String>,
    /// System prompt
    system_prompt: Option<String>,
    /// Capabilities
    capabilities: Vec<Capability>,
    /// Optional confidence override
    confidence: Option<f64>,
}

impl CompiledRegexRule {
    /// Create from RoutingRuleConfig
    fn from_config(config: &RoutingRuleConfig, index: usize) -> Result<Self> {
        let regex = Regex::new(&config.regex).map_err(|e| {
            crate::error::AetherError::invalid_config(format!(
                "Invalid regex pattern '{}': {}",
                config.regex, e
            ))
        })?;

        let category = Self::determine_category(config);
        let intent_type = config
            .intent_type
            .clone()
            .unwrap_or_else(|| "general".to_string());

        Ok(Self {
            regex,
            pattern: config.regex.clone(),
            index,
            category,
            intent_type,
            provider_name: config.provider.clone(),
            system_prompt: config.system_prompt.clone(),
            capabilities: config.get_capabilities(),
            confidence: None, // Use layer default
        })
    }

    /// Determine intent category from config
    fn determine_category(config: &RoutingRuleConfig) -> IntentCategory {
        if let Some(ref intent_type) = config.intent_type {
            match intent_type.as_str() {
                "search" | "web_search" | "builtin_search" => {
                    IntentCategory::search()
                }
                "video" | "video_analysis" => IntentCategory::video(),
                "mcp" | "tool_call" | "builtin_mcp" => IntentCategory::mcp(),
                s if s.starts_with("skills:") => {
                    let id = s.strip_prefix("skills:").unwrap_or("");
                    IntentCategory::Skills(id.to_string())
                }
                _ => IntentCategory::Semantic(intent_type.clone()),
            }
        } else {
            IntentCategory::General
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_regex_layer_match() {
        let mut layer = RegexLayer::new();

        let config = RoutingRuleConfig {
            regex: r"translate.*to\s+(chinese|english)".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("You are a translator.".to_string()),
            intent_type: Some("translation".to_string()),
            ..Default::default()
        };

        layer.add_rule(&config, 0).unwrap();

        let ctx = MatchingContext::simple("translate this to chinese");
        let result = layer.try_match(&ctx).await;

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.confidence, 0.9);
        assert_eq!(result.intent.intent_type, "translation");
    }

    #[tokio::test]
    async fn test_regex_layer_no_match() {
        let mut layer = RegexLayer::new();

        let config = RoutingRuleConfig {
            regex: r"^/specific\s+pattern".to_string(),
            ..Default::default()
        };

        layer.add_rule(&config, 0).unwrap();

        let ctx = MatchingContext::simple("Hello world");
        let result = layer.try_match(&ctx).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_regex_layer_custom_confidence() {
        let mut layer = RegexLayer::new().with_confidence(0.85);

        let config = RoutingRuleConfig {
            regex: r"test".to_string(),
            ..Default::default()
        };

        layer.add_rule(&config, 0).unwrap();

        let ctx = MatchingContext::simple("this is a test");
        let result = layer.try_match(&ctx).await;

        assert!(result.is_some());
        assert_eq!(result.unwrap().confidence, 0.85);
    }

    #[test]
    fn test_regex_layer_properties() {
        let layer = RegexLayer::new();

        assert_eq!(layer.layer_id(), "regex");
        assert_eq!(layer.priority(), 1);
        assert!(layer.is_terminal());
        assert!(layer.is_enabled());
    }
}
