//! SemanticMatcher - Multi-layer semantic matching orchestrator
//!
//! Implements a 4-layer matching system:
//! - Layer 1: Fast path (command/regex matching)
//! - Layer 2: Keyword index (weighted keyword scoring)
//! - Layer 3: Context inference (multi-turn, app, time aware)
//! - Layer 4: AI fallback (AI-first detection)

use super::context::{MatchingContext, PendingParam};
use super::intent::{
    BuiltinCapability, DetectionMethod, IntentCategory, ParamValue, SemanticIntent,
};
use super::keyword::{KeywordIndex, KeywordMatch, KeywordRule};
use crate::config::RoutingRuleConfig;
use crate::error::Result;
use crate::payload::Capability;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

/// Multi-layer semantic matcher
pub struct SemanticMatcher {
    /// Layer 1: Command rules (^/xxx patterns)
    command_rules: Vec<CompiledRule>,

    /// Layer 1: Regex rules (other patterns)
    regex_rules: Vec<CompiledRule>,

    /// Layer 2: Keyword index
    keyword_index: KeywordIndex,

    /// Layer 3: Context rules
    context_rules: Vec<ContextRule>,

    /// Configuration
    config: MatcherConfig,
}

impl SemanticMatcher {
    /// Create a new SemanticMatcher from routing rules
    pub fn new(config: MatcherConfig) -> Self {
        Self {
            command_rules: Vec::new(),
            regex_rules: Vec::new(),
            keyword_index: KeywordIndex::new(),
            context_rules: Vec::new(),
            config,
        }
    }

    /// Create from routing rule configs
    pub fn from_rules(rules: &[RoutingRuleConfig], config: MatcherConfig) -> Result<Self> {
        let mut matcher = Self::new(config);

        for (index, rule) in rules.iter().enumerate() {
            matcher.add_routing_rule(rule, index)?;
        }

        Ok(matcher)
    }

    /// Add a routing rule
    fn add_routing_rule(&mut self, rule: &RoutingRuleConfig, index: usize) -> Result<()> {
        let compiled = CompiledRule::from_config(rule, index)?;

        if rule.is_command_rule() {
            self.command_rules.push(compiled);
        } else {
            self.regex_rules.push(compiled);
        }

        Ok(())
    }

    /// Add a keyword rule
    pub fn add_keyword_rule(&mut self, rule: KeywordRule) {
        self.keyword_index.add_rule(rule);
    }

    /// Add a context rule
    pub fn add_context_rule(&mut self, rule: ContextRule) {
        self.context_rules.push(rule);
    }

    /// Create a SemanticMatcher with a pre-built keyword index
    pub fn with_keyword_index(config: &MatcherConfig, keyword_index: KeywordIndex) -> Self {
        Self {
            command_rules: Vec::new(),
            regex_rules: Vec::new(),
            keyword_index,
            context_rules: Vec::new(),
            config: config.clone(),
        }
    }

    /// Match keywords only (synchronous, for quick lookups)
    ///
    /// Returns all keyword matches sorted by score, without going through
    /// the full async semantic detection pipeline.
    pub fn match_keywords_only(&self, input: &str) -> Vec<KeywordMatch> {
        self.keyword_index.match_keywords(input)
    }

    /// Match input against all layers
    ///
    /// Returns the best match result with confidence and detection method.
    pub async fn match_input(&self, ctx: &MatchingContext) -> MatchResult {
        let input = &ctx.raw_input;

        // Layer 1: Fast path - exact command match
        if let Some(result) = self.try_command_match(input, ctx) {
            debug!(
                "Layer 1 match: command '{}' -> intent '{}'",
                result.intent.intent_type,
                result.intent.category
            );
            return result;
        }

        // Layer 1: Regex pattern match
        if let Some(result) = self.try_regex_match(input, ctx) {
            if result.confidence >= self.config.regex_threshold {
                debug!(
                    "Layer 1 match: regex -> intent '{}' (confidence: {:.2})",
                    result.intent.intent_type, result.confidence
                );
                return result;
            }
        }

        // Layer 2: Keyword matching
        let keyword_result = self.try_keyword_match(input, ctx);

        // Layer 3: Context-aware inference
        let context_result = self.try_context_inference(ctx);

        // Combine Layer 2 and Layer 3 results
        let combined = self.merge_results(keyword_result, context_result);

        // Layer 4: AI fallback if confidence too low
        if combined.confidence < self.config.ai_threshold {
            debug!(
                "Combined confidence ({:.2}) below AI threshold ({:.2}), using AI fallback",
                combined.confidence, self.config.ai_threshold
            );

            // Note: AI detection is handled externally in AetherCore
            // We return the combined result with a flag indicating AI should be tried
            return combined.with_needs_ai_fallback(true);
        }

        combined
    }

