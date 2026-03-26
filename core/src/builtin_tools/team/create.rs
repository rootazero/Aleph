//! TeamCreateTool — create a named team and enroll member agents.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::agent_manager::AgentManager;
use crate::config::types::agents_def::AgentDefinition;
use crate::error::{AlephError, Result};
use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
use crate::sync_primitives::Arc;
use crate::teams::{NewTeam, NewTeamMember, TeamStore};
use crate::tools::AlephTool;

// =============================================================================
// Inline agent creation spec
// =============================================================================

/// Specification for creating a new agent inline during team creation.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CreateAgentSpec {
    /// Unique agent ID (a-z, 0-9, _, -, max 64 chars)
    pub id: String,
    /// Human-readable display name (defaults to id)
    #[serde(default)]
    pub name: Option<String>,
    /// LLM model override (default: claude-sonnet-4-5)
    #[serde(default)]
    pub model: Option<String>,
    /// Custom system prompt written to SOUL.md
    #[serde(default)]
    pub profile: Option<String>,
    /// Brief description of what this agent specializes in
    #[serde(default)]
    pub identity: Option<String>,
}

// =============================================================================
// MemberSpec
// =============================================================================

/// Specification for a team member: either reference an existing agent or create one inline.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemberSpec {
    /// ID of an existing agent to add as a team member.
    /// Mutually exclusive with `create`.
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Inline spec for creating a new agent and immediately adding it.
    /// Mutually exclusive with `agent_id`.
    #[serde(default)]
    pub create: Option<CreateAgentSpec>,

    /// Role description for this member within the team (e.g. "researcher", "writer")
    #[serde(default)]
    pub role: String,
}

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for creating a team.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamCreateArgs {
    /// Human-readable name for the team
    pub name: String,

    /// Optional description of the team's purpose
    #[serde(default)]
    pub description: String,

    /// Members to enroll in the team.
    /// Each member either references an existing agent_id or provides a `create` spec.
    #[serde(default)]
    pub members: Vec<MemberSpec>,
}

/// Summary of an enrolled member returned in the output.
#[derive(Debug, Clone, Serialize)]
pub struct EnrolledMember {
    pub agent_id: String,
    pub role: String,
    /// true if the agent was created inline by this call
    pub created: bool,
}

/// Output from team_create.
#[derive(Debug, Clone, Serialize)]
pub struct TeamCreateOutput {
    pub team_id: String,
    pub name: String,
    pub leader_id: String,
    pub members: Vec<EnrolledMember>,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that creates a team of agents.
///
/// The calling agent is automatically set as the team leader.
/// Members can be existing agents (by agent_id) or new agents created inline.
#[derive(Clone)]
pub struct TeamCreateTool {
    store: Arc<dyn TeamStore>,
    registry: Arc<AgentRegistry>,
    agent_manager: Option<Arc<AgentManager>>,
    /// Injected by ExecutionEngine: the ID of the agent calling this tool.
    pub current_agent_id: String,
}

impl TeamCreateTool {
    pub fn new(
        store: Arc<dyn TeamStore>,
        registry: Arc<AgentRegistry>,
        agent_manager: Option<Arc<AgentManager>>,
        current_agent_id: impl Into<String>,
    ) -> Self {
        Self {
            store,
            registry,
            agent_manager,
            current_agent_id: current_agent_id.into(),
        }
    }

    /// Set the current agent ID (called by ExecutionEngine before each run).
    pub fn set_current_agent_id(&mut self, agent_id: impl Into<String>) {
        self.current_agent_id = agent_id.into();
    }

    /// Resolve a MemberSpec to an agent_id, creating the agent if needed.
    async fn resolve_member(&self, spec: &MemberSpec) -> Result<String> {
        if let Some(ref agent_id) = spec.agent_id {
            // Verify the existing agent is present in the runtime registry
            if self.registry.get(agent_id).await.is_none() {
                return Err(AlephError::other(format!(
                    "Agent '{}' not found in registry",
                    agent_id
                )));
            }
            return Ok(agent_id.clone());
        }

        if let Some(ref create_spec) = spec.create {
            return self.create_inline_agent(create_spec).await;
        }

        Err(AlephError::other(
            "MemberSpec must specify either agent_id or create",
        ))
    }

    /// Create a new agent from an inline CreateAgentSpec and register it.
    async fn create_inline_agent(&self, spec: &CreateAgentSpec) -> Result<String> {
        // Check for duplicates
        if self.registry.get(&spec.id).await.is_some() {
            return Err(AlephError::other(format!(
                "Agent '{}' already exists",
                spec.id
            )));
        }

        let agents_state_root = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".aleph/agents");
        let agent_state_dir = agents_state_root.join(&spec.id);

