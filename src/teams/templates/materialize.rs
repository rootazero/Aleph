//! Template materializer — turns a parsed [`TeamTemplate`] into a live team
//! with members enrolled and the initial task DAG persisted.
//!
//! Designed to be invokable from both the `team_from_template` builtin tool
//! and the gateway RPC handler with the same semantics.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;
use tracing::{info, warn};

use super::substitute::substitute;
use super::types::{TeamTemplate, TeamTemplateError, TemplateMember};
use crate::agents::swarm::tasks::{CoordTaskStore, NewCoordTask};
use crate::config::agent_manager::AgentManager;
use crate::config::types::agents_def::{AgentDefinition, AgentModelRef};
use crate::error::AlephError;
use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
use crate::gateway::session_store::SessionStore;
use crate::sync_primitives::Arc;
use crate::teams::dispatcher::schedule::{MANAGED_BY_DISPATCHER, MANAGED_BY_KEY};
use crate::teams::{NewTeam, NewTeamMember, TeamStore};

/// Convenient identifier for "the calling agent should become the leader".
pub const LEADER_SELF_ID: &str = "self";

/// Inputs for [`materialize_template`].
pub struct MaterializeRequest {
    /// The parsed template (already validated).
    pub template: TeamTemplate,
    /// Human-readable team name (substituted as `{team_name}`).
    pub team_name: String,
    /// Optional team description override (defaults to `template.description`).
    pub description: Option<String>,
    /// Goal text (substituted as `{goal}`); falls back to `template.default_goal`
    /// or `team_name` if neither is provided.
    pub goal: Option<String>,
    /// The agent calling the materializer. When `template.leader.id == "self"`
    /// this becomes the team leader.
    pub current_agent_id: String,
}

/// Successful materialization outcome.
#[derive(Debug, Clone, Serialize)]
pub struct MaterializedTeam {
    pub team_id: String,
    pub team_name: String,
    pub leader_id: String,
    /// Member agent ids that were enrolled (excluding leader).
    pub member_ids: Vec<String>,
    /// (`template_task_key` → `coord_task_id`) — useful for callers that want to
    /// surface a "what got scheduled" report.
    pub task_ids: Vec<(String, String)>,
    pub message: String,
}

/// Materializer execution context. Captures the shared dependencies so callers
/// (tool / RPC handler) can build it once and reuse.
#[derive(Clone)]
pub struct MaterializeDeps {
    pub team_store: Arc<dyn TeamStore>,
    pub coord_store: Arc<dyn CoordTaskStore>,
    pub registry: Arc<AgentRegistry>,
    pub agent_manager: Option<Arc<AgentManager>>,
    pub session_store: Arc<dyn SessionStore>,
}

