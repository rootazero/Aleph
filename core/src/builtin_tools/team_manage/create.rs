//! TeamCreateTool — create a new coordination team with sub-agent members.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::sub_agents::{SubAgentRegistry, SubAgentRun};
use crate::agents::swarm::tasks::{CoordTaskStore, NewTeam, TeamMember};
use crate::error::{AlephError, Result};
use crate::routing::SessionKey;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::launch::generate_team_id;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for creating a coordination team.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamCreateArgs {
    /// Human-readable team name
    pub name: String,
    /// Description of the team's purpose
    #[serde(default)]
    pub description: Option<String>,
    /// Agent ID of the team leader (defaults to current agent)
    #[serde(default)]
    pub leader: Option<String>,
    /// Team members to spawn as sub-agents
    #[serde(default)]
    pub members: Vec<TeamMemberSpec>,
}

/// Specification for a team member to be spawned
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamMemberSpec {
    /// Role name for this member (e.g., "code-reviewer", "tester")
    pub role: String,
    /// Persona prompt — defines the member's identity and expertise
    pub persona: String,
}

/// Output from team creation.
#[derive(Debug, Clone, Serialize)]
pub struct TeamCreateOutput {
    pub team_id: String,
    pub name: String,
    pub leader: String,
    pub members: Vec<SpawnedMember>,
    pub message: String,
}

/// A spawned sub-agent member
#[derive(Debug, Clone, Serialize)]
pub struct SpawnedMember {
    pub role: String,
    pub run_id: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that creates a new coordination team with sub-agent members.
#[derive(Clone)]
pub struct TeamCreateTool {
    store: Arc<dyn CoordTaskStore>,
    sub_registry: Arc<SubAgentRegistry>,
    current_agent_id: String,
    current_session_key: SessionKey,
}

impl TeamCreateTool {
    pub fn new(
        store: Arc<dyn CoordTaskStore>,
        sub_registry: Arc<SubAgentRegistry>,
        current_agent_id: String,
        current_session_key: SessionKey,
    ) -> Self {
        Self {
            store,
            sub_registry,
            current_agent_id,
            current_session_key,
        }
    }
}

#[async_trait]
impl AlephTool for TeamCreateTool {
    const NAME: &'static str = "team_create";
    const DESCRIPTION: &'static str =
        "Create a new coordination team. Members are spawned as sub-agents with \
         distinct personas. The team leader coordinates task execution.";

    type Args = TeamCreateArgs;
    type Output = TeamCreateOutput;

    fn examples(&self) -> Option<Vec<String>> {
        None
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let team_id = generate_team_id(&args.name);
        let leader = args
            .leader
            .unwrap_or_else(|| self.current_agent_id.clone());

        // Enforce member limit
        if args.members.len() > 8 {
            return Err(AlephError::Other {
                message: "Team cannot have more than 8 members".to_string(),
                suggestion: None,
            });
        }

        let new_team = NewTeam {
            id: team_id.clone(),
            name: args.name.clone(),
            description: args.description.unwrap_or_default(),
            leader: leader.clone(),
        };

        let team = self.store.create_team(new_team).await?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut spawned_members = Vec::new();

        for member_spec in &args.members {
            let subagent_id = format!("team-{}-{}", team_id, member_spec.role);
            let session_key = SessionKey::Subagent {
                parent_key: Box::new(self.current_session_key.clone()),
                subagent_id: subagent_id.clone(),
            };

            let run = SubAgentRun::new(
                session_key,
                self.current_session_key.clone(),
                format!("Team member: {}", member_spec.role),
                "team",
            )
            .with_persona(member_spec.persona.clone())
            .with_keep_alive(true)
            .with_label(format!("{}/{}", team_id, member_spec.role));

            let run_id: String = self.sub_registry.register(run).await?;

            let tm = TeamMember {
                agent_id: subagent_id,
                role: member_spec.role.clone(),
                joined_at: now,
                run_id: Some(run_id.clone()),
                persona: Some(member_spec.persona.clone()),
            };
            self.store.add_member(&team_id, tm).await?;

            spawned_members.push(SpawnedMember {
                role: member_spec.role.clone(),
                run_id,
            });
        }

        info!(
            team_id = %team.id,
            name = %team.name,
            members = spawned_members.len(),
            "Team created with sub-agent members"
        );

        Ok(TeamCreateOutput {
            message: format!(
                "Team '{}' created (id: {}) with {} sub-agent members",
                team.name,
                team.id,
                spawned_members.len()
            ),
            team_id: team.id,
            name: team.name,
            leader,
            members: spawned_members,
        })
    }
}
