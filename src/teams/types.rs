//! Team management data types.
//!
//! Provides the foundational types for the team management system:
//! - `Team`: A named group of agents with a designated leader
//! - `TeamMember`: An agent's membership record within a team
//! - Store input/output types for CRUD operations
//!
//! Task tracking is handled by the unified `CoordTask` system
//! (see `agents::swarm::tasks`).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

pub type TeamId = String;

// ---------------------------------------------------------------------------
// TeamStatus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStatus {
    #[default]
    Active,
    Disbanded,
}

impl TeamStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disbanded => "disbanded",
        }
    }
}

impl std::str::FromStr for TeamStatus {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "disbanded" => Ok(Self::Disbanded),
            _ => Err(format!("unknown TeamStatus: {s}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Team
// ---------------------------------------------------------------------------

/// A named group of agents led by a designated leader agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: TeamId,
    pub name: String,
    pub description: String,
    pub leader_id: String,
    pub status: TeamStatus,
    pub created_at: i64,
    pub disbanded_at: Option<i64>,
    /// Free-form team operating protocol: role definitions, hand-off rules,
    /// and quality standards authored by the leader. When set, it is injected
    /// verbatim into every member's launch context by the dispatcher's
    /// handoff-context builder (see `teams::dispatcher::handoff`). `None` /
    /// empty means no protocol is in effect. Stored as an additive nullable
    /// column so older databases and serialized payloads stay compatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// The user who owns this team (P1 data isolation).
    ///
    /// `None` on rows created before P1 and on rows created outside any
    /// dispatch scope (cron, internal, tests) — adoption-by-absence, read
    /// through `gateway::visibility::owner_or_legacy` exactly like a session's
    /// own stamp, never re-derived here. Stored as an additive nullable column
    /// and skipped when absent so a legacy team serializes byte-identically to
    /// what it did before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
}

// ---------------------------------------------------------------------------
// TeamMemberKind
// ---------------------------------------------------------------------------

/// Discriminates the runtime backing of a team member.
///
/// `Agent` is the default in-process Aleph agent resolvable via the
/// agent registry. `AcpSession` represents an external coding CLI
/// (Claude Code, Codex, Gemini CLI, ...) attached via the ACP adapter
/// manager. Tasks owned by such a member are dispatched through
/// `AcpAdapterManager::prompt` instead of the in-process execution
/// adapter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TeamMemberKind {
    #[default]
    Agent,
    AcpSession,
}

impl TeamMemberKind {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::AcpSession => "acp_session",
        }
    }
}

impl std::str::FromStr for TeamMemberKind {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "agent" => Ok(Self::Agent),
            "acp_session" => Ok(Self::AcpSession),
            _ => Err(format!("unknown TeamMemberKind: {s}")),
        }
    }
}

// ---------------------------------------------------------------------------
// TeamMember
// ---------------------------------------------------------------------------

/// An agent's membership record within a team.
///
/// Field `agent_id` is the canonical handle used by the dispatcher.
/// - For `kind = Agent`, it matches an entry in the in-process agent registry.
/// - For `kind = AcpSession`, it is a synthetic id (`acp:<harness>:<cwd>[:name]`)
///   produced via [`acp_member_id`]; the structural fields
///   (`acp_harness_id`, `acp_cwd`, `acp_session_name`) carry the routing data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub team_id: TeamId,
    pub agent_id: String,
    pub role: String,
    pub joined_at: i64,
    #[serde(default)]
    pub kind: TeamMemberKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_harness_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_session_name: Option<String>,
}

/// Build the synthetic `agent_id` for an ACP-backed team member.
///
/// The id is stable for a given (harness, cwd, `session_name`) triple so the
/// dispatcher can reconcile a `CoordTask.owner` string back to a member row.
#[must_use]
pub fn acp_member_id(harness_id: &str, cwd: &str, session_name: Option<&str>) -> String {
    match session_name {
        Some(name) if !name.is_empty() => format!("acp:{harness_id}:{cwd}:{name}"),
        _ => format!("acp:{harness_id}:{cwd}"),
    }
}

// ---------------------------------------------------------------------------
// Input types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTeam {
    pub name: String,
    pub description: String,
    pub leader_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewTeamMember {
    pub team_id: TeamId,
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub kind: TeamMemberKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_harness_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_session_name: Option<String>,
}

impl NewTeamMember {
    /// Convenience constructor for an in-process agent member.
    pub fn for_agent(
        team_id: impl Into<TeamId>,
        agent_id: impl Into<String>,
        role: impl Into<String>,
    ) -> Self {
        Self {
            team_id: team_id.into(),
            agent_id: agent_id.into(),
            role: role.into(),
            kind: TeamMemberKind::Agent,
            ..Default::default()
        }
    }

    /// Convenience constructor for an ACP-backed external CLI member.
    ///
    /// `agent_id` is auto-derived via [`acp_member_id`] so the dispatcher
    /// can resolve `CoordTask.owner` back to a member row deterministically.
    pub fn for_acp_session(
        team_id: impl Into<TeamId>,
        harness_id: impl Into<String>,
        cwd: impl Into<String>,
        session_name: Option<String>,
        role: impl Into<String>,
    ) -> Self {
        let harness_id = harness_id.into();
        let cwd = cwd.into();
        let agent_id = acp_member_id(&harness_id, &cwd, session_name.as_deref());
        Self {
            team_id: team_id.into(),
            agent_id,
            role: role.into(),
            kind: TeamMemberKind::AcpSession,
            acp_harness_id: Some(harness_id),
            acp_cwd: Some(cwd),
            acp_session_name: session_name,
        }
    }
}

// ---------------------------------------------------------------------------
// TeamSummary
// ---------------------------------------------------------------------------

/// A lightweight summary of a team and its member count.
///
/// Per-team task counts are intentionally not included here: tasks live in the
/// `coord_tasks` DAG (a separate database from `teams.db`). Use the
/// `team_status` tool for an accurate task breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamSummary {
    pub id: TeamId,
    pub name: String,
    pub description: String,
    pub leader_id: String,
    pub status: TeamStatus,
    pub member_count: u64,
    pub created_at: i64,
    pub disbanded_at: Option<i64>,
    /// Owning user — the same stamp [`Team::owner_user_id`] carries, projected
    /// onto the summary so a list can be filtered in one query instead of
    /// re-fetching every team. Same adoption-by-absence rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
}
