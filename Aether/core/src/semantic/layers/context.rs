//! Context inference layer (Layer 3).
//!
//! This layer handles context-aware inference including:
//! - Pending parameter completion
//! - App-specific context rules
//! - Time-based context rules
//! - Previous intent continuation

use crate::dispatcher::RoutingLayer;
use crate::payload::Capability;
use crate::semantic::context::{MatchingContext, PendingParam};
use crate::semantic::intent::{DetectionMethod, IntentCategory, ParamValue, SemanticIntent};
use crate::semantic::layer::{LayerEnabledFlag, MatchingLayer};
use crate::semantic::matcher::MatchResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

/// Context inference layer
///
/// Matches input using context-aware inference including:
/// - Pending parameter completion from previous turns
/// - App-specific rules (e.g., code help in VS Code)
/// - Time-based rules (e.g., different behavior on weekends)
///
/// This layer has priority 3 and is non-terminal.
pub struct ContextLayer {
    /// Context rules
    rules: Vec<ContextRule>,
    /// Enabled flag
    enabled: LayerEnabledFlag,
    /// Default confidence for context matches
    default_confidence: f64,
}

impl ContextLayer {
    /// Create a new empty context layer
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            enabled: LayerEnabledFlag::new(true),
            default_confidence: 0.7,
        }
    }

    /// Create with custom default confidence
    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.default_confidence = confidence;
        self
    }

    /// Add a context rule
    pub fn add_rule(&mut self, rule: ContextRule) {
        self.rules.push(rule);
    }

    /// Get rules
    pub fn rules(&self) -> &[ContextRule] {
        &self.rules
    }
}

impl Default for ContextLayer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MatchingLayer for ContextLayer {
    fn layer_id(&self) -> &str {
        "context"
    }

    fn priority(&self) -> u32 {
        3 // Fourth priority
    }

    fn is_enabled(&self) -> bool {
        self.enabled.is_enabled()
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.set_enabled(enabled);
    }

    fn is_terminal(&self) -> bool {
        false // Context matches can be merged
    }

    fn confidence_threshold(&self) -> f64 {
        0.5 // Lower threshold for context inference
    }

    async fn try_match(&self, ctx: &MatchingContext) -> Option<MatchResult> {
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
}

impl ContextLayer {
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
            .with_confidence(0.85)
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
                param = %param.param_name,
                intent = %param.required_for,
                "Context layer: completing pending param"
            );

            // L2 Semantic match (context inference)
            return Some(MatchResult::new(intent, 0.85, RoutingLayer::L2Semantic));
        }

        None
    }

    /// Try app-specific context rules
    fn try_app_context_rules(&self, ctx: &MatchingContext) -> Option<MatchResult> {
        for rule in &self.rules {
            if let ContextCondition::App { bundle_ids } = &rule.condition {
                let matches = bundle_ids.iter().any(|id| ctx.app.matches_bundle(id));

                if matches {
                    return Some(self.apply_context_rule(rule, "app_context"));
                }
            }
        }

        None
    }

    /// Try time-based context rules
    fn try_time_context_rules(&self, ctx: &MatchingContext) -> Option<MatchResult> {
        for rule in &self.rules {
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
                    return Some(self.apply_context_rule(rule, "time_context"));
                }
            }
        }

        None
    }

    /// Apply a matched context rule
    fn apply_context_rule(&self, rule: &ContextRule, source: &str) -> MatchResult {
        let mut intent = SemanticIntent::general()
            .with_confidence(self.default_confidence)
            .with_method(DetectionMethod::ContextInference {
                source: source.to_string(),
                details: Some(rule.id.clone()),
            });

        // Apply actions
        for action in &rule.actions {
            match action {
                ContextAction::AddCapability { value } => {
                    if let Some(c) = parse_capability(value) {
                        intent.capabilities.push(c);
                    }
                }
                ContextAction::SetProvider { value } => {
                    intent = intent.with_provider(value.clone());
                }
                ContextAction::SetSystemPrompt { value } => {
                    intent = intent.with_system_prompt(value.clone());
                }
                ContextAction::SetIntent { value } => {
                    intent.intent_type = value.clone();
                }
            }
        }

        debug!(
            rule_id = %rule.id,
            source = %source,
            "Context layer: applied rule"
        );

        // L2 Semantic match (context rule)
        MatchResult::new(intent, self.default_confidence, RoutingLayer::L2Semantic)
    }
}

/// Context rule for inference
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
    App { bundle_ids: Vec<String> },

    /// Match by time
    Time {
        is_weekend: Option<bool>,
        hour_range: Option<(u8, u8)>,
    },

    /// Match by pending parameter
    PendingParam { param_name: String, intent: String },

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
    AddCapability { value: String },
    /// Set provider
    SetProvider { value: String },
    /// Set system prompt
    SetSystemPrompt { value: String },
    /// Set intent type
    SetIntent { value: String },
}

/// Parse capability string to Capability enum
fn parse_capability(s: &str) -> Option<Capability> {
    match s.to_lowercase().as_str() {
        "memory" => Some(Capability::Memory),
        "mcp" => Some(Capability::Mcp),
        "skills" => Some(Capability::Skills),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::context::{AppContext, ConversationContext};

    #[tokio::test]
    async fn test_context_layer_pending_param() {
        let layer = ContextLayer::new();

        // Create context with pending param
        let mut conversation = ConversationContext::new();
        conversation.add_pending_param(PendingParam::new(
            "location",
            "weather",
            "Please provide a location:",
        ));

        let ctx = MatchingContext::builder()
            .raw_input("Beijing")
            .conversation(conversation)
            .build();

        let result = layer.try_match(&ctx).await;

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.confidence, 0.85);
        assert_eq!(result.intent.intent_type, "weather");
        assert!(result.intent.params.contains_key("location"));
    }

    #[tokio::test]
    async fn test_context_layer_app_rule() {
        let mut layer = ContextLayer::new();

        layer.add_rule(ContextRule {
            id: "vscode_code_help".to_string(),
            condition: ContextCondition::App {
                bundle_ids: vec!["com.microsoft.VSCode".to_string()],
            },
            actions: vec![
                ContextAction::AddCapability { value: "memory".to_string() },
                ContextAction::SetIntent { value: "code_help".to_string() },
            ],
        });

        let ctx = MatchingContext::builder()
            .raw_input("How do I fix this?")
            .app(AppContext::new("com.microsoft.VSCode", "Visual Studio Code"))
            .build();

        let result = layer.try_match(&ctx).await;

        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.intent.capabilities.contains(&Capability::Memory));
        assert_eq!(result.intent.intent_type, "code_help");
    }

    #[tokio::test]
    async fn test_context_layer_no_match() {
        let layer = ContextLayer::new();

        let ctx = MatchingContext::simple("Hello world");
        let result = layer.try_match(&ctx).await;

        assert!(result.is_none());
    }

    #[test]
    fn test_context_layer_properties() {
        let layer = ContextLayer::new();

        assert_eq!(layer.layer_id(), "context");
        assert_eq!(layer.priority(), 3);
        assert!(!layer.is_terminal()); // Non-terminal
        assert!(layer.is_enabled());
    }

    #[test]
    fn test_context_rule_serialization() {
        let rule = ContextRule {
            id: "test".to_string(),
            condition: ContextCondition::App {
                bundle_ids: vec!["com.test".to_string()],
            },
            actions: vec![ContextAction::SetIntent { value: "test_intent".to_string() }],
        };

        let json = serde_json::to_string(&rule).unwrap();
        let parsed: ContextRule = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "test");
    }
}
