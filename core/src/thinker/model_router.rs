//! Model routing for Agent Loop
//!
//! This module selects the appropriate AI model based on
//! task characteristics and context.

use crate::agent_loop::{ModelRoutingConfig, Observation};

/// Model identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelId(pub String);

impl ModelId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ModelId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ModelId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Routing condition for model selection
#[derive(Debug, Clone, PartialEq)]
pub enum RoutingCondition {
    /// Task involves images or vision
    VisionRequired,
    /// Task requires complex reasoning
    ComplexReasoning,
    /// Simple task that can use fast model
    SimpleTask,
    /// Code-related task
    CodeRelated,
    /// Default fallback
    Default,
}

/// Routing rule
#[derive(Debug, Clone)]
pub struct RoutingRule {
    pub condition: RoutingCondition,
    pub model: ModelId,
    pub priority: i32,
}

/// Model selector for Agent Loop - selects appropriate AI model based on task
///
/// This is distinct from `dispatcher::model_router::ModelRouter` trait which
/// provides a more comprehensive routing system with health checks and failover.
///
/// Use `ThinkerModelSelector` for simple model selection based on Observation.
pub struct ThinkerModelSelector {
    config: ModelRoutingConfig,
    rules: Vec<RoutingRule>,
}

impl ThinkerModelSelector {
    /// Create a new model selector
    pub fn new(config: ModelRoutingConfig) -> Self {
        let rules = Self::build_rules(&config);
        Self { config, rules }
    }

    /// Build routing rules from config
    fn build_rules(config: &ModelRoutingConfig) -> Vec<RoutingRule> {
        vec![
            RoutingRule {
                condition: RoutingCondition::VisionRequired,
                model: ModelId::new(&config.vision_model),
                priority: 100,
            },
            RoutingRule {
                condition: RoutingCondition::ComplexReasoning,
                model: ModelId::new(&config.reasoning_model),
                priority: 80,
            },
            RoutingRule {
                condition: RoutingCondition::CodeRelated,
                model: ModelId::new(&config.default_model),
                priority: 60,
            },
            RoutingRule {
                condition: RoutingCondition::SimpleTask,
                model: ModelId::new(&config.fast_model),
                priority: 40,
            },
            RoutingRule {
                condition: RoutingCondition::Default,
                model: ModelId::new(&config.default_model),
                priority: 0,
            },
        ]
    }

    /// Select the appropriate model based on observation
    pub fn select(&self, observation: &Observation) -> ModelId {
        if !self.config.auto_route {
            return ModelId::new(&self.config.default_model);
        }

        let conditions = self.detect_conditions(observation);

        // Find highest priority matching rule
        let mut best_rule: Option<&RoutingRule> = None;

        for rule in &self.rules {
            if conditions.contains(&rule.condition) {
                match best_rule {
                    None => best_rule = Some(rule),
                    Some(current) if rule.priority > current.priority => {
                        best_rule = Some(rule);
                    }
                    _ => {}
                }
            }
        }

        best_rule
            .map(|r| r.model.clone())
            .unwrap_or_else(|| ModelId::new(&self.config.default_model))
    }

    /// Detect applicable routing conditions from observation
    fn detect_conditions(&self, observation: &Observation) -> Vec<RoutingCondition> {
        let mut conditions = vec![RoutingCondition::Default];

        // Check for vision requirement
        if self.has_images(observation) {
            conditions.push(RoutingCondition::VisionRequired);
        }

        // Check for complex reasoning indicators
        if self.is_complex_task(observation) {
            conditions.push(RoutingCondition::ComplexReasoning);
        }

        // Check for code-related task
        if self.is_code_related(observation) {
            conditions.push(RoutingCondition::CodeRelated);
        }

        // Check for simple task
        if self.is_simple_task(observation) {
            conditions.push(RoutingCondition::SimpleTask);
        }

        conditions
    }

    /// Check if observation contains images
    fn has_images(&self, observation: &Observation) -> bool {
        observation.attachments.iter().any(|a| {
            a.media_type == "image" || a.mime_type.starts_with("image/")
        })
    }

    /// Check if task appears complex
    fn is_complex_task(&self, observation: &Observation) -> bool {
        // Heuristics for complex tasks:
        // 1. Many steps already taken
        // 2. Multiple failed attempts
        // 3. Long history summary

        if observation.current_step > 5 {
            return true;
        }

        let failure_count = observation
            .recent_steps
            .iter()
            .filter(|s| !s.success)
            .count();

        if failure_count >= 2 {
            return true;
        }

        if observation.history_summary.len() > 1000 {
            return true;
        }

        false
    }