    /// Layer 1: Try command match (^/xxx patterns)
    fn try_command_match(&self, input: &str, _ctx: &MatchingContext) -> Option<MatchResult> {
        for rule in &self.command_rules {
            if rule.regex.is_match(input) {
                let cleaned_input = rule.strip_prefix(input);

                let intent = SemanticIntent::new(
                    rule.category.clone(),
                    rule.intent_type.clone(),
                )
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

                return Some(MatchResult {
                    intent,
                    confidence: 1.0,
                    rule_index: Some(rule.index),
                    needs_ai_fallback: false,
                });
            }
        }

        None
    }

    /// Layer 1: Try regex match (non-command patterns)
    fn try_regex_match(&self, input: &str, _ctx: &MatchingContext) -> Option<MatchResult> {
        for rule in &self.regex_rules {
            if rule.regex.is_match(input) {
                let intent = SemanticIntent::new(
                    rule.category.clone(),
                    rule.intent_type.clone(),
                )
                .with_confidence(self.config.regex_threshold)
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

                return Some(MatchResult {
                    intent,
                    confidence: self.config.regex_threshold,
                    rule_index: Some(rule.index),
                    needs_ai_fallback: false,
                });
            }
        }

        None
    }

    /// Layer 2: Try keyword match
    fn try_keyword_match(&self, input: &str, _ctx: &MatchingContext) -> Option<MatchResult> {
        let matches = self
            .keyword_index
            .match_with_threshold(input, self.config.keyword_threshold);

        if let Some(best) = matches.into_iter().next() {
            let confidence = (best.score as f64).min(1.0);

            let intent = SemanticIntent::new(
                IntentCategory::Semantic(best.intent_type.clone()),
                best.intent_type.clone(),
            )
            .with_confidence(confidence)
            .with_method(DetectionMethod::keyword(
                best.score as f64,
                best.matched_keywords.clone(),
            ))
            .with_capabilities(
                best.capabilities
                    .iter()
                    .filter_map(|c| parse_capability(c))
                    .collect(),
            );

            let intent = if let Some(prompt) = best.system_prompt {
                intent.with_system_prompt(prompt)
            } else {
                intent
            };

            return Some(MatchResult {
                intent,
                confidence,
                rule_index: None,
                needs_ai_fallback: false,
            });
        }

        None
    }

    /// Layer 3: Try context-aware inference
    fn try_context_inference(&self, ctx: &MatchingContext) -> Option<MatchResult> {
        // Priority 1: Check for pending parameter completion
        if let Some(result) = self.infer_param_completion(ctx) {
            return Some(result);
        }

        // Priority 2: Check app-specific rules
        if let Some(result) = self.try_app_context_rules(ctx) {
            return Some(result);
        }

        // Priority 3: Check time-based rules
        if let Some(result) = self.try_time_context_rules(ctx) {
            return Some(result);
        }

        None
    }

    /// Infer parameter completion from previous turn
    ///
    /// E.g., Previous: "weather?" -> AI asks for location
    ///       Current: "Beijing" -> Infer this completes the location param
    fn infer_param_completion(&self, ctx: &MatchingContext) -> Option<MatchResult> {
        if ctx.conversation.pending_params.is_empty() {
            return None;
        }

        // Find the most recent non-expired pending param
        let pending: Option<&PendingParam> = ctx
            .conversation
            .pending_params
            .values()
            .filter(|p| !p.is_expired())
            .next();

        if let Some(param) = pending {
            let input = ctx.effective_input();

            // The input is likely the value for this pending parameter
            let mut params = HashMap::new();
            params.insert(param.param_name.clone(), ParamValue::from(input.to_string()));

            let intent = SemanticIntent::new(
                IntentCategory::Semantic(param.required_for.clone()),
                param.required_for.clone(),
            )
            .with_confidence(0.85) // High but not certain
            .with_method(DetectionMethod::ContextInference {
                source: "pending_param".to_string(),
                details: Some(format!(
                    "Completing '{}' for '{}'",
                    param.param_name, param.required_for
                )),
            })
            .with_params(params)
            .with_reasoning(format!(
                "Previous turn asked for '{}', interpreting input as parameter value",
                param.param_name
            ));

            debug!(
                "Context inference: completing param '{}' for intent '{}'",
                param.param_name, param.required_for
            );

            return Some(MatchResult {
                intent,
                confidence: 0.85,
                rule_index: None,
                needs_ai_fallback: false,
            });
        }

        None
    }

