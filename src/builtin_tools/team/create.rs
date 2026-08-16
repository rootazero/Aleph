//! `TeamCreateTool` — create a named team and enroll member agents.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::builtin_tools::agent_manage::create::validate_agent_id;
use crate::config::agent_manager::AgentManager;
use crate::error::{AlephError, Result};
use crate::gateway::agent_instance::AgentRegistry;
use crate::sync_primitives::Arc;
use crate::teams::member_provision::{
    register_member_agent, validate_toolset, MemberContract, MemberToolset, ProvisionDeps,
    ProvisionRequest,
};
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
    /// Team role for this agent. When set, the corresponding role prompt
    /// template is automatically appended to the agent's system prompt.
    #[serde(default)]
    pub role: Option<String>,
    /// Tools this member may call. Omit (the default) to give it every tool,
    /// which is how members behaved before this field existed. Entries support
    /// a trailing-`*` prefix glob, so `["task_*", "team_*", "message_send",
    /// "search"]` is a research member that cannot run shell commands or edit
    /// files. A declared list must still admit the hand-off verbs the member's
    /// launch prompt instructs it to call (`task_submit`, `message_send`) —
    /// creation fails with the missing names otherwise.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Tools explicitly withheld from this member, applied over `tools`.
    /// Use this to subtract from the full surface (`["bash", "code_exec"]`)
    /// without having to enumerate everything it keeps.
    #[serde(default)]
    pub tools_denied: Option<Vec<String>>,
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
    /// Each member either references an existing `agent_id` or provides a `create` spec.
    #[serde(default)]
    pub members: Vec<MemberSpec>,

    /// Optional team operating protocol: role definitions, hand-off rules, and
    /// quality standards. When set, it is injected verbatim into every member's
    /// launch context, so the whole team works under one shared agreement.
    /// Amend it later with `team_set_protocol`.
    #[serde(default)]
    pub protocol: Option<String>,
}

/// Summary of an enrolled member returned in the output.
#[derive(Debug, Clone, Serialize)]
pub struct EnrolledMember {
    pub agent_id: String,
    pub role: String,
    /// true if the agent was created inline by this call
    pub created: bool,
}

/// Output from `team_create`.
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
/// Members can be existing agents (by `agent_id`) or new agents created inline.
#[derive(Clone)]
pub struct TeamCreateTool {
    store: Arc<dyn TeamStore>,
    registry: Arc<AgentRegistry>,
    agent_manager: Option<Arc<AgentManager>>,
    session_store: Arc<dyn crate::gateway::session_store::SessionStore>,
    /// Injected by `ExecutionEngine`: the ID of the agent calling this tool.
    pub current_agent_id: String,
}

