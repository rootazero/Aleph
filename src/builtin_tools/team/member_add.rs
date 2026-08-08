//! `TeamMemberAddTool` — add a member to an existing team.
//!
//! Symmetric counterpart to [`TeamMemberRemoveTool`](super::TeamMemberRemoveTool).
//! Critically, this is the R8 entry point for bringing external CLI agents
//! (Claude Code, Codex, Gemini CLI, …) into a shared team session by passing
//! an `acp:<harness>[/<session>]` ID — the team dispatcher's ACP bridge
//! routes execution to [`crate::acp::manager::AcpAdapterManager`] at run
//! time, so no registry registration is required for ACP members.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::gateway::agent_instance::AgentRegistry;
use crate::sync_primitives::Arc;
use crate::teams::dispatcher::AcpMemberRef;
use crate::teams::{NewTeamMember, TeamStore};
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for adding a member to a team.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamMemberAddArgs {
    /// ID of the team to add the member to.
    pub team_id: String,
    /// Agent identifier. Accepts either:
    /// - A registered native agent ID (e.g. `"researcher"`, `"coder"`).
    /// - An ACP harness reference: `"acp:claude-code"`,
    ///   `"acp:codex/backend"`, `"acp:gemini/frontend"`. Named sessions let
    ///   the same harness participate as multiple personas in one team.
    pub agent_id: String,
    /// Role within the team (free-form, e.g. `"reviewer"`, `"researcher"`).
    /// Empty string = unspecified.
    #[serde(default)]
    pub role: String,
}

/// Output from `team_member_add`.
#[derive(Debug, Clone, Serialize)]
pub struct TeamMemberAddOutput {
    pub message: String,
    pub agent_id: String,
    pub team_id: String,
    /// `true` when the new member is an ACP harness reference; `false` when
    /// the member is a native registered agent.
    pub is_acp: bool,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that adds a member to an existing team.
///
/// Only the team leader may add members. ACP harness IDs bypass the registry
/// lookup since they identify external CLI processes managed by the ACP
/// adapter manager rather than native `AgentInstance`s.
#[derive(Clone)]
pub struct TeamMemberAddTool {
    store: Arc<dyn TeamStore>,
    registry: Arc<AgentRegistry>,
    /// Fallback actor for calls made outside a turn scope. The live identity
    /// comes from [`acting_agent_id`] — see [`Self::actor`].
    pub current_agent_id: String,
}

impl TeamMemberAddTool {
    pub fn new(
        store: Arc<dyn TeamStore>,
        registry: Arc<AgentRegistry>,
        current_agent_id: impl Into<String>,
    ) -> Self {
        Self {
            store,
            registry,
            current_agent_id: current_agent_id.into(),
        }
    }

    /// The agent acting in THIS call — the running turn's identity, not the
    /// one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }
}

#[async_trait]
impl AlephTool for TeamMemberAddTool {
    const NAME: &'static str = "team_member_add";
    const DESCRIPTION: &'static str =
        "Add a member to an existing team. Only the team leader can add members. \
        The agent_id accepts either a registered native agent ID (e.g. 'researcher') \
        OR an ACP harness reference of the form 'acp:<harness>[/<session>]' to bring \
        external CLI agents into the team. Supported harness IDs include: claude-code, \
        codex, gemini, opencode, kimi, cursor, copilot, droid, pi, iflow, kilocode, kiro, \
        qoder, qwen, trae, openclaw. Named sessions let the same harness participate as \
        multiple personas (e.g. 'acp:codex/backend' and 'acp:codex/frontend').";

    type Args = TeamMemberAddArgs;
    type Output = TeamMemberAddOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Verify the team exists and the caller is the leader.
        let team = self
            .store
            .get_team(&args.team_id)
            .await?
            .ok_or_else(|| AlephError::tool(format!("team not found: {}", args.team_id)))?;

        if team.leader_id != self.actor() {
            return Err(AlephError::tool(format!(
                "only the team leader ('{}') can add members",
                team.leader_id
            )));
        }

        // Validate the agent_id: either an ACP reference (no registry lookup
        // needed — the dispatcher's ACP bridge resolves at run time) or a
        // registered native agent.
        let is_acp = AcpMemberRef::parse(&args.agent_id).is_some();
        if !is_acp && self.registry.get(&args.agent_id).await.is_none() {
            // A long-form roster id (`acp:<harness>:<cwd>[:<name>]`) is
            // display-only — harness ids never contain ':'. Steer to the
            // explicit-cwd tool instead of the generic not-found error.
            if args.agent_id.starts_with("acp:") {
                return Err(AlephError::tool(format!(
                    "'{}' is not a valid ACP reference: harness ids never \
                     contain ':' (long-form roster ids are display-only). Use \
                     team_acp_member(harness_id=…, cwd=…) to add an ACP member \
                     with an explicit cwd, or the short form \
                     'acp:<harness>[/<session>]'",
                    args.agent_id
                )));
            }
            return Err(AlephError::tool(format!(
                "agent '{}' not found in registry and not a valid ACP reference \
                 (use 'acp:<harness>[/<session>]' for external CLI agents)",
                args.agent_id
            )));
        }

        // Build NewTeamMember. For ACP references the helper auto-derives
        // a stable agent_id from harness/cwd/session — but this tool is
        // documented to honour the caller-supplied id verbatim, so we
        // populate the struct manually here.
        let new_member = if let Some(member_ref) = AcpMemberRef::parse(&args.agent_id) {
            NewTeamMember {
                team_id: args.team_id.clone(),
                agent_id: args.agent_id.clone(),
                role: args.role.clone(),
                kind: crate::teams::types::TeamMemberKind::AcpSession,
                acp_harness_id: Some(member_ref.harness_id.clone()),
                // The `acp:<harness>[/<session>]` ref carries no cwd, but the
                // dispatcher force-fails any ACP member with no cwd. Default to
                // the active project root (then the server cwd) so the member is
                // dispatchable; use `team_acp_member` to set an explicit cwd.
                acp_cwd: Some(super::acp_default_cwd()),
                acp_session_name: member_ref.session_name,
            }
        } else {
            NewTeamMember::for_agent(
                args.team_id.clone(),
                args.agent_id.clone(),
                args.role.clone(),
            )
        };
        self.store.add_member(new_member).await?;

        info!(
            team_id = %args.team_id,
            agent_id = %args.agent_id,
            is_acp = is_acp,
            "team_member_add: member added"
        );

        let kind = if is_acp { "ACP harness" } else { "agent" };
        Ok(TeamMemberAddOutput {
            message: format!(
                "{} '{}' has been added to team '{}'.",
                kind, args.agent_id, args.team_id
            ),
            agent_id: args.agent_id,
            team_id: args.team_id,
            is_acp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_deserialize_with_default_role() {
        let json = r#"{"team_id":"t1","agent_id":"acp:claude-code"}"#;
        let args: TeamMemberAddArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.team_id, "t1");
        assert_eq!(args.agent_id, "acp:claude-code");
        assert_eq!(args.role, "");
    }

    #[test]
    fn args_deserialize_with_explicit_role() {
        let json = r#"{"team_id":"t1","agent_id":"researcher","role":"lead"}"#;
        let args: TeamMemberAddArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.role, "lead");
    }

    #[test]
    fn output_serialize_marks_acp_correctly() {
        let out = TeamMemberAddOutput {
            message: "ok".into(),
            agent_id: "acp:codex/backend".into(),
            team_id: "t1".into(),
            is_acp: true,
        };
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["is_acp"], serde_json::Value::Bool(true));
        assert_eq!(v["agent_id"], "acp:codex/backend");
    }
}
