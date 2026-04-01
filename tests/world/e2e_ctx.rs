//! E2E context for BDD tests (YAML policies)
//! NOTE: Evolution context removed — skill_evolution module deleted

use std::path::PathBuf;

use alephcore::daemon::dispatcher::policy::{PolicyEngine, ProposedAction};
use alephcore::daemon::events::DerivedEvent;
use alephcore::daemon::worldmodel::state::EnhancedContext;

/// Context for E2E tests (policies)
#[derive(Default)]
pub struct E2eContext {
    // Policy fields
    /// Policy engine
    pub policy_engine: Option<PolicyEngine>,
    /// Enhanced context for policy evaluation
    pub enhanced_context: Option<EnhancedContext>,
    /// Derived event for policy evaluation
    pub derived_event: Option<DerivedEvent>,
    /// Actions triggered by policy evaluation
    pub triggered_actions: Vec<ProposedAction>,
    /// YAML file path
    pub yaml_path: Option<PathBuf>,
    /// YAML file content
    pub yaml_content: Option<String>,
}

/// Batch load result structure (kept for compatibility, may be unused now)
#[derive(Debug, Clone)]
pub struct BatchLoadResult {
    pub total: usize,
    pub loaded: usize,
    pub failed: usize,
}

impl BatchLoadResult {
    pub fn all_succeeded(&self) -> bool {
        self.failed == 0 && self.loaded == self.total
    }

    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.loaded as f64 / self.total as f64
        }
    }
}

impl std::fmt::Debug for E2eContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2eContext")
            .field(
                "policy_engine",
                &self.policy_engine.as_ref().map(|_| "PolicyEngine"),
            )
            .field(
                "enhanced_context",
                &self.enhanced_context.as_ref().map(|_| "EnhancedContext"),
            )
            .field(
                "derived_event",
                &self.derived_event.as_ref().map(|_| "DerivedEvent"),
            )
            .field("triggered_actions", &self.triggered_actions.len())
            .field("yaml_path", &self.yaml_path)
            .finish()
    }
}
