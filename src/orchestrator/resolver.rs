//! Pure routing + depth guard + session resolution helpers.
//! See design §6 (dispatch step 1, 2, 4), §7 (MAX_FLOW_DEPTH).

use std::collections::HashMap;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{AgentId, FlowId, SessionStrategy};

/// Hardcoded maximum depth for `flow_run` recursion. See design §7.
pub const MAX_FLOW_DEPTH: u8 = 4;

pub fn depth_guard(depth: u8) -> Result<(), FlowError> {
    if depth > MAX_FLOW_DEPTH {
        Err(FlowError::RecursionLimit {
            max: MAX_FLOW_DEPTH,
        })
    } else {
        Ok(())
    }
}

/// User-provided routing overrides from `aleph.toml [flow_routing]`.
#[derive(Debug, Default, Clone)]
pub struct RoutingOverrides {
    /// `(agent, channel) → flow_id` — exact match.
    pub exact: HashMap<(AgentId, String), FlowId>,
    /// `agent → flow_id` — wildcard (any channel).
    pub wildcard: HashMap<AgentId, FlowId>,
}

/// Map `(agent_id, channel)` → `flow_id`. Precedence:
/// 1. exact `(agent, channel)` override
/// 2. wildcard `agent` override
/// 3. default table (agent_id == flow_id fallback from builtin table)
pub fn resolve_flow_id(
    agent_id: &str,
    channel: Option<&str>,
    overrides: &RoutingOverrides,
    defaults: &HashMap<String, String>,
) -> Result<FlowId, FlowError> {
    if let Some(ch) = channel {
        if let Some(id) = overrides
            .exact
            .get(&(agent_id.to_string(), ch.to_string()))
        {
            return Ok(id.clone());
        }
    }
    if let Some(id) = overrides.wildcard.get(agent_id) {
        return Ok(id.clone());
    }
    if let Some(id) = defaults.get(agent_id) {
        return Ok(id.clone());
    }
    Err(FlowError::UnknownAgent(agent_id.to_string()))
}

/// Decide which `SessionKey` a dispatch writes to.
/// Phase 5 keeps this pure; Orchestrator applies the per-session lock separately.
#[derive(Debug, Clone)]
pub struct SessionResolution {
    pub session_key: String,
    pub parent_session_key: Option<String>,
    pub is_new: bool,
}

#[derive(Debug, Clone)]
pub struct SessionResolveInput {
    pub strategy: SessionStrategy,
    pub session_hint: Option<String>,
    pub parent_session: Option<String>,
    pub fresh_key_fn: fn() -> String,
}

pub fn resolve_session(input: SessionResolveInput) -> Result<SessionResolution, FlowError> {
    match input.strategy {
        SessionStrategy::Reuse => match input.session_hint {
            Some(k) => Ok(SessionResolution {
                session_key: k,
                parent_session_key: None,
                is_new: false,
            }),
            None => Err(FlowError::InvalidConfig(
                "SessionStrategy::Reuse requires session_hint".into(),
            )),
        },
        SessionStrategy::Fresh => Ok(SessionResolution {
            session_key: (input.fresh_key_fn)(),
            parent_session_key: None,
            is_new: true,
        }),
        SessionStrategy::Child { parent_session_key } => {
            let parent = parent_session_key
                .or(input.parent_session.clone())
                .ok_or_else(|| {
                    FlowError::InvalidConfig(
                        "SessionStrategy::Child requires parent_session at runtime".into(),
                    )
                })?;
            Ok(SessionResolution {
                session_key: (input.fresh_key_fn)(),
                parent_session_key: Some(parent),
                is_new: true,
            })
        }
    }
}