/// Materialize a template end-to-end.
///
/// Non-transactional: if an inline-agent creation succeeds but a later step
/// fails, the agent remains registered without team membership. This matches
/// the documented limitation of `team_create` — orphaned agents can be cleaned
/// up via the agent management tools.
pub async fn materialize_template(
    deps: &MaterializeDeps,
    req: MaterializeRequest,
) -> Result<MaterializedTeam, TeamTemplateError> {
    let tpl = &req.template;
    let team_name = req.team_name.trim().to_string();
    if team_name.is_empty() {
        return Err(TeamTemplateError::Materialize(
            "team_name must not be empty".into(),
        ));
    }

    let goal = req
        .goal
        .as_deref()
        .or(tpl.default_goal.as_deref())
        .unwrap_or(team_name.as_str())
        .to_string();

    // Pre-build substitution map once; reused for every string field.
    let leader_label = tpl
        .leader
        .name
        .as_deref()
        .unwrap_or(tpl.leader.id.as_str())
        .to_string();
    let vars: HashMap<&str, &str> = HashMap::from([
        ("goal", goal.as_str()),
        ("team_name", team_name.as_str()),
        ("leader", leader_label.as_str()),
    ]);

    // --- 1. Resolve / create the leader ---------------------------------
    let leader_id = if tpl.leader.id == LEADER_SELF_ID {
        if req.current_agent_id.is_empty() {
            return Err(TeamTemplateError::Materialize(
                "template leader is `self` but current_agent_id is empty".into(),
            ));
        }
        // Inject the leader prompt addendum (if any) onto the caller's agent.
        if let Some(addendum) = &tpl.leader.prompt_addendum {
            let rendered = substitute(addendum, &vars);
            inject_role_prompt(deps, &req.current_agent_id, "leader", &rendered).await;
        }
        req.current_agent_id.clone()
    } else {
        // Treat a non-self leader spec like a member: lookup-or-create.
        let pseudo_member = TemplateMember {
            id: tpl.leader.id.clone(),
            name: tpl.leader.name.clone(),
            role: Some(tpl.leader.role.clone().unwrap_or_else(|| "leader".into())),
            model: tpl.leader.model.clone(),
            prompt_addendum: tpl.leader.prompt_addendum.clone(),
        };
        provision_member(deps, &pseudo_member, &req.current_agent_id, &vars).await?
    };

    // --- 2. Resolve / create each worker member -------------------------
    let mut enrolled_members: Vec<String> = Vec::with_capacity(tpl.members.len());
    for m in &tpl.members {
        let agent_id = provision_member(deps, m, &req.current_agent_id, &vars).await?;
        enrolled_members.push(agent_id);
    }

    // --- 3. Create team record -------------------------------------------
    let description = req
        .description
        .clone()
        .unwrap_or_else(|| substitute(&tpl.description, &vars));

    let team = deps
        .team_store
        .create_team(NewTeam {
            name: team_name.clone(),
            description,
            leader_id: leader_id.clone(),
        })
        .await
        .map_err(|e| TeamTemplateError::Materialize(format!("create_team failed: {e}")))?;

    info!(team_id = %team.id, template = %tpl.name, leader = %leader_id, "team_template: team created");

    // --- 4. Enroll leader + members --------------------------------------
    let leader_role = tpl.leader.role.clone().unwrap_or_else(|| "leader".into());
    deps.team_store
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: leader_id.clone(),
            role: leader_role,
            ..Default::default()
        })
        .await
        .map_err(|e| TeamTemplateError::Materialize(format!("add_member(leader) failed: {e}")))?;

    // --- 4b. R3 — inject team-level strategy prompt into all members ----
    // Strategy is a *team-wide* addendum that complements the per-member
    // role addendum. Pure prompt injection: dispatcher does not read it
    // (R7/R10 — intelligence lives in prompt). Idempotent: if SOUL.md
    // already contains `## Team Strategy`, we skip.
    if let Some(strategy) = tpl.strategy.as_deref().filter(|s| !s.trim().is_empty()) {
        let rendered = substitute(strategy, &vars);
        inject_strategy_prompt(deps, &leader_id, &rendered).await;
        for agent_id in &enrolled_members {
            inject_strategy_prompt(deps, agent_id, &rendered).await;
        }
    }

    for (m, agent_id) in tpl.members.iter().zip(enrolled_members.iter()) {
        let role = m.role.clone().unwrap_or_default();
        deps.team_store
            .add_member(NewTeamMember {
                team_id: team.id.clone(),
                agent_id: agent_id.clone(),
                role,
                ..Default::default()
            })
            .await
            .map_err(|e| TeamTemplateError::Materialize(format!("add_member failed: {e}")))?;
    }

    // --- 5. Create initial tasks with key→id mapping --------------------
    // Pass 1: create tasks in template order; collect (key, task_id).
    // Pass 2: would normally rewrite depends_on, but CoordTaskStore::create_task
    //         takes blocked_by upfront. So we sort topologically here and
    //         insert dependencies as we go (template was already validated as
    //         acyclic by `TeamTemplate::validate`).
    let topo = topo_sort(tpl)?;
    let mut key_to_id: HashMap<String, String> = HashMap::with_capacity(tpl.tasks.len());
    let mut task_ids: Vec<(String, String)> = Vec::with_capacity(tpl.tasks.len());

    for task in topo {
        let owner = if task.owner == tpl.leader.id {
            leader_id.clone()
        } else {
            task.owner.clone()
        };
        let blocked_by: Vec<String> = task
            .depends_on
            .iter()
            .filter_map(|k| key_to_id.get(k).cloned())
            .collect();

        let mut metadata = serde_json::json!({
            MANAGED_BY_KEY: MANAGED_BY_DISPATCHER,
            "template_name": tpl.name,
            "template_task_key": task.key,
        });
        // Convenience pointer to the parent team for downstream consumers.
        metadata["team_id"] = serde_json::Value::String(team.id.clone());

        let coord_task = deps
            .coord_store
            .create_task(NewCoordTask {
                team_id: Some(team.id.clone()),
                subject: substitute(&task.subject, &vars),
                description: substitute(&task.description, &vars),
                owner: Some(owner),
                priority: task.priority.as_coord_priority(),
                blocked_by,
                metadata,
            })
            .await
            .map_err(|e| {
                TeamTemplateError::Materialize(format!("create_task `{}` failed: {e}", task.key))
            })?;
        key_to_id.insert(task.key.clone(), coord_task.id.clone());
        task_ids.push((task.key.clone(), coord_task.id));
    }

    let message = format!(
        "Team '{}' (id: {}) materialized from template '{}': {} member(s), {} task(s).",
        team.name,
        team.id,
        tpl.name,
        enrolled_members.len() + 1,
        task_ids.len()
    );
    Ok(MaterializedTeam {
        team_id: team.id,
        team_name: team.name,
        leader_id,
        member_ids: enrolled_members,
        task_ids,
        message,
    })
}