    /// Try app-specific context rules
    fn try_app_context_rules(&self, ctx: &MatchingContext) -> Option<MatchResult> {
        for rule in &self.context_rules {
            if let ContextCondition::App { bundle_ids } = &rule.condition {
                let matches = bundle_ids
                    .iter()
                    .any(|id| ctx.app.matches_bundle(id));

                if matches {
                    return Some(self.apply_context_rule(rule, ctx, "app_context"));
                }
            }
        }

        None
    }

    /// Try time-based context rules
    fn try_time_context_rules(&self, ctx: &MatchingContext) -> Option<MatchResult> {
        for rule in &self.context_rules {
            if let ContextCondition::Time {
                is_weekend,
                hour_range,
            } = &rule.condition
            {
                let mut matches = true;

                if let Some(weekend) = is_weekend {
                    matches = matches && (ctx.time.is_weekend == *weekend);
                }

                if let Some((start, end)) = hour_range {
                    matches = matches && ctx.time.is_within_hours(*start, *end);
                }

                if matches {
                    return Some(self.apply_context_rule(rule, ctx, "time_context"));
                }
            }
        }

        None
    }

    /// Apply a matched context rule
    fn apply_context_rule(
        &self,
        rule: &ContextRule,
        _ctx: &MatchingContext,
        source: &str,
    ) -> MatchResult {
        let mut intent = SemanticIntent::general()
            .with_confidence(0.7)
            .with_method(DetectionMethod::ContextInference {
                source: source.to_string(),
                details: Some(rule.id.clone()),
            });

        // Apply actions
        for action in &rule.actions {
            match action {
                ContextAction::AddCapability(cap) => {
                    if let Some(c) = parse_capability(cap) {
                        intent.capabilities.push(c);
                    }
                }
                ContextAction::SetProvider(provider) => {
                    intent = intent.with_provider(provider.clone());
                }
                ContextAction::SetSystemPrompt(prompt) => {
                    intent = intent.with_system_prompt(prompt.clone());
                }
                ContextAction::SetIntent(intent_type) => {
                    intent.intent_type = intent_type.clone();
                }
            }
        }

        MatchResult {
            intent,
            confidence: 0.7,
            rule_index: None,
            needs_ai_fallback: false,
        }
    }

    /// Merge results from different layers
    fn merge_results(
        &self,
        keyword_result: Option<MatchResult>,
        context_result: Option<MatchResult>,
    ) -> MatchResult {
        match (keyword_result, context_result) {
            (Some(kw), Some(ctx)) => {
                // Prefer higher confidence, or context if tied
                if kw.confidence > ctx.confidence {
                    kw
                } else {
                    ctx
                }
            }
            (Some(kw), None) => kw,
            (None, Some(ctx)) => ctx,
            (None, None) => {
                // No match - return general intent
                MatchResult {
                    intent: SemanticIntent::general()
                        .with_confidence(0.0)
                        .with_reasoning("No matching rules found".to_string()),
                    confidence: 0.0,
                    rule_index: None,
                    needs_ai_fallback: true, // Signal that AI should be tried
                }
            }
        }
    }

    /// Get configuration
    pub fn config(&self) -> &MatcherConfig {
        &self.config
    }