    /// Check if task is code-related
    fn is_code_related(&self, observation: &Observation) -> bool {
        // Check recent steps for code-related tools
        observation.recent_steps.iter().any(|s| {
            let action = s.action_type.to_lowercase();
            action.contains("code")
                || action.contains("execute")
                || action.contains("compile")
                || action.contains("debug")
        })
    }

    /// Check if task is simple
    fn is_simple_task(&self, observation: &Observation) -> bool {
        // Simple task: first step, no attachments, no complex history
        observation.current_step == 0
            && observation.attachments.is_empty()
            && observation.history_summary.is_empty()
    }

    /// Get the default model
    pub fn default_model(&self) -> ModelId {
        ModelId::new(&self.config.default_model)
    }

    /// Get the vision model
    pub fn vision_model(&self) -> ModelId {
        ModelId::new(&self.config.vision_model)
    }

    /// Get the fast model
    pub fn fast_model(&self) -> ModelId {
        ModelId::new(&self.config.fast_model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::StepSummary;
    use crate::core::MediaAttachment;

    fn default_config() -> ModelRoutingConfig {
        ModelRoutingConfig {
            default_model: "claude-sonnet".to_string(),
            vision_model: "claude-vision".to_string(),
            reasoning_model: "claude-opus".to_string(),
            fast_model: "claude-haiku".to_string(),
            auto_route: true,
        }
    }

    #[test]
    fn test_default_routing() {
        let selector = ThinkerModelSelector::new(default_config());

        let observation = Observation {
            history_summary: String::new(),
            recent_steps: vec![],
            available_tools: vec![],
            attachments: vec![],
            current_step: 0,
            total_tokens: 0,
        };

        // Simple task should use fast model
        let model = selector.select(&observation);
        assert_eq!(model.as_str(), "claude-haiku");
    }

    #[test]
    fn test_vision_routing() {
        let selector = ThinkerModelSelector::new(default_config());

        let observation = Observation {
            history_summary: String::new(),
            recent_steps: vec![],
            available_tools: vec![],
            attachments: vec![MediaAttachment {
                media_type: "image".to_string(),
                mime_type: "image/png".to_string(),
                data: String::new(),
                encoding: "base64".to_string(),
                filename: None,
                size_bytes: 0,
            }],
            current_step: 0,
            total_tokens: 0,
        };

        let model = selector.select(&observation);
        assert_eq!(model.as_str(), "claude-vision");
    }

    #[test]
    fn test_complex_task_routing() {
        let selector = ThinkerModelSelector::new(default_config());

        let observation = Observation {
            history_summary: "x".repeat(1500), // Long history
            recent_steps: vec![],
            available_tools: vec![],
            attachments: vec![],
            current_step: 6,
            total_tokens: 5000,
        };

        let model = selector.select(&observation);
        assert_eq!(model.as_str(), "claude-opus");
    }

    #[test]
    fn test_auto_route_disabled() {
        let mut config = default_config();
        config.auto_route = false;

        let selector = ThinkerModelSelector::new(config);

        let observation = Observation {
            history_summary: String::new(),
            recent_steps: vec![],
            available_tools: vec![],
            attachments: vec![MediaAttachment {
                media_type: "image".to_string(),
                mime_type: "image/png".to_string(),
                data: String::new(),
                encoding: "base64".to_string(),
                filename: None,
                size_bytes: 0,
            }],
            current_step: 0,
            total_tokens: 0,
        };

        // Should always return default when auto_route is disabled
        let model = selector.select(&observation);
        assert_eq!(model.as_str(), "claude-sonnet");
    }

    #[test]
    fn test_code_related_routing() {
        let selector = ThinkerModelSelector::new(default_config());

        let observation = Observation {
            history_summary: String::new(),
            recent_steps: vec![StepSummary {
                step_id: 0,
                reasoning: "Execute code".to_string(),
                action_type: "tool:execute_code".to_string(),
                action_args: "{}".to_string(),
                result_summary: "Success".to_string(),
                result_output: "Success".to_string(),
                success: true,
                tool_call_id: None,
            }],
            available_tools: vec![],
            attachments: vec![],
            current_step: 1,
            total_tokens: 100,
        };

        let model = selector.select(&observation);
        // Code-related uses default model
        assert_eq!(model.as_str(), "claude-sonnet");
    }
}
