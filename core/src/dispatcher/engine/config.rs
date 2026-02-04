//! Agent configuration types

use crate::dispatcher::model_router::{ModelProfile, ModelRoutingRules};

/// Configuration for the Agent engine
///
/// Renamed from CoworkConfig to reflect the agent-centric architecture.
/// Note: Core execution parameters (confirmation, parallelism, retries) are hardcoded
/// for security and stability. Only model routing settings are configurable.
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct AgentConfig {
    /// Model profiles for multi-model routing
    pub model_profiles: Vec<ModelProfile>,

    /// Model routing rules (contains enable_pipelines flag)
    pub routing_rules: Option<ModelRoutingRules>,
}


impl AgentConfig {
    /// Create config with model routing enabled
    pub fn with_model_routing(
        mut self,
        profiles: Vec<ModelProfile>,
        rules: ModelRoutingRules,
    ) -> Self {
        self.model_profiles = profiles;
        self.routing_rules = Some(rules);
        self
    }

    /// Check if pipelines are enabled
    pub fn pipelines_enabled(&self) -> bool {
        self.routing_rules
            .as_ref()
            .is_some_and(|r| r.enable_pipelines)
    }
}

/// Current execution state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    /// Not executing
    Idle,
    /// Planning a task
    Planning,
    /// Waiting for user confirmation
    AwaitingConfirmation,
    /// Executing tasks
    Executing,
    /// Execution paused
    Paused,
    /// Execution cancelled
    Cancelled,
    /// Execution completed
    Completed,
}
