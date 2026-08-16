//! `TeamWorkflowCanvasTool` — R8 wrapper around the DAG ↔ JSON Canvas bridge.
//!
//! Exposes both directions of [`crate::teams::workflow_canvas`] as one
//! builtin tool with an `action` discriminant (`export` or `import`),
//! mirroring `team_snapshot`'s shape. Lets the LLM produce / read Obsidian-
//! compatible workflow plans without leaving conversation.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::info;

use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStatus, CoordTaskStore};
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::workflow_canvas::{canvas_to_new_tasks, tasks_to_canvas};
use crate::tools::AlephTool;

use aleph_protocol::json_canvas::Document;

// =============================================================================
// Args / Output
// =============================================================================

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowCanvasAction {
    /// Render `team_id`'s coord-task DAG as a JSON Canvas document.
    Export,
    /// Create coord-tasks from a JSON Canvas document under `team_id`.
    Import,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamWorkflowCanvasArgs {
    pub action: WorkflowCanvasAction,
    pub team_id: String,
    /// Optional status filter for `export` (vocabulary matches
    /// `teams.list_tasks`).
    #[serde(default)]
    pub status: Option<String>,
    /// Required for `import` — the JSON Canvas document to materialize.
    #[serde(default)]
    pub canvas: Option<Document>,
    /// For `import`: when `true`, returns the planned tasks without mutating
    /// state. Defaults to live import.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TeamWorkflowCanvasOutput {
    Export {
        team_id: String,
        canvas: Document,
        node_count: usize,
        edge_count: usize,
    },
    Import {
        team_id: String,
        created: usize,
        dry_run: bool,
        tasks: Vec<Value>,
    },
}

// =============================================================================
// Tool
// =============================================================================

#[derive(Clone)]
pub struct TeamWorkflowCanvasTool {
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
    coord_store: Arc<dyn CoordTaskStore>,
}

impl TeamWorkflowCanvasTool {
    pub fn new(coord_store: Arc<dyn CoordTaskStore>) -> Self {
        Self {
            team_store: None,
            coord_store,
        }
    }

    /// Wire the ownership gate — see [`crate::teams::task_team_reachable`].
    #[must_use]
    pub fn with_team_store(mut self, store: Option<Arc<dyn crate::teams::TeamStore>>) -> Self {
        self.team_store = store;
        self
    }

    /// Parse the optional status filter. `None` input means "no filter".
    /// An unrecognised string is an error — silently returning `None` would
    /// drop the filter and export the entire DAG, the opposite of what the
    /// caller asked for. Vocabulary lives in the single-source
    /// [`CoordTaskStatus::from_filter_str`].
    fn parse_status(s: Option<&str>) -> Result<Option<CoordTaskStatus>> {
        let Some(raw) = s else {
            return Ok(None);
        };
        match CoordTaskStatus::from_filter_str(raw) {
            Some(status) => Ok(Some(status)),
            None => Err(AlephError::other(format!(
                "Invalid status filter '{raw}'. Expected one of: pending, blocked, \
                 in_progress, waiting_review, completed, failed, cancelled, skipped, \
                 paused, unsatisfiable."
            ))),
        }
    }
}

#[async_trait]
impl AlephTool for TeamWorkflowCanvasTool {
    const NAME: &'static str = "team_workflow_canvas";
    const DESCRIPTION: &'static str =
        "Convert between a team's CoordTask DAG and an Obsidian-compatible JSON Canvas \
        document. action='export' renders the team's tasks (optionally filtered by status / \
        owner) as a canvas — useful for showing a plan, generating diagrams, or exporting to \
        an .canvas file. action='import' takes a canvas document and creates coord-tasks \
        under team_id: each text node becomes a task; each edge becomes a `blocked_by` \
        dependency. Pass dry_run=true to preview the planned tasks without writing.";

    type Args = TeamWorkflowCanvasArgs;
    type Output = TeamWorkflowCanvasOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Ownership gate. Both actions are team-addressed, and `export`
        // enumerates every task in the team — the one shape here that hands
        // out ids for the other task tools, so it has to be gated even though
        // it only reads.
        if !crate::teams::task_team_reachable(self.team_store.as_ref(), Some(&args.team_id)).await {
            return Err(AlephError::invalid_input(format!(
                "team '{}' not found",
                args.team_id
            )));
        }
        match args.action {
            WorkflowCanvasAction::Export => {
                let filter = CoordTaskFilter {
                    team_id: Some(args.team_id.clone()),
                    status: Self::parse_status(args.status.as_deref())?,
                };
                let tasks = self
                    .coord_store
                    .list_tasks(filter)
                    .await
                    .map_err(|e| AlephError::other(format!("list_tasks failed: {e}")))?;
                let doc = tasks_to_canvas(&tasks);
                let node_count = doc.nodes.len();
                let edge_count = doc.edges.len();
                info!(
                    team_id = %args.team_id,
                    node_count,
                    edge_count,
                    "team_workflow_canvas: exported"
                );
                Ok(TeamWorkflowCanvasOutput::Export {
                    team_id: args.team_id,
                    canvas: doc,
                    node_count,
                    edge_count,
                })
            }
            WorkflowCanvasAction::Import => {
                let canvas = args.canvas.ok_or_else(|| {
                    AlephError::other("import action requires a `canvas` document")
                })?;
                let planned = canvas_to_new_tasks(&canvas, &args.team_id);

                if args.dry_run {
                    let tasks: Vec<Value> = planned
                        .into_iter()
                        .map(|t| {
                            serde_json::json!({
                                "subject": t.subject,
                                "description": t.description,
                                "blocked_by": t.blocked_by,
                                "metadata": t.metadata,
                            })
                        })
                        .collect();
                    return Ok(TeamWorkflowCanvasOutput::Import {
                        team_id: args.team_id,
                        created: tasks.len(),
                        dry_run: true,
                        tasks,
                    });
                }

                // Live import: same topological pass as the gateway handler,
                // duplicated here so the tool can be used without the gateway
                // being mounted (test / minimal setups). Kept simple — single
                // pass that fails loudly on cycles.
                let batch_canvas_ids: std::collections::HashSet<String> = planned
                    .iter()
                    .filter_map(|t| {
                        crate::teams::workflow_canvas::extract_canvas_task_id(&t.metadata)
                            .map(str::to_string)
                    })
                    .collect();
                let mut node_to_task: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                let mut work: Vec<crate::agents::swarm::tasks::NewCoordTask> = planned;
                let mut created: Vec<Value> = Vec::with_capacity(work.len());
                let max_passes = work.len() + 1;
                for _ in 0..max_passes {
                    if work.is_empty() {
                        break;
                    }
                    let mut remaining: Vec<crate::agents::swarm::tasks::NewCoordTask> = Vec::new();
                    for mut task in work.into_iter() {
                        let mut ready = true;
                        let mut rewritten = Vec::with_capacity(task.blocked_by.len());
                        for b in &task.blocked_by {
                            if let Some(live) = node_to_task.get(b) {
                                rewritten.push(live.clone());
                            } else if batch_canvas_ids.contains(b) {
                                ready = false;
                                break;
                            } else {
                                rewritten.push(b.clone());
                            }
                        }
                        if !ready {
                            remaining.push(task);
                            continue;
                        }
                        task.blocked_by = rewritten;
                        let canvas_id =
                            crate::teams::workflow_canvas::extract_canvas_task_id(&task.metadata)
                                .map(str::to_string);
                        let subject_for_err = task.subject.clone();
                        // Stamp the `origin_session` anchor so an autonomously
                        // dispatched workflow step enrolls into the launching
                        // session's goal tree budget (no-op outside a turn).
                        task.metadata = crate::gateway::goal_budget::with_origin_session(
                            std::mem::take(&mut task.metadata),
                        );
                        let t = self.coord_store.create_task(task).await.map_err(|e| {
                            AlephError::other(format!(
                                "create_task '{subject_for_err}' failed: {e}"
                            ))
                        })?;
                        if let Some(cid) = canvas_id {
                            node_to_task.insert(cid, t.id.clone());
                        }
                        created.push(serde_json::json!({
                            "id": t.id,
                            "subject": t.subject,
                        }));
                    }
                    work = remaining;
                }
                if !work.is_empty() {
                    return Err(AlephError::other(format!(
                        "canvas import: {} tasks unresolvable (cyclic or stranded)",
                        work.len()
                    )));
                }
                info!(
                    team_id = %args.team_id,
                    created = created.len(),
                    "team_workflow_canvas: imported"
                );
                Ok(TeamWorkflowCanvasOutput::Import {
                    team_id: args.team_id,
                    created: created.len(),
                    dry_run: false,
                    tasks: created,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_export_minimal() {
        let json = r#"{"action":"export","team_id":"t1"}"#;
        let args: TeamWorkflowCanvasArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.action, WorkflowCanvasAction::Export);
        assert_eq!(args.team_id, "t1");
        assert!(args.canvas.is_none());
        assert!(!args.dry_run);
    }

    #[test]
    fn args_import_with_canvas() {
        let json =
            r#"{"action":"import","team_id":"t1","canvas":{"nodes":[],"edges":[]},"dry_run":true}"#;
        let args: TeamWorkflowCanvasArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.action, WorkflowCanvasAction::Import);
        assert!(args.dry_run);
        let doc = args.canvas.unwrap();
        assert!(doc.nodes.is_empty());
    }
}
