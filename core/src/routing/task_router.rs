//! Task routing decision layer — core types and trait.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Routing decision for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskRoute {
    Simple,
    MultiStep { reason: String },
    Critical { reason: String, manifest_hints: ManifestHints },
    Collaborative { reason: String, strategy: CollabStrategy },
}

impl TaskRoute {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Simple => "simple",
            Self::MultiStep { .. } => "multi_step",
            Self::Critical { .. } => "critical",
            Self::Collaborative { .. } => "collaborative",
        }
    }
}

/// Hints for constructing a success manifest on critical tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestHints {
    pub hard_constraints: Vec<String>,
    pub quality_threshold: f64,
}

impl Default for ManifestHints {
    fn default() -> Self {
        Self {
            hard_constraints: Vec::new(),
            quality_threshold: 0.7,
        }
    }
}

/// Strategy for collaborative multi-agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollabStrategy {
    Parallel,
    Adversarial,
    GroupChat,
}

/// Context provided to the router for classification.
pub struct RouterContext {
    pub session_history_len: usize,
    pub available_tools: Vec<String>,
    pub user_preferences: Option<String>,
}

/// Context for evaluating whether a running task should be escalated.
pub struct EscalationContext {
    pub step_count: usize,
    pub tools_invoked: Vec<String>,
    pub has_failures: bool,
    pub original_message: String,
}

/// Snapshot of task state at escalation time.
#[derive(Debug, Clone)]
pub struct EscalationSnapshot {
    pub original_message: String,
    pub completed_steps: usize,
    pub tools_invoked: Vec<String>,
    pub partial_result: Option<String>,
}

/// Trait for task routing implementations.
#[async_trait]
pub trait TaskRouter: Send + Sync {
    /// Classify an incoming message into a task route.
    async fn classify(&self, message: &str, context: &RouterContext) -> TaskRoute;

    /// Check whether a running task should be escalated to a higher route.
    async fn should_escalate(&self, state: &EscalationContext) -> Option<TaskRoute>;
}
