//! `workflow` tool — manage and run declarative workflow templates (R8).
//!
//! The LLM-facing surface for the [`crate::workflow`] layer: save / list /
//! describe / delete reusable templates, and `run` one against a team. `run`
//! compiles the template into the existing `coord_tasks` DAG
//! ([`crate::workflow::materialize`]) and signals the dispatcher — execution
//! then proceeds on the existing autonomous loop. This tool performs **no
//! orchestration of its own** (R10).
//!
//! Single tool with an `action` discriminator, mirroring `workflow_step_review`
//! and `team_snapshot`.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::agents::swarm::tasks::CoordTaskStore;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use crate::workflow::{self, WorkflowDef};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum WorkflowArgs {
    /// Save (create or overwrite) a reusable workflow template to disk.
    Save { definition: WorkflowDef },
    /// List the names of all saved workflow templates.
    List {},
    /// Show the full definition of a saved workflow template.
    Describe { name: String },
    /// Delete a saved workflow template. Idempotent.
    Delete { name: String },
    /// Run a saved workflow: compile its steps into coordination tasks owned
    /// by the named team's members and start execution. Create the team first
    /// (with `team_create`) so every step's `agent` resolves to a member.
    Run {
        /// Name of the saved template to run.
        name: String,
        /// Team that hosts the run; its members own the materialised steps.
        team_id: String,
        /// Run input substituted for `{input}` in each step's prompt.
        #[serde(default)]
        input: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowToolOutput {
    pub action: String,
    pub message: String,
    /// Populated by `list`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub names: Option<Vec<String>>,
    /// Populated by `describe`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<WorkflowDef>,
    /// Populated by `run` — the created coordination-task ids.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<Vec<String>>,
}

impl WorkflowToolOutput {
    fn msg(action: &str, message: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            message: message.into(),
            names: None,
            definition: None,
            task_ids: None,
        }
    }
}

#[derive(Clone)]
pub struct WorkflowTool {
    coord_store: Arc<dyn CoordTaskStore>,
    /// Wakes the team dispatcher after `run` so materialised tasks start
    /// without waiting for the fallback tick. `None` → tasks still run on the
    /// dispatcher's periodic tick, just with added latency.
    dispatch_signal: Option<Arc<tokio::sync::Notify>>,
}

impl WorkflowTool {
    pub fn new(
        coord_store: Arc<dyn CoordTaskStore>,
        dispatch_signal: Option<Arc<tokio::sync::Notify>>,
    ) -> Self {
        Self {
            coord_store,
            dispatch_signal,
        }
    }
}

#[async_trait]
impl AlephTool for WorkflowTool {
    const NAME: &'static str = "workflow";
    const DESCRIPTION: &'static str =
        "Manage and run reusable workflow templates. A template is a named, \
         declarative multi-step pipeline (each step = one agent + a prompt + \
         dependencies); running it compiles the steps into a coordination-task \
         DAG that executes concurrently where dependencies allow. \
         Actions: save / list / describe / delete / run. For `run`, create a \
         team first so each step's agent resolves to a member.";

    type Args = WorkflowArgs;
    type Output = WorkflowToolOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"workflow(action='save', definition={"name":"research-report","description":"research then write","steps":[{"id":"gather","agent":"researcher","prompt":"research {input}"},{"id":"write","agent":"writer","prompt":"write a report","depends_on":["gather"]}]})"#.into(),
            "workflow(action='list')".into(),
            "workflow(action='describe', name='research-report')".into(),
            "workflow(action='run', name='research-report', team_id='team-42', input='quantum error correction')".into(),
            "workflow(action='delete', name='research-report')".into(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args {
            WorkflowArgs::Save { definition } => {
                debug!(name = %definition.name, "workflow: save");
                let path = workflow::store::save(&definition)?;
                Ok(WorkflowToolOutput::msg(
                    "save",
                    format!("saved workflow '{}' → {}", definition.name, path.display()),
                ))
            }
            WorkflowArgs::List {} => {
                let names: Vec<String> =
                    workflow::store::list()?.into_iter().map(|m| m.name).collect();
                Ok(WorkflowToolOutput {
                    action: "list".into(),
                    message: format!("{} workflow(s)", names.len()),
                    names: Some(names),
                    definition: None,
                    task_ids: None,
                })
            }
            WorkflowArgs::Describe { name } => {
                let def = workflow::store::load(&name)?;
                Ok(WorkflowToolOutput {
                    action: "describe".into(),
                    message: format!("workflow '{name}' has {} step(s)", def.steps.len()),
                    names: None,
                    definition: Some(def),
                    task_ids: None,
                })
            }
            WorkflowArgs::Delete { name } => {
                let removed = workflow::store::delete(&name)?;
                let message = if removed {
                    format!("deleted workflow '{name}'")
                } else {
                    format!("workflow '{name}' did not exist")
                };
                Ok(WorkflowToolOutput::msg("delete", message))
            }
            WorkflowArgs::Run {
                name,
                team_id,
                input,
            } => {
                debug!(name = %name, team_id = %team_id, "workflow: run");
                let def = workflow::store::load(&name)?;
                let mat =
                    workflow::materialize(&def, &input, &team_id, self.coord_store.as_ref()).await?;
                if let Some(signal) = &self.dispatch_signal {
                    signal.notify_one();
                }
                Ok(WorkflowToolOutput {
                    action: "run".into(),
                    message: format!(
                        "started workflow '{name}' on team '{team_id}': {} task(s) queued",
                        mat.task_ids.len()
                    ),
                    names: None,
                    definition: None,
                    task_ids: Some(mat.task_ids),
                })
            }
        }
    }
}
