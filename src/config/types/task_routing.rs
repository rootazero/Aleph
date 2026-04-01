//! Task routing decision layer configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::routing::rules::RoutingPatternsConfig;

/// Task routing decision layer configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskRoutingConfig {
    /// Enable the routing decision layer
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Enable LLM fallback when rules don't match
    #[serde(default = "default_true")]
    pub enable_llm_fallback: bool,

    /// Model tier for classification ("fast" = cheapest available)
    #[serde(default = "default_classify_model")]
    pub classify_model: String,

    /// Step count threshold for dynamic escalation
    #[serde(default = "default_escalation_threshold")]
    pub escalation_step_threshold: usize,

    /// Enable dynamic escalation from within Agent Loop
    #[serde(default = "default_true")]
    pub escalation_enabled: bool,

    /// Max parallel agents for Swarm
    #[serde(default = "default_max_parallel")]
    pub max_parallel_agents: usize,

    /// Max rounds for adversarial verification
    #[serde(default = "default_adversarial_rounds")]
    pub adversarial_max_rounds: usize,

    /// Pattern matching rules
    #[serde(default)]
    pub patterns: RoutingPatternsConfig,
}

impl Default for TaskRoutingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            enable_llm_fallback: true,
            classify_model: "fast".into(),
            escalation_step_threshold: 3,
            escalation_enabled: true,
            max_parallel_agents: 4,
            adversarial_max_rounds: 3,
            patterns: RoutingPatternsConfig::default(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_classify_model() -> String {
    "fast".into()
}
fn default_escalation_threshold() -> usize {
    3
}
fn default_max_parallel() -> usize {
    4
}
fn default_adversarial_rounds() -> usize {
    3
}
