//! Command matching layer (Layer 1a).
//!
//! This layer handles exact ^/xxx command pattern matching.
//! Commands have the highest priority (0) and are terminal matches.

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

/// Command matching layer
///
/// Matches input against ^/xxx command patterns.
/// This is the highest priority layer and produces terminal matches.
pub struct CommandLayer {
    /// Compiled command rules
    rules: Vec<CompiledCommandRule>,
    /// Enabled flag
    enabled: LayerEnabledFlag,
}

impl CommandLayer {
    /// Create a new empty command layer
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            enabled: LayerEnabledFlag::new(true),
        }
    }

    /// Create from routing rule configs (filters for command rules only)
    pub fn from_rules(rules: &[RoutingRuleConfig]) -> Result<Self> {
        let mut layer = Self::new();

        for (index, rule) in rules.iter().enumerate() {
            if rule.is_command_rule() {
                layer.add_rule(rule, index)?;
            }
        }

        Ok(layer)
    }

    /// Add a command rule
    pub fn add_rule(&mut self, config: &RoutingRuleConfig, index: usize) -> Result<()> {
        let compiled = CompiledCommandRule::from_config(config, index)?;
        self.rules.push(compiled);
        Ok(())
    }

    /// Get rule count
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

impl Default for CommandLayer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MatchingLayer for CommandLayer {
    fn layer_id(&self) -> &str {
        "command"
    }

    fn priority(&self) -> u32 {
        0 // Highest priority
    }

    fn is_enabled(&self) -> bool {
        self.enabled.is_enabled()
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.set_enabled(enabled);
    }

    fn is_terminal(&self) -> bool {
        true // Command matches are always terminal
    }

    fn confidence_threshold(&self) -> f64 {
        1.0 // Commands require exact match
    }

    async fn try_match(&self, ctx: &MatchingContext) -> Option<MatchResult> {
        let input = &ctx.raw_input;

        for rule in &self.rules {
            if rule.regex.is_match(input) {
                let cleaned_input = rule.strip_prefix(input);

                let intent = SemanticIntent::new(rule.category.clone(), rule.intent_type.clone())
                    .with_confidence(1.0)
                    .with_method(DetectionMethod::ExactCommand)
                    .with_capabilities(rule.capabilities.clone())
                    .with_cleaned_input(cleaned_input);

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
                    command = %rule.pattern,
                    intent = %rule.intent_type,
                    "Command layer matched"
                );

                return Some(
                    MatchResult::new(intent, 1.0, RoutingLayer::L1Rule)
                        .with_rule_index(rule.index)
                );
            }
        }

        None
    }
}

/// Compiled command rule for fast matching
#[derive(Debug, Clone)]
struct CompiledCommandRule {
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
}

impl CompiledCommandRule {
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
            .unwrap_or_else(|| Self::extract_command_name(&config.regex));

        Ok(Self {
            regex,
            pattern: config.regex.clone(),
            index,
            category,
            intent_type,
            provider_name: config.provider.clone(),
            system_prompt: config.system_prompt.clone(),
            capabilities: config.get_capabilities(),
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
                _ => IntentCategory::Command(intent_type.clone()),
            }
        } else {
            let cmd_name = Self::extract_command_name(&config.regex);
            IntentCategory::Command(cmd_name)
        }
    }

    /// Extract command name from pattern (e.g., "^/translate" -> "translate")
    fn extract_command_name(pattern: &str) -> String {
        pattern
            .trim_start_matches("^/")
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or("command")
            .to_string()
    }

    /// Strip matched prefix from input
    fn strip_prefix(&self, input: &str) -> String {
        if let Some(mat) = self.regex.find(input) {
            let stripped = &input[mat.end()..];
            stripped.trim_start().to_string()
        } else {
            input.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_command_layer_match() {
        let mut layer = CommandLayer::new();

        let config = RoutingRuleConfig {
            regex: r"^/translate\s*".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("You are a translator.".to_string()),
            intent_type: Some("translate".to_string()),
            capabilities: Some(vec!["memory".to_string()]),
            ..Default::default()
        };

        layer.add_rule(&config, 0).unwrap();

        let ctx = MatchingContext::simple("/translate Hello world");
        let result = layer.try_match(&ctx).await;

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.confidence, 1.0);
        assert_eq!(result.intent.intent_type, "translate");
        assert_eq!(
            result.intent.cleaned_input,
            Some("Hello world".to_string())
        );
    }

    #[tokio::test]
    async fn test_command_layer_no_match() {
        let mut layer = CommandLayer::new();

        let config = RoutingRuleConfig {
            regex: r"^/translate\s*".to_string(),
            ..Default::default()
        };

        layer.add_rule(&config, 0).unwrap();

        let ctx = MatchingContext::simple("Hello world");
        let result = layer.try_match(&ctx).await;

        assert!(result.is_none());
    }

    #[test]
    fn test_extract_command_name() {
        assert_eq!(
            CompiledCommandRule::extract_command_name("^/translate"),
            "translate"
        );
        assert_eq!(
            CompiledCommandRule::extract_command_name("^/search\\s+"),
            "search"
        );
        assert_eq!(
            CompiledCommandRule::extract_command_name("^/zh"),
            "zh"
        );
    }

    #[test]
    fn test_command_layer_properties() {
        let layer = CommandLayer::new();

        assert_eq!(layer.layer_id(), "command");
        assert_eq!(layer.priority(), 0);
        assert!(layer.is_terminal());
        assert!(layer.is_enabled());
    }
}