// --------------------------------------------------------------------------
// Internal helpers
// --------------------------------------------------------------------------

/// Topologically order the template's tasks so dependencies are created first.
/// Returns the tasks in execution order. `TeamTemplate::validate` already
/// ruled out cycles, so this is Kahn's algorithm without the cycle check.
fn topo_sort(tpl: &TeamTemplate) -> Result<Vec<&super::types::TemplateTask>, TeamTemplateError> {
    // Build adjacency + in-degree maps keyed by task.key.
    let mut in_deg: HashMap<&str, usize> = HashMap::with_capacity(tpl.tasks.len());
    for t in &tpl.tasks {
        in_deg.insert(
            t.key.as_str(),
            t.depends_on
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
        );
    }
    let by_key: HashMap<&str, &super::types::TemplateTask> =
        tpl.tasks.iter().map(|t| (t.key.as_str(), t)).collect();

    let mut ready: Vec<&str> = in_deg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| *k)
        .collect();
    // Determinism: sort ready set by task position in source order.
    let pos: HashMap<&str, usize> = tpl
        .tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.key.as_str(), i))
        .collect();
    ready.sort_by_key(|k| pos.get(*k).copied().unwrap_or(0));

    let mut out: Vec<&super::types::TemplateTask> = Vec::with_capacity(tpl.tasks.len());
    let mut newly_ready: Vec<&str> = Vec::new();
    while let Some(k) = ready.pop() {
        out.push(by_key[k]);
        // Decrement in-degree for everyone depending on `k`.
        newly_ready.clear();
        for other in &tpl.tasks {
            if other.depends_on.iter().any(|d| d == k) {
                let entry = in_deg.get_mut(other.key.as_str()).ok_or_else(|| {
                    TeamTemplateError::Materialize(format!(
                        "topo sort: unknown dependency key '{}' for task '{}'",
                        k, other.key
                    ))
                })?;
                *entry -= 1;
                if *entry == 0 {
                    newly_ready.push(other.key.as_str());
                }
            }
        }
        newly_ready.sort_by_key(|k| std::cmp::Reverse(pos.get(*k).copied().unwrap_or(0)));
        ready.append(&mut newly_ready);
    }

    if out.len() != tpl.tasks.len() {
        // Defensive — should be caught by validate(); surface a clearer error.
        return Err(TeamTemplateError::Materialize(format!(
            "topo sort produced {} tasks but template has {} (cycle?)",
            out.len(),
            tpl.tasks.len()
        )));
    }
    Ok(out)
}

