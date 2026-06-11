//! Task classification value types.
//!
//! Consumed by the `gateway_route` builtin tool (via the
//! [`super::rules::RoutingRules`] regex classifier) to report how a message
//! would be classified. The orchestration trait that once drove
//! single-vs-multi-agent dispatch was removed when the Dispatcher dissolved
//! (R7/R10) — only the classification value types survive.

use serde::{Deserialize, Serialize};

/// Routing decision for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskRoute {
    Simple,
    MultiStep {
        reason: String,
    },
    Critical {
        reason: String,
        manifest_hints: ManifestHints,
    },
    Collaborative {
        reason: String,
        strategy: CollabStrategy,
    },
}

impl TaskRoute {
    #[must_use]
    pub const fn label(&self) -> &'static str {
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
