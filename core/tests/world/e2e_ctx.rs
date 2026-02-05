//! E2E context for BDD tests (Evolution, YAML policies)

use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

use alephcore::skill_evolution::{EvolutionTracker, PipelineResult, SolidificationSuggestion};
use alephcore::tools::markdown_skill::EvolutionAutoLoader;
use alephcore::tools::AlephToolServer;
use alephcore::daemon::dispatcher::policy::{PolicyEngine, ProposedAction};
use alephcore::daemon::events::DerivedEvent;
use alephcore::daemon::worldmodel::state::EnhancedContext;

/// Context for E2E tests (evolution, policies)
#[derive(Default)]
pub struct E2eContext {
    // Evolution fields
    /// In-memory evolution tracker
    pub tracker: Option<Arc<EvolutionTracker>>,
    /// Tool server for evolution tests
    pub tool_server: Option<Arc<AlephToolServer>>,
    /// Evolution auto-loader
    pub auto_loader: Option<Arc<EvolutionAutoLoader>>,
    /// Temporary output directory
    pub temp_dir: Option<TempDir>,
    /// Solidification pipeline result
    pub solidification_result: Option<PipelineResult>,
    /// Number of tools loaded from auto-loader
    pub loaded_count: usize,
    /// Current suggestion being processed
    pub current_suggestion: Option<SolidificationSuggestion>,
    /// Batch load result
    pub batch_result: Option<BatchLoadResult>,
    /// Generated skill name for verification
    pub generated_skill_name: Option<String>,

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

/// Batch load result structure
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
            .field("tracker", &self.tracker.as_ref().map(|_| "EvolutionTracker"))
            .field("tool_server", &self.tool_server.as_ref().map(|_| "AlephToolServer"))
            .field("auto_loader", &self.auto_loader.as_ref().map(|_| "EvolutionAutoLoader"))
            .field("temp_dir", &self.temp_dir.as_ref().map(|_| "TempDir"))
            .field("solidification_result", &self.solidification_result.as_ref().map(|r| r.suggestions.len()))
            .field("loaded_count", &self.loaded_count)
            .field("current_suggestion", &self.current_suggestion.as_ref().map(|s| &s.suggested_name))
            .field("batch_result", &self.batch_result)
            .field("generated_skill_name", &self.generated_skill_name)
            .field("policy_engine", &self.policy_engine.as_ref().map(|_| "PolicyEngine"))
            .field("enhanced_context", &self.enhanced_context.as_ref().map(|_| "EnhancedContext"))
            .field("derived_event", &self.derived_event.as_ref().map(|_| "DerivedEvent"))
            .field("triggered_actions", &self.triggered_actions.len())
            .field("yaml_path", &self.yaml_path)
            .finish()
    }
}