        let workspaces_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".aleph/workspaces");
        let workspace_path = workspaces_dir.join(&spec.id);

        let display_name = spec.name.as_deref().unwrap_or(&spec.id);

        // Initialize identity files
        crate::config::agent_resolver::initialize_agent_identity(&agent_state_dir, display_name)
            .map_err(|e| {
                AlephError::other(format!(
                    "Failed to initialize identity files for '{}': {}",
                    spec.id, e
                ))
            })?;

        crate::config::agent_resolver::initialize_agent_dir(&agent_state_dir).map_err(|e| {
            AlephError::other(format!(
                "Failed to initialize agent state dir for '{}': {}",
                spec.id, e
            ))
        })?;

        std::fs::create_dir_all(&workspace_path).map_err(|e| {
            AlephError::other(format!(
                "Failed to create workspace for '{}': {}",
                spec.id, e
            ))
        })?;

        // Write custom SOUL.md if profile provided
        if let Some(ref profile) = spec.profile {
            let soul_path = agent_state_dir.join("SOUL.md");
            std::fs::write(&soul_path, profile).map_err(|e| {
                AlephError::other(format!("Failed to write SOUL.md for '{}': {}", spec.id, e))
            })?;
        }

        // Create AgentInstance
        let model = spec.model.as_deref().unwrap_or("claude-sonnet-4-5");
        let config = AgentInstanceConfig {
            agent_id: spec.id.clone(),
            workspace: workspace_path.clone(),
            model: model.to_string(),
            system_prompt: spec.profile.clone(),
            agent_dir: agents_state_root.join(&spec.id),
            ..Default::default()
        };

        let instance = AgentInstance::new(config).map_err(|e| {
            AlephError::other(format!(
                "Failed to create agent instance '{}': {}",
                spec.id, e
            ))
        })?;

        // Register in runtime registry
        self.registry.register(instance).await;

        // Persist to TOML config (non-fatal)
        if let Some(ref manager) = self.agent_manager {
            let def = AgentDefinition {
                id: spec.id.clone(),
                name: spec.name.clone(),
                model: Some(model.to_string()),
                ..Default::default()
            };
            if let Err(e) = manager.create(def) {
                tracing::warn!(
                    agent_id = %spec.id,
                    error = %e,
                    "team_create: failed to persist inline agent to TOML (runtime registration succeeded)"
                );
            }
        }

        info!(agent_id = %spec.id, "team_create: created inline agent");
        Ok(spec.id.clone())
    }
}

#[async_trait]
impl AlephTool for TeamCreateTool {
    const NAME: &'static str = "team_create";
    const DESCRIPTION: &'static str =
        "Create a named team of agents. The calling agent becomes the team leader. \
        Members can be existing agents (by agent_id) or new agents created inline. \
        Returns the team ID and the list of enrolled members.";

    type Args = TeamCreateArgs;
    type Output = TeamCreateOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "team_create(name='Research Team', members=[{agent_id: 'researcher', role: 'lead'}])".to_string(),
            "team_create(name='Dev Squad', description='Backend development team', members=[\
                {agent_id: 'coder', role: 'developer'}, \
                {create: {id: 'reviewer', name: 'Code Reviewer'}, role: 'reviewer'}])".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let leader_id = self.current_agent_id.clone();

        // Resolve all members first (fail fast before creating the team)
        let mut resolved: Vec<(String, String, bool)> = Vec::new(); // (agent_id, role, created)
        for spec in &args.members {
            let created = spec.create.is_some();
            let agent_id = self.resolve_member(spec).await?;
            resolved.push((agent_id, spec.role.clone(), created));
        }

        // Create the team record
        let team = self
            .store
            .create_team(NewTeam {
                name: args.name.clone(),
                description: args.description.clone(),
                leader_id: leader_id.clone(),
            })
            .await?;

        info!(team_id = %team.id, leader = %leader_id, "team_create: team created");

        // Enroll members
        let mut enrolled: Vec<EnrolledMember> = Vec::new();
        for (agent_id, role, created) in resolved {
            self.store
                .add_member(NewTeamMember {
                    team_id: team.id.clone(),
                    agent_id: agent_id.clone(),
                    role: role.clone(),
                })
                .await?;

            info!(
                team_id = %team.id,
                agent_id = %agent_id,
                role = %role,
                "team_create: member enrolled"
            );

            enrolled.push(EnrolledMember {
                agent_id,
                role,
                created,
            });
        }

        let member_count = enrolled.len();
        Ok(TeamCreateOutput {
            team_id: team.id.clone(),
            name: team.name.clone(),
            leader_id: team.leader_id.clone(),
            members: enrolled,
            message: format!(
                "Team '{}' created (id: {}) with {} member(s).",
                team.name, team.id, member_count
            ),
        })
    }
}