    /// Create a LayerChain from this matcher's configuration
    ///
    /// This allows gradual migration to the layer pattern while maintaining
    /// backward compatibility with the existing API.
    pub fn to_layer_chain(&self) -> super::layer::LayerChain {
        use super::layers::{CommandLayer, ContextLayer, KeywordLayer, RegexLayer};
        use std::sync::Arc;

        let mut chain = super::layer::LayerChain::new();

        // Create and register Command layer
        let command_layer = CommandLayer::new();
        for rule in &self.command_rules {
            // We need to re-add rules from the compiled rules
            // For now, just create an empty layer that will be populated externally
            let _ = rule; // Acknowledge we have the rules
        }
        // Note: In a full implementation, we'd reconstruct rules from compiled_rules
        // For now, we create empty layers that can be populated via from_rules
        chain.register(Arc::new(command_layer));

        // Create and register Regex layer
        let regex_layer = RegexLayer::new().with_confidence(self.config.regex_threshold);
        chain.register(Arc::new(regex_layer));

        // Create and register Keyword layer
        let keyword_layer = KeywordLayer::with_index(self.keyword_index.clone())
            .with_threshold(self.config.keyword_threshold);
        chain.register(Arc::new(keyword_layer));

        // Create and register Context layer
        let mut context_layer = ContextLayer::new();
        for rule in &self.context_rules {
            context_layer.add_rule(super::layers::context::ContextRule {
                id: rule.id.clone(),
                condition: match &rule.condition {
                    ContextCondition::App { bundle_ids } => {
                        super::layers::context::ContextCondition::App {
                            bundle_ids: bundle_ids.clone(),
                        }
                    }
                    ContextCondition::Time { is_weekend, hour_range } => {
                        super::layers::context::ContextCondition::Time {
                            is_weekend: *is_weekend,
                            hour_range: *hour_range,
                        }
                    }
                    ContextCondition::PendingParam { param_name, intent } => {
                        super::layers::context::ContextCondition::PendingParam {
                            param_name: param_name.clone(),
                            intent: intent.clone(),
                        }
                    }
                    ContextCondition::PreviousIntent { intents, within_turns } => {
                        super::layers::context::ContextCondition::PreviousIntent {
                            intents: intents.clone(),
                            within_turns: *within_turns,
                        }
                    }
                },
                actions: rule.actions.iter().map(|a| match a {
                    ContextAction::AddCapability(s) => {
                        super::layers::context::ContextAction::AddCapability { value: s.clone() }
                    }
                    ContextAction::SetProvider(s) => {
                        super::layers::context::ContextAction::SetProvider { value: s.clone() }
                    }
                    ContextAction::SetSystemPrompt(s) => {
                        super::layers::context::ContextAction::SetSystemPrompt { value: s.clone() }
                    }
                    ContextAction::SetIntent(s) => {
                        super::layers::context::ContextAction::SetIntent { value: s.clone() }
                    }
                }).collect(),
            });
        }
        chain.register(Arc::new(context_layer));

        chain
    }

    /// Match input using the layer chain pattern
    ///
    /// This is an alternative to `match_input` that uses the LayerChain
    /// with pluggable layers. Useful for testing or when layer pattern benefits
    /// are needed.
    pub async fn match_input_with_layers(&self, ctx: &MatchingContext) -> MatchResult {
        let chain = self.to_layer_chain();

        if let Some(result) = chain.execute(ctx).await {
            result
        } else {
            // No match - return general intent
            MatchResult {
                intent: SemanticIntent::general()
                    .with_confidence(0.0)
                    .with_reasoning("No matching rules found".to_string()),
                confidence: 0.0,
                rule_index: None,
                needs_ai_fallback: true,
            }
        }
    }

    /// Get command rule count
    pub fn command_rule_count(&self) -> usize {
        self.command_rules.len()
    }

    /// Get regex rule count
    pub fn regex_rule_count(&self) -> usize {
        self.regex_rules.len()
    }
}

/// Compiled routing rule for fast matching
#[derive(Debug, Clone)]
struct CompiledRule {
    /// Compiled regex
    regex: Regex,

    /// Original pattern (kept for debugging)
    #[allow(dead_code)]
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

    /// Whether to strip matched prefix
    strip_prefix: bool,
}

