/// RoutingDecision - Extended routing result with capabilities and format info
///
/// This module provides an enhanced routing decision structure that includes
/// not just the selected provider, but also capabilities, intent, and context format.
use crate::config::RoutingRuleConfig;
use crate::payload::{Capability, ContextFormat, Intent};
use crate::providers::AiProvider;

/// Routing decision result (extended version)
///
/// Contains provider selection + extended information (capabilities, intent, format)
pub struct RoutingDecision<'a> {
    /// Target provider
    pub provider: &'a dyn AiProvider,

    /// Provider name (for logging)
    pub provider_name: String,

    /// System prompt (from rule or provider default)
    pub system_prompt: String,

    /// Capabilities to execute
    pub capabilities: Vec<Capability>,

    /// User intent
    pub intent: Intent,

    /// Context injection format
    pub context_format: ContextFormat,

    /// Fallback provider (if main provider fails)
    pub fallback: Option<&'a dyn AiProvider>,
}

impl<'a> RoutingDecision<'a> {
    /// Create a routing decision from a rule
    pub fn from_rule(
        provider: &'a dyn AiProvider,
        provider_name: String,
        rule: &RoutingRuleConfig,
        fallback: Option<&'a dyn AiProvider>,
    ) -> Self {
        let capabilities = rule.get_capabilities();
        let intent = Intent::from_rule(rule);
        let context_format = rule.get_context_format();

        // System prompt priority: rule > provider default
        let system_prompt = rule
            .system_prompt
            .clone()
            .unwrap_or_else(|| "You are a helpful AI assistant.".to_string());

        Self {
            provider,
            provider_name,
            system_prompt,
            capabilities,
            intent,
            context_format,
            fallback,
        }
    }

    /// Create a basic decision without a rule (for default routing)
    pub fn basic(
        provider: &'a dyn AiProvider,
        provider_name: String,
        system_prompt: String,
    ) -> Self {
        Self {
            provider,
            provider_name,
            system_prompt,
            capabilities: vec![],
            intent: Intent::GeneralChat,
            context_format: ContextFormat::Markdown,
            fallback: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::MockProvider;

    #[test]
    fn test_decision_from_rule() {
        let provider = MockProvider::new("Test Provider");
        let rule = RoutingRuleConfig::test_config("^/test", "openai");

        let decision = RoutingDecision::from_rule(
            &provider as &dyn AiProvider,
            "openai".to_string(),
            &rule,
            None,
        );

        assert_eq!(decision.provider_name, "openai");
        assert_eq!(decision.intent, Intent::GeneralChat);
        assert_eq!(decision.context_format, ContextFormat::Markdown);
    }

    #[test]
    fn test_decision_with_capabilities() {
        let provider = MockProvider::new("Test Provider");
        let mut rule = RoutingRuleConfig::test_config("^/research", "openai");
        rule.capabilities = Some(vec!["memory".to_string(), "search".to_string()]);

        let decision = RoutingDecision::from_rule(
            &provider as &dyn AiProvider,
            "openai".to_string(),
            &rule,
            None,
        );

        assert_eq!(decision.capabilities.len(), 2);
        assert!(decision.capabilities.contains(&Capability::Memory));
        assert!(decision.capabilities.contains(&Capability::Search));
    }

    #[test]
    fn test_decision_with_custom_intent() {
        let provider = MockProvider::new("Test Provider");
        let mut rule = RoutingRuleConfig::test_config("^/translate", "openai");
        rule.intent_type = Some("translation".to_string());

        let decision = RoutingDecision::from_rule(
            &provider as &dyn AiProvider,
            "openai".to_string(),
            &rule,
            None,
        );

        assert_eq!(decision.intent, Intent::Custom("translation".to_string()));
    }

    #[test]
    fn test_basic_decision() {
        let provider = MockProvider::new("Test Provider");

        let decision = RoutingDecision::basic(
            &provider as &dyn AiProvider,
            "openai".to_string(),
            "Custom system prompt".to_string(),
        );

        assert_eq!(decision.provider_name, "openai");
        assert_eq!(decision.system_prompt, "Custom system prompt");
        assert_eq!(decision.capabilities.len(), 0);
        assert_eq!(decision.intent, Intent::GeneralChat);
    }
}