/// Look up an agent by id; if missing, create it inline with the given
/// prompt addendum.
async fn provision_member(
    deps: &MaterializeDeps,
    member: &TemplateMember,
    caller_agent_id: &str,
    vars: &HashMap<&str, &str>,
) -> Result<String, TeamTemplateError> {
    // Reuse existing agent when present.
    if deps.registry.get(&member.id).await.is_some() {
        if let Some(addendum) = &member.prompt_addendum {
            let rendered = substitute(addendum, vars);
            let role = member.role.as_deref().unwrap_or("worker");
            inject_role_prompt(deps, &member.id, role, &rendered).await;
        }
        return Ok(member.id.clone());
    }

    // Create inline. Mirrors team_create::create_inline_agent's I/O steps but
    // takes addendum text directly rather than a role name.
    crate::builtin_tools::agent_manage::create::validate_agent_id(&member.id).map_err(|e| {
        TeamTemplateError::Materialize(format!("invalid member id `{}`: {e}", member.id))
    })?;

    let agents_root = aleph_home().join("agents");
    let agent_state_dir = agents_root.join(&member.id);

    let workspaces_root = aleph_home().join("workspaces");
    let workspace_path = workspaces_root.join(&member.id);

    let display_name = member.name.as_deref().unwrap_or(&member.id);

    crate::config::agent_resolver::initialize_agent_identity(
        &agent_state_dir,
        display_name,
        crate::thinker::soul_archetypes::SoulArchetype::default(),
    )
    .map_err(|e| {
        TeamTemplateError::Materialize(format!(
            "initialize_agent_identity for `{}` failed: {e}",
            member.id
        ))
    })?;

    crate::config::agent_resolver::initialize_agent_dir(&agent_state_dir).map_err(|e| {
        TeamTemplateError::Materialize(format!(
            "initialize_agent_dir for `{}` failed: {e}",
            member.id
        ))
    })?;

    tokio::fs::create_dir_all(&workspace_path)
        .await
        .map_err(|e| {
            TeamTemplateError::Materialize(format!(
                "create workspace for `{}` failed: {e}",
                member.id
            ))
        })?;

    // Resolve the model: explicit override → caller's model → default.
    let caller_model = deps
        .registry
        .get(caller_agent_id)
        .await
        .map(|inst| inst.config().model.clone());
    let model = member
        .model
        .clone()
        .or(caller_model)
        .unwrap_or_else(|| "claude-sonnet-4-5".to_string());

    // Compose the SOUL.md body. The role header makes the addendum locatable
    // when humans inspect the file later.
    let body = match &member.prompt_addendum {
        Some(addendum) => {
            let role = member.role.as_deref().unwrap_or("worker");
            format!("## Team Role ({role})\n\n{}\n", substitute(addendum, vars))
        }
        None => String::new(),
    };

    if !body.is_empty() {
        let soul_path = agent_state_dir.join("SOUL.md");
        tokio::fs::write(&soul_path, &body).await.map_err(|e| {
            TeamTemplateError::Materialize(format!("write SOUL.md for `{}` failed: {e}", member.id))
        })?;
    }

    let system_prompt = if body.is_empty() { None } else { Some(body) };

    let config = AgentInstanceConfig {
        agent_id: member.id.clone(),
        workspace: workspace_path,
        model: model.clone(),
        system_prompt,
        agent_dir: agent_state_dir,
        ..Default::default()
    };

    let instance = AgentInstance::new(config, Arc::clone(&deps.session_store)).map_err(|e| {
        TeamTemplateError::Materialize(format!(
            "AgentInstance::new for `{}` failed: {e}",
            member.id
        ))
    })?;

    deps.registry.register(instance).await;

    // Best-effort persistence to the TOML registry.
    if let Some(manager) = &deps.agent_manager {
        let def = AgentDefinition {
            id: member.id.clone(),
            name: member.name.clone(),
            model: Some(AgentModelRef::Legacy(model)),
            ..Default::default()
        };
        if let Err(e) = manager.create(def) {
            warn!(
                agent_id = %member.id,
                error = %e,
                "team_template: failed to persist inline agent to TOML (runtime registration succeeded)"
            );
        }
    }

    info!(agent_id = %member.id, "team_template: created inline agent");
    Ok(member.id.clone())
}

/// Append a team-strategy section to an existing agent's SOUL.md,
/// idempotently. R3 (`ClawTeam` parity): team-scope counterpart of
/// [`inject_role_prompt`]. Same I/O pattern; different section heading
/// so the two can coexist in one SOUL.md without ambiguity.
async fn inject_strategy_prompt(deps: &MaterializeDeps, agent_id: &str, body: &str) {
    let Some(instance) = deps.registry.get(agent_id).await else {
        return;
    };
    let soul_path = instance.agent_dir().join("SOUL.md");
    let section = format!("\n\n---\n\n## Team Strategy\n\n{body}\n");
    let result = if soul_path.exists() {
        // Read the existing SOUL.md; on a read error we must NOT fall back to an
        // empty string, because the subsequent write would then truncate the
        // file to just `section`, silently destroying the agent's persona.
        let existing = match tokio::fs::read_to_string(&soul_path).await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    path = %soul_path.display(),
                    error = %e,
                    "team_template: failed to read SOUL.md, skipping strategy injection"
                );
                return;
            }
        };
        if existing.contains("## Team Strategy") {
            return;
        }
        tokio::fs::write(&soul_path, format!("{existing}{section}")).await
    } else {
        tokio::fs::write(&soul_path, section.trim_start_matches('\n')).await
    };
    if let Err(e) = result {
        warn!(
            path = %soul_path.display(),
            error = %e,
            "team_template: failed to inject strategy prompt into SOUL.md"
        );
    }
}