impl CompiledRule {
    /// Create from RoutingRuleConfig
    fn from_config(config: &RoutingRuleConfig, index: usize) -> Result<Self> {
        let regex = Regex::new(&config.regex).map_err(|e| {
            crate::error::AetherError::invalid_config(format!(
                "Invalid regex pattern '{}': {}",
                config.regex, e
            ))
        })?;

        // Determine intent category
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
            strip_prefix: config.should_strip_prefix(),
        })
    }

    /// Determine intent category from config
    fn determine_category(config: &RoutingRuleConfig) -> IntentCategory {
        if let Some(ref intent_type) = config.intent_type {
            match intent_type.as_str() {
                "search" | "web_search" | "builtin_search" => {
                    IntentCategory::Builtin(BuiltinCapability::Search)
                }
                "video" | "video_analysis" => {
                    IntentCategory::Builtin(BuiltinCapability::Video)
                }
                "mcp" | "tool_call" | "builtin_mcp" => {
                    IntentCategory::Builtin(BuiltinCapability::Mcp)
                }
                s if s.starts_with("skills:") => {
                    let id = s.strip_prefix("skills:").unwrap_or("");
                    IntentCategory::Skills(id.to_string())
                }
                _ => {
                    if config.is_command_rule() {
                        IntentCategory::Command(intent_type.clone())
                    } else {
                        IntentCategory::Semantic(intent_type.clone())
                    }
                }
            }
        } else if config.is_command_rule() {
            // Extract command name from pattern
            let cmd_name = config
                .regex
                .trim_start_matches("^/")
                .split(|c: char| !c.is_alphanumeric())
                .next()
                .unwrap_or("command")
                .to_string();
            IntentCategory::Command(cmd_name)
        } else {
            IntentCategory::General
        }
    }

    /// Strip matched prefix from input
    fn strip_prefix(&self, input: &str) -> String {
        if !self.strip_prefix {
            return input.to_string();
        }

        if let Some(mat) = self.regex.find(input) {
            let stripped = &input[mat.end()..];
            stripped.trim_start().to_string()
        } else {
            input.to_string()
        }
    }
}

/// Match result from SemanticMatcher
#[derive(Debug, Clone)]
pub struct MatchResult {
    /// Matched intent
    pub intent: SemanticIntent,

    /// Overall confidence (0.0 - 1.0)
    pub confidence: f64,

    /// Index of matched rule (if from Layer 1)
    pub rule_index: Option<usize>,

    /// Whether AI fallback should be attempted
    pub needs_ai_fallback: bool,
}

impl MatchResult {
    /// Set needs_ai_fallback flag
    pub fn with_needs_ai_fallback(mut self, needs: bool) -> Self {
        self.needs_ai_fallback = needs;
        self
    }

    /// Check if confident enough
    pub fn is_confident(&self, threshold: f64) -> bool {
        self.confidence >= threshold
    }
}

/// Matcher configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatcherConfig {
    /// Whether semantic matching is enabled
    pub enabled: bool,

    /// Regex match confidence threshold (default: 0.9)
    pub regex_threshold: f64,

    /// Keyword match confidence threshold (default: 0.7)
    pub keyword_threshold: f32,

    /// AI fallback threshold - below this, try AI detection (default: 0.6)
    pub ai_threshold: f64,

    /// Whether to enable context inference
    pub enable_context_inference: bool,
}

impl Default for MatcherConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            regex_threshold: 0.9,
            keyword_threshold: 0.7,
            ai_threshold: 0.6,
            enable_context_inference: true,
        }
    }
}

/// Context rule for Layer 3 inference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRule {
    /// Rule ID
    pub id: String,

    /// Condition to match
    pub condition: ContextCondition,

    /// Actions to apply when matched
    pub actions: Vec<ContextAction>,
}

/// Condition for context rules
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContextCondition {
    /// Match by application bundle IDs
    App {
        bundle_ids: Vec<String>,
    },

    /// Match by time
    Time {
        is_weekend: Option<bool>,
        hour_range: Option<(u8, u8)>,
    },

    /// Match by pending parameter
    PendingParam {
        param_name: String,
        intent: String,
    },

    /// Match by previous intent
    PreviousIntent {
        intents: Vec<String>,
        within_turns: usize,
    },
}

/// Action for context rules
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContextAction {
    /// Add a capability
    AddCapability(String),

    /// Set provider
    SetProvider(String),

    /// Set system prompt
    SetSystemPrompt(String),

    /// Set intent type
    SetIntent(String),
}