impl TeamCreateTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    pub fn new(
        store: Arc<dyn TeamStore>,
        registry: Arc<AgentRegistry>,
        agent_manager: Option<Arc<AgentManager>>,
        session_store: Arc<dyn crate::gateway::session_store::SessionStore>,
        current_agent_id: impl Into<String>,
    ) -> Self {
        Self {
            store,
            registry,
            agent_manager,
            session_store,
            current_agent_id: current_agent_id.into(),
        }
    }

    /// Look up a built-in role prompt template by name.
    fn builtin_role_prompt(role: &str) -> Option<&'static str> {
        const LEADER_PROMPT: &str = include_str!("../../agents/prompts/team_leader.md");
        const WORKER_PROMPT: &str = include_str!("../../agents/prompts/team_worker.md");

        match role {
            "leader" => Some(LEADER_PROMPT),
            "worker" => Some(WORKER_PROMPT),
            _ => None,
        }
    }

    /// Resolve a `MemberSpec` to an `agent_id`, creating the agent if needed.
    /// When `role` is set on the spec, the corresponding role prompt
    /// template is injected into the agent's system prompt.
    async fn resolve_member(&self, spec: &MemberSpec) -> Result<String> {
        if let Some(ref agent_id) = spec.agent_id {
            // ACP harness members (`acp:claude-code`, `acp:codex/backend`, …)
            // are external CLI processes routed via `AcpAdapterManager` — they
            // do not have an `AgentInstance` in the registry. Accept the ID
            // as-is; the dispatcher's ACP bridge resolves it at run time.
            if crate::teams::dispatcher::AcpMemberRef::parse(agent_id).is_some() {
                return Ok(agent_id.clone());
            }
            // `acp:`-prefixed but unparseable = a long-form roster/display id
            // (harness ids never contain ':') — steer to the explicit-cwd
            // tool rather than falling through to the registry not-found.
            if agent_id.starts_with("acp:") {
                return Err(AlephError::other(format!(
                    "'{agent_id}' is not a valid ACP reference: harness ids \
                     never contain ':' (long-form roster ids are display-only). \
                     Use the short form 'acp:<harness>[/<session>]', or add the \
                     member after create via team_acp_member(harness_id=…, cwd=…)"
                )));
            }

            let instance = self.registry.get(agent_id).await.ok_or_else(|| {
                AlephError::other(format!("Agent '{agent_id}' not found in registry"))
            })?;

            if !spec.role.is_empty() {
                if let Some(template) = Self::builtin_role_prompt(&spec.role) {
                    Self::append_role_prompt_to_soul(instance.agent_dir(), template).await;
                }
            }

            return Ok(agent_id.clone());
        }

        if let Some(ref create_spec) = spec.create {
            let leader_model = self.registry.get(&self.actor()).await.map_or_else(
                || "claude-sonnet-4-5".to_string(),
                |inst| inst.config().model.clone(),
            );
            return self
                .create_inline_agent(create_spec, &spec.role, &leader_model)
                .await;
        }

        Err(AlephError::other(
            "MemberSpec must specify either agent_id or create",
        ))
    }

    /// Append a role prompt template section to the agent's SOUL.md.
    /// Non-fatal: logs a warning on I/O failure.
    async fn append_role_prompt_to_soul(agent_dir: &std::path::Path, template: &str) {
        let soul_path = agent_dir.join("SOUL.md");
        let section = format!("\n\n---\n\n## Team Role\n\n{template}");

        let result = if soul_path.exists() {
            let existing = tokio::fs::read_to_string(&soul_path)
                .await
                .unwrap_or_default();
            // Avoid double-injection
            if existing.contains("## Team Role") {
                return;
            }
            tokio::fs::write(&soul_path, format!("{existing}{section}")).await
        } else {
            tokio::fs::write(&soul_path, section.trim_start()).await
        };

        if let Err(e) = result {
            tracing::warn!(
                path = %soul_path.display(),
                error = %e,
                "team_create: failed to append role prompt to SOUL.md"
            );
        }
    }

    /// Create a new agent from an inline `CreateAgentSpec` and register it.
    /// If an agent with the same ID already exists, silently reuse it
    /// instead of creating a duplicate (prevents registry pollution).
    ///
    /// When `role` is provided, the matching role prompt template is appended
    /// to the agent's system prompt.
    async fn create_inline_agent(
        &self,
        spec: &CreateAgentSpec,
        role: &str,
        leader_model: &str,
    ) -> Result<String> {
        validate_agent_id(&spec.id)
            .map_err(|e| AlephError::other(format!("Invalid agent ID '{}': {}", spec.id, e)))?;

        if self.registry.get(&spec.id).await.is_some() {
            info!(agent_id = %spec.id, "team_create: reusing existing agent");
            return Ok(spec.id.clone());
        }

        // Inline members always run under the worker contract — `team_create`
        // makes the *calling* agent the leader, so a member created here never
        // gets the orchestration prompt. Checked before any directory is
        // created so a stranding declaration leaves nothing behind.
        let toolset = MemberToolset::new(spec.tools.clone(), spec.tools_denied.clone());
        validate_toolset(&toolset, MemberContract::Worker).map_err(|missing| {
            AlephError::other(format!(
                "Agent '{}' declares `tools` that hide {}, which its team launch \
                 prompt instructs it to call. Add them (a `task_*` glob works) or \
                 omit `tools` to keep the full surface.",
                spec.id,
                missing.join(", ")
            ))
        })?;

        // Both roots come from the manager boot resolved them into, which is
        // the same rule `agent_resolver` rebuilds this member with after a
        // restart (`member_provision`'s module doc spells that dependency
        // out). Resolving them here by hand meant provisioning wrote identity
        // files and a workspace where the resolver would not look — first
        // because the hand-rolled path ignored `ALEPH_HOME`, then because it
        // ignored `[agents.defaults] agents_root / workspace_root`.
        let roots = crate::config::agent_manager::provisioning_roots(self.agent_manager.as_deref());
        let agent_state_dir = roots.agents.join(&spec.id);
        let workspace_path = roots.workspaces.join(&spec.id);

        let display_name = spec.name.as_deref().unwrap_or(&spec.id);

        // Initialize identity files
        crate::config::agent_resolver::initialize_agent_identity(
            &agent_state_dir,
            display_name,
            crate::thinker::soul_archetypes::SoulArchetype::default(),
        )
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

        tokio::fs::create_dir_all(&workspace_path)
            .await
            .map_err(|e| {
                AlephError::other(format!(
                    "Failed to create workspace for '{}': {}",
                    spec.id, e
                ))
            })?;

        // Determine the effective role: prefer spec.role if present, fall back
        // to the function parameter.
        let effective_role = spec.role.as_deref().unwrap_or(role);
        let role_template = if effective_role.is_empty() {
            None
        } else {
            Self::builtin_role_prompt(effective_role)
        };

        let combined_prompt = match (&spec.profile, role_template) {
            (Some(profile), Some(tpl)) => {
                Some(format!("{profile}\n\n---\n\n## Team Role\n\n{tpl}"))
            }
            (Some(profile), None) => Some(profile.clone()),
            (None, Some(tpl)) => Some(format!("## Team Role\n\n{tpl}")),
            (None, None) => None,
        };

        // Write SOUL.md with combined prompt
        if let Some(ref prompt) = combined_prompt {
            let soul_path = agent_state_dir.join("SOUL.md");
            tokio::fs::write(&soul_path, prompt).await.map_err(|e| {
                AlephError::other(format!("Failed to write SOUL.md for '{}': {}", spec.id, e))
            })?;
        }

        let model = spec.model.as_deref().unwrap_or(leader_model);
        register_member_agent(
            ProvisionRequest {
                agent_id: spec.id.clone(),
                display_name: spec.name.clone(),
                workspace: workspace_path,
                agent_dir: agent_state_dir.clone(),
                model: model.to_string(),
                system_prompt: combined_prompt,
                toolset,
            },
            ProvisionDeps {
                registry: &self.registry,
                session_store: &self.session_store,
                agent_manager: self.agent_manager.as_ref(),
            },
        )
        .await
        .map_err(AlephError::other)?;

        Ok(spec.id.clone())
    }
}

