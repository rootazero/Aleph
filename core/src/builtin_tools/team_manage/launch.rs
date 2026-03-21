//! TeamLaunchTool — launch a team from a template file.

use std::collections::HashMap;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::agents::swarm::tasks::template::{self, TeamTemplate};
use crate::agents::swarm::tasks::{
    CoordTaskId, CoordTaskStore, NewCoordTask, NewTeam, Priority, TeamMember, TeamStatus,
    TeamUpdate,
};
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Helpers
// =============================================================================

/// Generate a slug-style team ID from a display name.
///
/// ASCII names are slugified ("Code Review Team" -> "code-review-team").
/// Non-ASCII names fall back to a deterministic hash ("代码审查" -> "team-a1b2c3d4").
pub fn generate_team_id(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c == ' ' || c == '-' || c == '_' {
                '-'
            } else {
                '\0'
            }
        })
        .filter(|&c| c != '\0')
        .collect();

    // Clean up consecutive hyphens
    let slug: String = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if slug.len() >= 2 {
        return slug;
    }

    // Fallback: deterministic hash
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    format!("team-{:08x}", hasher.finish() as u32)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for launching a team from a template.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamLaunchArgs {
    /// Template name (looked up in ~/.aleph/templates/{name}.md)
    pub template: String,
    /// Override the team name from the template
    #[serde(default)]
    pub name: Option<String>,
    /// Variable substitutions for the template (key -> value)
    #[serde(default)]
    pub variables: Option<HashMap<String, String>>,
}

/// Summary of a task created during launch.
#[derive(Debug, Clone, Serialize)]
pub struct LaunchedTask {
    pub task_id: String,
    pub key: String,
    pub subject: String,
    pub owner: String,
    pub blocked_by: Vec<String>,
}

/// Output from team launch.
#[derive(Debug, Clone, Serialize)]
pub struct TeamLaunchOutput {
    pub team_id: String,
    pub name: String,
    pub leader: String,
    pub member_count: usize,
    pub tasks: Vec<LaunchedTask>,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that launches a full team from a template file, creating the team,
/// registering members, and instantiating the task DAG.
#[derive(Clone)]
pub struct TeamLaunchTool {
    store: Arc<dyn CoordTaskStore>,
}

impl TeamLaunchTool {
    pub fn new(store: Arc<dyn CoordTaskStore>) -> Self {
        Self { store }
    }
}

/// Attempt to clean up a team by marking it as disbanded.
/// Logs errors but never propagates them.
async fn cleanup_team(store: &dyn CoordTaskStore, team_id: &str) {
    let update = TeamUpdate {
        status: Some(TeamStatus::Disbanded),
    };
    if let Err(e) = store.update_team(team_id, update).await {
        warn!(team_id = %team_id, error = %e, "Failed to clean up team after launch failure");
    }
}

/// Build the task DAG from a parsed template, creating tasks in the store.
async fn create_tasks_from_template(
    store: &dyn CoordTaskStore,
    team_id: &str,
    tmpl: &TeamTemplate,
    variables: &HashMap<String, String>,
) -> Result<Vec<LaunchedTask>> {
    // Map template task key -> created CoordTaskId
    let mut key_to_id: HashMap<String, CoordTaskId> = HashMap::new();
    let mut launched = Vec::new();

    for task in &tmpl.tasks {
        // Resolve blocked_by keys to actual CoordTaskIds
        let mut resolved_deps: Vec<CoordTaskId> = Vec::new();
        for dep_key in &task.blocked_by {
            match key_to_id.get(dep_key) {
                Some(id) => resolved_deps.push(id.clone()),
                None => {
                    return Err(AlephError::Other {
                        message: format!(
                            "Task '{}' blocked_by '{}' but that task hasn't been created yet \
                             (check task ordering in template)",
                            task.key, dep_key
                        ),
                        suggestion: Some(
                            "Ensure blocked_by references only tasks defined earlier in the template"
                                .to_string(),
                        ),
                    });
                }
            }
        }

        let priority = task
            .priority
            .as_deref()
            .and_then(Priority::from_stored)
            .unwrap_or_default();

        let subject = template::substitute_variables(&task.subject, variables);

        let new_task = NewCoordTask {
            team_id: Some(team_id.to_string()),
            subject,
            description: String::new(),
            owner: Some(task.owner.clone()),
            priority,
            blocked_by: resolved_deps,
            metadata: serde_json::Value::Null,
        };

        let created = store.create_task(new_task).await?;
        key_to_id.insert(task.key.clone(), created.id.clone());

        launched.push(LaunchedTask {
            task_id: created.id,
            key: task.key.clone(),
            subject: created.subject,
            owner: task.owner.clone(),
            blocked_by: task.blocked_by.clone(),
        });
    }

    Ok(launched)
}

#[async_trait]
impl AlephTool for TeamLaunchTool {
    const NAME: &'static str = "team_launch";
    const DESCRIPTION: &'static str =
        "Launch a full team from a template file. Reads a template from \
         ~/.aleph/templates/, creates the team, registers members, and \
         instantiates the task dependency graph. Use 'variables' to customize \
         template placeholders.";

    type Args = TeamLaunchArgs;
    type Output = TeamLaunchOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"team_launch(template="code-review-team", variables={"goal": "Review PR #42"})"#
                .to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // 1. Find and parse template
        let content = template::find_template(&args.template)?;
        let parsed = template::parse_template(&content)?;
        template::validate_template(&parsed.frontmatter)?;

        let tmpl = &parsed.frontmatter;

        // 2. Build variable map (built-in + user)
        let mut variables = args.variables.unwrap_or_default();
        variables
            .entry("team_name".to_string())
            .or_insert_with(|| tmpl.name.clone());
        variables
            .entry("leader_name".to_string())
            .or_insert_with(|| tmpl.leader.clone());

        // 3. Generate team ID and create team
        let team_name = args.name.unwrap_or_else(|| tmpl.name.clone());
        let team_id = generate_team_id(&team_name);

        let new_team = NewTeam {
            id: team_id.clone(),
            name: team_name.clone(),
            description: template::substitute_variables(&tmpl.description, &variables),
            leader: tmpl.leader.clone(),
        };

        let team = self.store.create_team(new_team).await?;

        // 4. Register members
        let now = now_secs();
        for member in &tmpl.members {
            let tm = TeamMember {
                agent_id: member.name.clone(),
                role: member.role.clone(),
                joined_at: now,
            };
            if let Err(e) = self.store.add_member(&team_id, tm).await {
                warn!(member = %member.name, error = %e, "Failed to add member, cleaning up");
                cleanup_team(self.store.as_ref(), &team_id).await;
                return Err(e);
            }
        }

        // 5. Create task DAG
        let tasks = match create_tasks_from_template(
            self.store.as_ref(),
            &team_id,
            tmpl,
            &variables,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                cleanup_team(self.store.as_ref(), &team_id).await;
                return Err(e);
            }
        };

        let member_count = team.members.len();
        let task_count = tasks.len();

        info!(
            team_id = %team.id,
            name = %team.name,
            members = member_count,
            tasks = task_count,
            "Team launched from template"
        );

        Ok(TeamLaunchOutput {
            message: format!(
                "Team '{}' launched (id: {}) with {} members and {} tasks",
                team.name, team.id, tmpl.members.len(), task_count
            ),
            team_id: team.id,
            name: team.name,
            leader: team.leader,
            member_count: tmpl.members.len(),
            tasks,
        })
    }
}
