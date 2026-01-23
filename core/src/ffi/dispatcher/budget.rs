//! Budget management FFI methods
//!
//! Contains budget status and limit management:
//! - agent_get_budget_status
//! - agent_get_budget_status_for_scope
//! - agent_get_budget_limit

use crate::ffi::AetherCore;

impl AetherCore {
    // =========================================================================
    // Budget Management (Model Router P1)
    // =========================================================================

    /// Get budget status overview
    ///
    /// Returns the overall budget status including all configured limits,
    /// current spending, and warning/exceeded states.
    pub fn agent_get_budget_status(&self) -> crate::ffi::dispatcher_types::BudgetStatusFFI {
        // Load config and get budget limits
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled(),
        };

        // Get budget configuration from cowork.model_routing.budget
        let budget_config = &config.agent.model_routing.budget;

        if !budget_config.enabled {
            return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled();
        }

        // Convert config limits to internal BudgetLimit types
        let default_enforcement = &budget_config.default_enforcement;
        let limits: Vec<crate::dispatcher::model_router::BudgetLimit> = budget_config
            .limits
            .iter()
            .map(|l| l.to_budget_limit(default_enforcement))
            .collect();

        if limits.is_empty() {
            return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled();
        }

        // Create initial states for each limit
        // TODO: When BudgetManager is integrated into AgentEngine, use actual states
        let mut states = std::collections::HashMap::new();
        for limit in &limits {
            states.insert(
                limit.id.clone(),
                crate::dispatcher::model_router::BudgetState::new(limit),
            );
        }

        crate::ffi::dispatcher_types::BudgetStatusFFI::from_limits_and_states(&limits, &states)
    }

    /// Get budget status for a specific scope
    ///
    /// Returns budget limits and status that apply to the given scope.
    pub fn agent_get_budget_status_for_scope(
        &self,
        scope_type: String,
        scope_id: Option<String>,
    ) -> crate::ffi::dispatcher_types::BudgetStatusFFI {
        // Parse scope
        let scope = match scope_type.as_str() {
            "global" => crate::dispatcher::model_router::BudgetScope::Global,
            "project" => {
                crate::dispatcher::model_router::BudgetScope::Project(scope_id.unwrap_or_default())
            }
            "session" => {
                crate::dispatcher::model_router::BudgetScope::Session(scope_id.unwrap_or_default())
            }
            "model" => {
                crate::dispatcher::model_router::BudgetScope::Model(scope_id.unwrap_or_default())
            }
            _ => return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled(),
        };

        // Load config and get budget limits
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled(),
        };

        let budget_config = &config.agent.model_routing.budget;

        if !budget_config.enabled {
            return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled();
        }

        // Convert config limits to internal BudgetLimit types
        let default_enforcement = &budget_config.default_enforcement;
        let all_limits: Vec<crate::dispatcher::model_router::BudgetLimit> = budget_config
            .limits
            .iter()
            .map(|l| l.to_budget_limit(default_enforcement))
            .collect();

        // Filter to limits that apply to this scope
        let applicable_limits: Vec<_> = all_limits
            .into_iter()
            .filter(|l| {
                l.scope.contains(&scope)
                    || l.scope == crate::dispatcher::model_router::BudgetScope::Global
            })
            .collect();

        if applicable_limits.is_empty() {
            return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled();
        }

        // Create initial states for each limit
        // TODO: When BudgetManager is integrated, use actual states
        let mut states = std::collections::HashMap::new();
        for limit in &applicable_limits {
            states.insert(
                limit.id.clone(),
                crate::dispatcher::model_router::BudgetState::new(limit),
            );
        }

        crate::ffi::dispatcher_types::BudgetStatusFFI::from_limits_and_states(&applicable_limits, &states)
    }

    /// Get a single budget limit status by ID
    ///
    /// Returns the status of a specific budget limit, or None if not found.
    pub fn agent_get_budget_limit(
        &self,
        limit_id: String,
    ) -> Option<crate::ffi::dispatcher_types::BudgetLimitStatusFFI> {
        // Load config
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return None,
        };

        let budget_config = &config.agent.model_routing.budget;

        if !budget_config.enabled {
            return None;
        }

        // Find the limit by ID
        let limit_config = budget_config.limits.iter().find(|l| l.id == limit_id)?;

        let default_enforcement = &budget_config.default_enforcement;
        let limit = limit_config.to_budget_limit(default_enforcement);
        let state = crate::dispatcher::model_router::BudgetState::new(&limit);

        Some(crate::ffi::dispatcher_types::BudgetLimitStatusFFI::from_limit_and_state(&limit, &state))
    }
}