#[async_trait]
impl AlephTool for TeamCreateTool {
    const NAME: &'static str = "team_create";
    const DESCRIPTION: &'static str =
        "Create a named team of agents. The calling agent becomes the team leader. \
        Members can be existing agents (by agent_id) or new agents created inline. \
        IMPORTANT: Before creating new agents, prefer reusing existing agents by agent_id \
        when their capabilities match the required role. Only create new agents when no \
        suitable existing agent is available. \
        Returns the team ID and the list of enrolled members.";

    type Args = TeamCreateArgs;
    type Output = TeamCreateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let leader_id = self.actor();

        // Inject leader role prompt for the calling agent
        if let Some(instance) = self.registry.get(&leader_id).await {
            if let Some(template) = Self::builtin_role_prompt("leader") {
                Self::append_role_prompt_to_soul(instance.agent_dir(), template).await;
            }
        }

        // Resolve all members first (fail fast before creating the team).
        // NOTE: Inline agent creation is not atomic with team creation. If an
        // inline agent is successfully created but a subsequent step fails
        // (e.g., later member resolution or team record creation), the agent
        // will remain registered without being part of a team. This is an
        // accepted limitation — the spec does not require transactional
        // rollback, and the orphaned agent can be cleaned up manually.
        let mut resolved: Vec<(String, String, bool)> = Vec::new(); // (agent_id, role, created)
        for spec in &args.members {
            let created = spec.create.is_some();
            let agent_id = self.resolve_member(spec).await?;
            resolved.push((agent_id, spec.role.clone(), created));
        }

        // Reject duplicate team names up front. `get_team_by_name` resolves by
        // name via query_row (first match wins), so a second team sharing a name
        // would be permanently shadowed from every name-based lookup. Fail fast
        // instead of silently creating an unreachable duplicate.
        if let Some(existing) = self.store.get_team_by_name(&args.name).await? {
            return Err(AlephError::invalid_input(format!(
                "a team named '{}' already exists (id {})",
                args.name, existing.id
            )));
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

        // Apply the operating protocol if one was supplied. Non-fatal: a failed
        // protocol write must not orphan an already-created team — log and go on.
        if let Some(proto) = args.protocol.clone() {
            if !proto.trim().is_empty() {
                if let Err(e) = self.store.set_protocol(&team.id, Some(proto)).await {
                    tracing::warn!(team_id = %team.id, error = %e, "team_create: failed to set protocol");
                } else {
                    info!(team_id = %team.id, "team_create: protocol applied");
                }
            }
        }

        // Auto-enroll the leader as a member (role = "leader")
        self.store
            .add_member(NewTeamMember {
                team_id: team.id.clone(),
                agent_id: leader_id.clone(),
                role: "leader".to_string(),
                ..Default::default()
            })
            .await?;

        // Enroll members
        let mut enrolled: Vec<EnrolledMember> = Vec::new();
        for (agent_id, role, created) in resolved {
            // ACP harness references must be persisted with the AcpSession
            // kind and routing fields (mirroring `team_member_add`) — the
            // default Agent kind would make the dispatcher treat "acp:…" as
            // an in-process agent and fail every dispatched task.
            let new_member = if let Some(member_ref) =
                crate::teams::dispatcher::AcpMemberRef::parse(&agent_id)
            {
                NewTeamMember {
                    team_id: team.id.clone(),
                    agent_id: agent_id.clone(),
                    role: role.clone(),
                    kind: crate::teams::types::TeamMemberKind::AcpSession,
                    acp_harness_id: Some(member_ref.harness_id.clone()),
                    // No cwd in the `acp:` ref; the dispatcher force-fails an
                    // ACP member with no cwd, so default to the active project
                    // root (then the server cwd) to keep it dispatchable.
                    acp_cwd: Some(super::acp_default_cwd()),
                    acp_session_name: member_ref.session_name.clone(),
                }
            } else {
                NewTeamMember {
                    team_id: team.id.clone(),
                    agent_id: agent_id.clone(),
                    role: role.clone(),
                    ..Default::default()
                }
            };
            self.store.add_member(new_member).await?;

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