/// Parse capability string to Capability enum
fn parse_capability(s: &str) -> Option<Capability> {
    match s.to_lowercase().as_str() {
        "memory" => Some(Capability::Memory),
        "search" => Some(Capability::Search),
        "mcp" => Some(Capability::Mcp),
        "video" => Some(Capability::Video),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::context::AppContext;

    fn test_config() -> MatcherConfig {
        MatcherConfig::default()
    }

    #[tokio::test]
    async fn test_command_match() {
        let mut matcher = SemanticMatcher::new(test_config());

        // Add a command rule manually
        let rule = CompiledRule {
            regex: Regex::new(r"^/search\s+").unwrap(),
            pattern: "^/search\\s+".to_string(),
            index: 0,
            category: IntentCategory::search(),
            intent_type: "search".to_string(),
            provider_name: Some("openai".to_string()),
            system_prompt: Some("You are a search assistant.".to_string()),
            capabilities: vec![Capability::Search],
            strip_prefix: true,
        };
        matcher.command_rules.push(rule);

        let ctx = MatchingContext::simple("/search weather in Beijing");
        let result = matcher.match_input(&ctx).await;

        assert_eq!(result.confidence, 1.0);
        assert!(result.intent.is_builtin());
        assert_eq!(
            result.intent.cleaned_input,
            Some("weather in Beijing".to_string())
        );
    }

    #[tokio::test]
    async fn test_keyword_match() {
        let mut matcher = SemanticMatcher::new(test_config());

        matcher.add_keyword_rule(
            KeywordRule::new(
                "weather",
                "weather_query",
                vec![
                    "weather".to_string(),
                    "forecast".to_string(),
                    "天气".to_string(),
                ],
            )
            .with_capabilities(vec!["search".to_string()]),
        );

        let ctx = MatchingContext::simple("What's the weather today?");
        let result = matcher.match_input(&ctx).await;

        assert!(result.confidence >= 0.7);
        assert_eq!(result.intent.intent_type, "weather_query");
    }

    #[tokio::test]
    async fn test_context_inference_pending_param() {
        let matcher = SemanticMatcher::new(test_config());

        // Create context with pending param
        let mut conversation = super::super::context::ConversationContext::new();
        conversation.add_pending_param(PendingParam::new(
            "location",
            "weather",
            "Please provide a location:",
        ));

        let ctx = MatchingContext::builder()
            .raw_input("Beijing")
            .conversation(conversation)
            .build();

        let result = matcher.match_input(&ctx).await;

        assert!(result.confidence >= 0.8);
        assert_eq!(result.intent.intent_type, "weather");
        assert!(result
            .intent
            .params
            .contains_key("location"));
    }

    #[tokio::test]
    async fn test_app_context_rule() {
        let mut matcher = SemanticMatcher::new(test_config());

        matcher.add_context_rule(ContextRule {
            id: "code_in_vscode".to_string(),
            condition: ContextCondition::App {
                bundle_ids: vec!["com.microsoft.VSCode".to_string()],
            },
            actions: vec![
                ContextAction::AddCapability("memory".to_string()),
                ContextAction::SetIntent("code_help".to_string()),
            ],
        });

        let ctx = MatchingContext::builder()
            .raw_input("How do I fix this error?")
            .app(AppContext::new("com.microsoft.VSCode", "Visual Studio Code"))
            .build();

        let result = matcher.match_input(&ctx).await;

        // Should match app context rule
        assert!(result.confidence >= 0.7);
        assert!(result.intent.capabilities.contains(&Capability::Memory));
    }

    #[tokio::test]
    async fn test_no_match_needs_ai() {
        let matcher = SemanticMatcher::new(test_config());

        let ctx = MatchingContext::simple("Tell me a joke");
        let result = matcher.match_input(&ctx).await;

        // No rules match, should signal AI fallback
        assert!(result.needs_ai_fallback);
        assert!(result.confidence < 0.6);
    }

    #[test]
    fn test_strip_prefix() {
        let rule = CompiledRule {
            regex: Regex::new(r"^/translate\s*").unwrap(),
            pattern: "^/translate\\s*".to_string(),
            index: 0,
            category: IntentCategory::command("translate"),
            intent_type: "translate".to_string(),
            provider_name: None,
            system_prompt: None,
            capabilities: vec![],
            strip_prefix: true,
        };

        let stripped = rule.strip_prefix("/translate Hello world");
        assert_eq!(stripped, "Hello world");
    }
}
