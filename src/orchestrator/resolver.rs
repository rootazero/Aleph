//! Pure routing + depth guard + session resolution helpers.
//! See design §6 (dispatch step 1, 2, 4), §7 (`MAX_FLOW_DEPTH`).

use std::collections::HashMap;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{AgentId, FlowId, SessionStrategy};

/// Hardcoded maximum depth for **flow-to-flow** recursive dispatch (one flow's
/// run dispatching another). See design §7.
///
/// Scope matters, because the obvious reading is wrong: this does **not** cap
/// delegation depth. Sub-agents never reach `Orchestrator::dispatch` — the
/// spawner drives `AgentHarness` directly — so the live delegation-depth guard
/// is `ChainContext::child()` in `agents::subagent_spawner`, which refuses past
/// its own limit with "chain depth exceeded". Today the only production
/// `FlowRequest` producer passes `depth: 0`, so `depth_guard` is a fail-closed
/// limit standing over a door nobody has opened yet. It is kept rather than cut
/// precisely because it fails closed: the cost is a comparison per dispatch,
/// and the first producer to pass a non-zero depth gets a bound for free.
pub const MAX_FLOW_DEPTH: u8 = 4;

/// Canonical generic-agent flow id. Any registered agent that has no explicit
/// per-agent flow routes through this. The harness loads the agent's identity
/// from `~/.aleph/agents/<id>/` by `agent_id`, so a single flow serves every
/// agent — the orchestrator routing table never needs a per-agent entry.
pub const DEFAULT_AGENT_FLOW_ID: &str = "default-agent";

/// Allows `depth ∈ [0, MAX_FLOW_DEPTH]`; rejects strictly greater.
/// Called at every dispatch (see design §7).
pub const fn depth_guard(depth: u8) -> Result<(), FlowError> {
    if depth > MAX_FLOW_DEPTH {
        Err(FlowError::RecursionLimit {
            max: MAX_FLOW_DEPTH,
        })
    } else {
        Ok(())
    }
}

/// Map `agent_id` → `flow_id` through the default routing table.
///
/// There is deliberately only one rung. Two override rungs — exact
/// `(agent, channel)` and wildcard `agent` — used to sit above this one, fed
/// by a `RoutingOverrides` struct whose only producer was to have been an
/// `[flow_routing]` config key. That key was never implemented, so every
/// construction site passed `RoutingOverrides::default()` and neither rung
/// ever fired. Both were cut rather than given a config surface: "which flow
/// serves agent X" is already answered by this table plus
/// `~/.aleph/flows/<id>.toml`, and a third answer keyed on channel is the one
/// answer too many. `FlowRequest.channel` survives as a diagnostics-only
/// field; it is no longer a routing input.
pub fn resolve_flow_id(
    agent_id: &str,
    defaults: &HashMap<AgentId, FlowId>,
) -> Result<FlowId, FlowError> {
    defaults
        .get(agent_id)
        // rust-doctor-disable-next-line excessive-clone
        .cloned()
        .ok_or_else(|| FlowError::UnknownAgent(agent_id.to_string()))
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
            Some(k) if !k.is_empty() => Ok(SessionResolution {
                session_key: k,
                parent_session_key: None,
                is_new: false,
            }),
            _ => Err(FlowError::InvalidConfig(
                "SessionStrategy::Reuse requires a non-empty session_hint".into(),
            )),
        },
        SessionStrategy::Fresh => Ok(SessionResolution {
            session_key: (input.fresh_key_fn)(),
            parent_session_key: None,
            is_new: true,
        }),
        SessionStrategy::Child { parent_session_key } => {
            let parent = input
                .parent_session
                .filter(|s| !s.is_empty())
                .or(parent_session_key.filter(|s| !s.is_empty()));
            // Degrade gracefully instead of hard-failing: a Child flow whose
            // parent is missing at runtime (gateway runs currently pass
            // `parent_session: None`, and the preset flows declare Child
            // without a static parent) must still be servable — fall back to a
            // fresh session rather than InvalidConfig, which would make every
            // dispatch to the flow fail. The parent link, when present, is
            // still carried.
            let Some(parent) = parent else {
                tracing::debug!(
                    strategy = "Child",
                    "resolve_session: Child strategy with no runtime parent — falling back to Fresh"
                );
                return Ok(SessionResolution {
                    session_key: (input.fresh_key_fn)(),
                    parent_session_key: None,
                    is_new: true,
                });
            };
            Ok(SessionResolution {
                session_key: (input.fresh_key_fn)(),
                parent_session_key: Some(parent),
                is_new: true,
            })
        }
    }
}