/// Append a role prompt section to an existing agent's SOUL.md, idempotently.
async fn inject_role_prompt(deps: &MaterializeDeps, agent_id: &str, role: &str, body: &str) {
    let Some(instance) = deps.registry.get(agent_id).await else {
        return;
    };
    let soul_path = instance.agent_dir().join("SOUL.md");
    let section = format!("\n\n---\n\n## Team Role ({role})\n\n{body}\n");

    let result = if soul_path.exists() {
        // See `inject_strategy_prompt`: a read error must skip the injection,
        // never `unwrap_or_default()` — an empty fallback would truncate the
        // existing SOUL.md on the following write.
        let existing = match tokio::fs::read_to_string(&soul_path).await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    path = %soul_path.display(),
                    error = %e,
                    "team_template: failed to read SOUL.md, skipping role injection"
                );
                return;
            }
        };
        if existing.contains(&format!("## Team Role ({role})")) {
            return;
        }
        tokio::fs::write(&soul_path, format!("{existing}{section}")).await
    } else {
        tokio::fs::write(&soul_path, section.trim_start_matches('\n')).await
    };

    if let Err(e) = result {
        warn!(
            path = %soul_path.display(),
            error = %e,
            "team_template: failed to inject role prompt into SOUL.md"
        );
    }
}

fn aleph_home() -> PathBuf {
    std::env::var_os("ALEPH_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".aleph")))
        .unwrap_or_else(|| PathBuf::from("."))
}

// Errors thrown by helpers wrap `AlephError` only via `Materialize` so the
// caller sees one error variant per failure surface.
impl From<AlephError> for TeamTemplateError {
    fn from(e: AlephError) -> Self {
        Self::Materialize(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::templates::types::{
        TeamTemplate, TemplateLeader, TemplateMember, TemplatePriority, TemplateTask,
    };

    fn dag_template() -> TeamTemplate {
        TeamTemplate {
            name: "t".into(),
            description: "{goal} demo".into(),
            default_goal: Some("default goal".into()),
            strategy: None,
            leader: TemplateLeader {
                id: LEADER_SELF_ID.into(),
                name: Some("Lead".into()),
                role: None,
                model: None,
                prompt_addendum: None,
            },
            members: vec![TemplateMember {
                id: "w".into(),
                name: None,
                role: Some("worker".into()),
                model: None,
                prompt_addendum: None,
            }],
            tasks: vec![
                TemplateTask {
                    key: "a".into(),
                    subject: "A {goal}".into(),
                    description: String::new(),
                    owner: LEADER_SELF_ID.into(),
                    priority: TemplatePriority::High,
                    depends_on: vec![],
                },
                TemplateTask {
                    key: "b".into(),
                    subject: "B".into(),
                    description: String::new(),
                    owner: "w".into(),
                    priority: TemplatePriority::default(),
                    depends_on: vec!["a".into()],
                },
                TemplateTask {
                    key: "c".into(),
                    subject: "C".into(),
                    description: String::new(),
                    owner: "w".into(),
                    priority: TemplatePriority::default(),
                    depends_on: vec!["a".into()],
                },
            ],
        }
    }

    #[test]
    fn topo_sort_respects_dependencies() {
        let tpl = dag_template();
        let order = topo_sort(&tpl).expect("ok");
        let keys: Vec<&str> = order.iter().map(|t| t.key.as_str()).collect();
        // `a` must precede `b` and `c`; relative order of `b`/`c` is implementation-defined.
        let a_idx = keys.iter().position(|k| *k == "a").unwrap();
        let b_idx = keys.iter().position(|k| *k == "b").unwrap();
        let c_idx = keys.iter().position(|k| *k == "c").unwrap();
        assert!(a_idx < b_idx);
        assert!(a_idx < c_idx);
        assert_eq!(order.len(), 3);
    }
}
