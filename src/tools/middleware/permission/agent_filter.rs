//! AgentPermissionFilter — convenience builder that merges global + per-agent
//! permissions into a [`LayeredPermissionResolver`].
//!
//! Used by orchestrator paths that know which agent is running and want to
//! hand a ready-to-use `SmartFilter` to `PermissionLayer::set_smart_filter`.

use std::sync::Arc;

use crate::config::types::policies::tool_permissions::ToolPermissionsConfig;

use super::resolver::LayeredPermissionResolver;
use super::SmartFilter;

/// Stateless helper namespace for building per-agent `SmartFilter`s.
///
/// Kept as a unit struct so we can add more constructor variants (e.g.
/// with custom reason strings or logging hooks) without breaking callers.
pub struct AgentPermissionFilter;

impl AgentPermissionFilter {
    /// Build a `SmartFilter` that resolves through the merged two-tier
    /// permission table for a specific agent.
    pub fn build(
        global: &ToolPermissionsConfig,
        agent: &ToolPermissionsConfig,
    ) -> Arc<dyn SmartFilter> {
        Arc::new(LayeredPermissionResolver::from_layers(global, agent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::PermissionAction;
    use crate::tools::middleware::permission::Classification;
    use serde_json::Value;
    use std::collections::HashMap;

    #[test]
    fn build_returns_resolver_with_most_restrictive_classification() {
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: HashMap::new(),
        };
        let mut agent_overrides = HashMap::new();
        agent_overrides.insert("shell".to_string(), PermissionAction::Deny);
        let agent = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: agent_overrides,
        };

        let filter = AgentPermissionFilter::build(&global, &agent);
        match filter.classify("shell", &Value::Null) {
            Classification::Deny { .. } => {}
            other => panic!("expected Deny, got {other:?}"),
        }
        assert_eq!(
            filter.classify("other", &Value::Null),
            Classification::Allow
        );
    }
}
