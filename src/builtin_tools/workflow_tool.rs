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

use crate::agents::swarm::tasks::{
    CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate,
};
use crate::error::{AlephError, Result};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::tools::turn_context::current_turn_context;
use crate::tools::AlephTool;
use crate::workflow::{
    self, ClarifyContext, WorkflowDef, WorkflowManifest, WORKFLOW_NAME_KEY, WORKFLOW_RUN_ID_KEY,
    WORKFLOW_STEP_KEY,
};

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
    /// Report the live status of a workflow run: one row per step with the
    /// backing task id, status, and owner. Defaults to the most recently
    /// started run of `name` on `team_id`; pass `run_id` (returned by `run`)
    /// to inspect an older one.
    Status {
        /// Name of the workflow template the run was started from.
        name: String,
        /// Team hosting the run.
        team_id: String,
        /// Specific run to inspect; omitted → the latest run.
        #[serde(default)]
        run_id: Option<String>,
    },
    /// Cancel the remaining steps of a workflow run: every not-yet-finished
    /// task (pending / blocked / paused / `waiting_review` / `in_progress`) is
    /// marked Cancelled. An in-progress step's member run finishes on its own
    /// but cannot resurrect the task; finished steps keep their results.
    Cancel {
        /// Name of the workflow template the run was started from.
        name: String,
        /// Team hosting the run.
        team_id: String,
        /// Specific run to cancel; omitted → the latest run.
        #[serde(default)]
        run_id: Option<String>,
    },
    /// Render a saved template into a Claude-Code-compatible `.workflow.js`.
    Export {
        /// Name of the saved template to render.
        name: String,
        /// Also write it to `$ALEPH_HOME/workflows/<name>.workflow.js`.
        #[serde(default)]
        write_file: bool,
    },
    /// Parse a `.workflow.js` (or AWI manifest JSON) into a `WorkflowDef`.
    Import {
        /// Raw `.workflow.js` text or AWI manifest JSON.
        source: String,
        /// Also persist the parsed template via the store.
        #[serde(default)]
        save: bool,
    },
    /// List the gated `MetaSkill` proposals the dream pipeline auto-drafted from
    /// recurring skill co-occurrence. These are NOT active until accepted.
    Proposals {},
    /// Inspect a gated `MetaSkill` proposal *before* accepting it: returns its
    /// step definition and provenance (which skill chain, how many observations)
    /// so the gate can be reviewed rather than accepted blind. Reads the draft
    /// from the `proposals/` dir — plain `describe` only sees active workflows.
    DescribeProposal {
        /// Name of the pending proposal (see `action='proposals'`).
        name: String,
    },
    /// Accept (activate) a gated `MetaSkill` proposal: promote it from the
    /// `proposals/` draft dir into the active workflow store, then run it with
    /// `action='run'`. The draft is removed once accepted.
    AcceptProposal {
        /// Name of the pending proposal (see `action='proposals'`).
        name: String,
    },
}

/// One step of a workflow run in a `status` report — a mechanical projection
/// of the backing `coord_task` (R7: data for the LLM to reason over, no
/// judgement of its own).
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowRunStep {
    /// Step-local id from the template (`workflow_step` metadata).
    pub step: String,
    /// Backing coordination-task id — feed to `workflow_step_review` /
    /// `team_task_control` for per-step intervention.
    pub task_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// For failed steps: the (bounded) error text, so the LLM can decide
    /// retry / skip / cancel without an extra lookup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    /// Populated by `run` — the created coordination-task ids — and by
    /// `cancel` — the task ids actually cancelled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<Vec<String>>,
    /// Populated by `run` / `status` / `cancel` — the run identity grouping
    /// the materialised tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Populated by `status` — one row per step in creation (topological)
    /// order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<WorkflowRunStep>>,
    /// Populated by `export` — the rendered `.workflow.js` text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered: Option<String>,
    /// Populated by `import` — imperative constructs that could not be mapped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped: Option<Vec<String>>,
}

impl WorkflowToolOutput {
    fn msg(action: &str, message: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            message: message.into(),
            names: None,
            definition: None,
            task_ids: None,
            run_id: None,
            steps: None,
            rendered: None,
            dropped: None,
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
    /// Team roster used to pre-flight a `run`: before materialising, verify the
    /// target team actually covers every agent step's `agent`. `None` → the
    /// check is skipped (the dispatcher still fail-fasts any doomed task with a
    /// clear reason, just asynchronously).
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
    /// Tool-free planner provider; `None` → no Strategy minted on `run`.
    planner_provider: Option<Arc<dyn AiProvider>>,
}

impl WorkflowTool {
    pub fn new(
        coord_store: Arc<dyn CoordTaskStore>,
        dispatch_signal: Option<Arc<tokio::sync::Notify>>,
    ) -> Self {
        Self {
            coord_store,
            dispatch_signal,
            team_store: None,
            planner_provider: None,
        }
    }

    #[must_use]
    pub fn with_planner_provider(mut self, provider: Option<Arc<dyn AiProvider>>) -> Self {
        self.planner_provider = provider;
        self
    }

    /// Wire the team roster so `run` can pre-flight team coverage. Builder form
    /// keeps `new` two-arg (every existing caller and test compiles unchanged);
    /// `None` leaves the check disabled.
    #[must_use]
    pub fn with_team_store(mut self, team_store: Option<Arc<dyn crate::teams::TeamStore>>) -> Self {
        self.team_store = team_store;
        self
    }

    /// Reject a `run` whose team cannot execute it *before* materialising any
    /// `coord_tasks`. A clarify step is owned by the sentinel (not a team member),
    /// so it is exempt; only agent steps require a covering member. When no team
    /// store is wired the check is a no-op — the dispatcher still fail-fasts a
    /// doomed task, just after the run already reported success.
    ///
    /// Fails fast with an actionable message naming the uncovered agents and the
    /// team's actual members, so the LLM can `team_create` / `team_member_add`
    /// and retry instead of discovering the doomed run by polling task status
    /// (P7 boundary validation, R8 tool feedback).
    async fn preflight_team_coverage(&self, def: &WorkflowDef, team_id: &str) -> Result<()> {
        let Some(team_store) = &self.team_store else {
            return Ok(());
        };
        let members = team_store.get_members(team_id).await?;
        let covered: std::collections::HashSet<&str> =
            members.iter().map(|m| m.agent_id.as_str()).collect();

        let mut missing: Vec<&str> = def
            .steps
            .iter()
            .filter(|s| !s.is_clarify())
            .map(|s| s.agent.as_str())
            .filter(|a| !covered.contains(a))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        missing.sort_unstable();
        missing.dedup();

        let have = if covered.is_empty() {
            "none".to_string()
        } else {
            let mut h: Vec<&str> = covered.into_iter().collect();
            h.sort_unstable();
            h.join(", ")
        };
        Err(AlephError::invalid_input(format!(
            "workflow '{}' cannot run on team '{team_id}': step agent(s) [{}] are not team members \
             (current members: {have}). Add them with team_member_add (or create the team with \
             team_create) first.",
            def.name,
            missing.join(", "),
        )))
    }

    /// Resolve the tasks of one materialised run of `name` on `team_id`.
    ///
    /// Tasks are grouped by the `workflow_run_id` the compiler stamped at
    /// materialisation; `run_id=None` selects the most recently started group
    /// (max `created_at`). Returns the resolved run id plus its tasks in
    /// creation (topological) order. Runs materialised before run ids existed
    /// group under the empty id and stay reachable as the latest run.
    async fn run_tasks(
        &self,
        name: &str,
        team_id: &str,
        run_id: Option<&str>,
    ) -> Result<(String, Vec<CoordTask>)> {
        let tasks = self
            .coord_store
            .list_tasks(CoordTaskFilter {
                team_id: Some(team_id.to_string()),
                ..Default::default()
            })
            .await?;

        let mut groups: std::collections::HashMap<String, Vec<CoordTask>> =
            std::collections::HashMap::new();
        for task in tasks {
            if task
                .metadata
                .get(WORKFLOW_NAME_KEY)
                .and_then(|v| v.as_str())
                != Some(name)
            {
                continue;
            }
            let rid = task
                .metadata
                .get(WORKFLOW_RUN_ID_KEY)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            groups.entry(rid).or_default().push(task);
        }
        if groups.is_empty() {
            return Err(AlephError::invalid_input(format!(
                "no runs of workflow '{name}' found on team '{team_id}' — start one with \
                 action='run'"
            )));
        }

        let selected = match run_id {
            Some(rid) => rid.to_string(),
            // Latest run = the group whose newest task was created last.
            // created_at is epoch *seconds*, so two runs started within the
            // same second tie; the run id breaks the tie deterministically
            // (arbitrary but stable — pass run_id explicitly to disambiguate).
            None => groups
                .iter()
                .max_by_key(|(rid, tasks)| {
                    (tasks.iter().map(|t| t.created_at).max().unwrap_or(0), *rid)
                })
                .map(|(rid, _)| rid.clone())
                .unwrap_or_default(),
        };
        let mut tasks = groups.remove(&selected).ok_or_else(|| {
            AlephError::invalid_input(format!(
                "no run '{selected}' of workflow '{name}' on team '{team_id}' — omit run_id for \
                 the latest run"
            ))
        })?;
        // Deterministic step order for status output. `created_at` is epoch
        // *seconds*, so steps materialised within the same second tie; the
        // backing `list_tasks` query has no final tiebreaker, so SQLite would
        // otherwise return tied rows in arbitrary order. Break ties by
        // topological rank (a step appears after every step it depends on),
        // which reproduces the workflow-definition order, then by task id so
        // independent same-rank steps stay stable.
        let ranks = topological_ranks(&tasks);
        tasks.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| ranks[&a.id].cmp(&ranks[&b.id]))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok((selected, tasks))
    }

    /// Tool-free planner for a workflow run, fail-soft. Returns `None` when no
    /// provider is injected or the planner self-gates/errs. The objective is the
    /// run input (the user's request for this workflow execution).
    async fn plan_workflow_strategy(
        &self,
        def: &WorkflowDef,
        input: &str,
    ) -> Option<crate::strategy::Strategy> {
        let provider = self.planner_provider.as_ref()?;
        let objective = if input.trim().is_empty() {
            format!("Run workflow '{}'", def.name)
        } else {
            input.to_string()
        };
        let ctx = crate::strategy::planner::PlannerContext {
            tool_descriptions: Vec::new(),
            env_summary: crate::strategy::planner::env_summary(),
            lessons: Vec::new(),
        };
        crate::strategy::planner::plan_strategy(provider, &objective, &ctx, None).await
    }
}

/// Assign each task a topological rank within `tasks`: a task's rank is one
/// greater than the maximum rank of any task it depends on (roots are 0).
/// Dependencies pointing outside this set are ignored. Pure, deterministic, and
/// cycle-safe (a task already on the visiting stack contributes rank 0).
fn topological_ranks(
    tasks: &[CoordTask],
) -> std::collections::HashMap<crate::agents::swarm::tasks::CoordTaskId, usize> {
    use std::collections::HashMap;

    let by_id: HashMap<_, &CoordTask> = tasks.iter().map(|t| (t.id.clone(), t)).collect();
    let mut ranks: HashMap<_, usize> = HashMap::new();

    fn rank_of(
        id: &crate::agents::swarm::tasks::CoordTaskId,
        by_id: &HashMap<crate::agents::swarm::tasks::CoordTaskId, &CoordTask>,
        ranks: &mut HashMap<crate::agents::swarm::tasks::CoordTaskId, usize>,
        visiting: &mut std::collections::HashSet<crate::agents::swarm::tasks::CoordTaskId>,
    ) -> usize {
        if let Some(r) = ranks.get(id) {
            return *r;
        }
        if !visiting.insert(id.clone()) {
            // Cycle guard: treat the re-entrant node as a root for this path.
            return 0;
        }
        let task = by_id.get(id);
        let rank = task
            .map(|t| {
                t.dependencies
                    .iter()
                    .filter(|dep| by_id.contains_key(*dep))
                    .map(|dep| rank_of(dep, by_id, ranks, visiting) + 1)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        visiting.remove(id);
        ranks.insert(id.clone(), rank);
        rank
    }

    let mut visiting = std::collections::HashSet::new();
    for t in tasks {
        rank_of(&t.id, &by_id, &mut ranks, &mut visiting);
    }
    ranks
}

/// Project one backing task into its `status` row (pure).
fn step_row(task: &CoordTask) -> WorkflowRunStep {
    WorkflowRunStep {
        step: task
            .metadata
            .get(WORKFLOW_STEP_KEY)
            .and_then(|v| v.as_str())
            .unwrap_or(&task.subject)
            .to_string(),
        task_id: task.id.clone(),
        status: task.status.as_str().to_string(),
        owner: task.owner.clone(),
        error: match task.status {
            CoordTaskStatus::Failed => task
                .result
                .as_deref()
                .filter(|r| !r.trim().is_empty())
                .map(|r| bound_chars(r, 400)),
            _ => None,
        },
    }
}

/// Count tasks per status into a compact "2 completed, 1 failed, ..." summary
/// (pure aggregation — interpreting it is the LLM's job, R7).
fn summarize_statuses(tasks: &[CoordTask]) -> String {
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    for task in tasks {
        let key = task.status.as_str();
        match counts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => counts.push((key, 1)),
        }
    }
    let parts: Vec<String> = counts
        .into_iter()
        .map(|(k, n)| format!("{n} {k}"))
        .collect();
    format!("{} step(s): {}", tasks.len(), parts.join(", "))
}

/// Truncate to at most `max` characters on a char boundary (P7 UTF-8 safety).
fn bound_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((byte_idx, _)) => format!("{}…", &s[..byte_idx]),
        None => s.to_string(),
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
         Actions: save / list / describe / delete / run / status / cancel / \
         export / import / proposals / describe_proposal / accept_proposal. \
         `run` returns a run_id; `status` reports the per-step task states of \
         a run (latest by default) and `cancel` aborts its unfinished steps — \
         finished steps keep their results, and a step caught mid-execution \
         finishes its member run but stays cancelled. \
         `export` renders a template to a Claude-Code-compatible .workflow.js; \
         `import` parses one back into a template. `proposals` lists MetaSkill \
         drafts the dream pipeline auto-grew from recurring skill use; \
         `describe_proposal` reviews one's steps + provenance before \
         `accept_proposal` activates it. For `run`, create a team first so \
         each step's agent resolves to a member.";

    type Args = WorkflowArgs;
    type Output = WorkflowToolOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"workflow(action='save', definition={"name":"research-report","description":"research then write","steps":[{"id":"gather","agent":"researcher","prompt":"research {input}"},{"id":"write","agent":"writer","prompt":"write a report","depends_on":["gather"]}]})"#.into(),
            "workflow(action='list')".into(),
            "workflow(action='describe', name='research-report')".into(),
            "workflow(action='run', name='research-report', team_id='team-42', input='quantum error correction')".into(),
            "workflow(action='status', name='research-report', team_id='team-42')".into(),
            "workflow(action='cancel', name='research-report', team_id='team-42', run_id='1f2e…')".into(),
            "workflow(action='delete', name='research-report')".into(),
            "workflow(action='export', name='research-report')".into(),
            r#"workflow(action='import', source='export const meta = { name: \"x\" }\nawait agent(\"do it\")', save=true)"#.into(),
            "workflow(action='proposals')".into(),
            "workflow(action='describe_proposal', name='metaskill-git-pr')".into(),
            "workflow(action='accept_proposal', name='metaskill-git-pr')".into(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args {
            WorkflowArgs::Save { definition } => {
                debug!(name = %definition.name, "workflow: save");
                // `save` authors the lean executable core; persist it as a
                // manifest (extras empty) so the on-disk format stays uniform.
                let manifest = WorkflowManifest::from_def(&definition);
                let path = workflow::store::save(&manifest)?;
                Ok(WorkflowToolOutput::msg(
                    "save",
                    format!("saved workflow '{}' → {}", definition.name, path.display()),
                ))
            }
            WorkflowArgs::List {} => {
                let names: Vec<String> = workflow::store::list()?
                    .into_iter()
                    .map(|m| m.name)
                    .collect();
                let message = format!("{} workflow(s)", names.len());
                Ok(WorkflowToolOutput {
                    names: Some(names),
                    ..WorkflowToolOutput::msg("list", message)
                })
            }
            WorkflowArgs::Describe { name } => {
                let manifest = workflow::store::load(&name)?;
                // Output the executable projection — the tool's `definition`
                // field is a `WorkflowDef`; the extra metadata is reachable via
                // `export`.
                let message = format!("workflow '{name}' has {} step(s)", manifest.steps.len());
                Ok(WorkflowToolOutput {
                    definition: Some(manifest.to_def()),
                    ..WorkflowToolOutput::msg("describe", message)
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
                // Load the full manifest: the executable core (`to_def`) drives
                // materialisation, while per-step `model` overrides — which the
                // lean `WorkflowDef` does not carry — are threaded in separately
                // and stamped onto task metadata for the dispatcher to resolve
                // at launch (the executable wiring of the manifest's `model`
                // field). Other interchange-only metadata
                // (schema/isolation/agentType/phase) the executor still ignores
                // (R10).
                let manifest = workflow::store::load(&name)?;
                let def = manifest.to_def();
                // step-local id → model override; empty when no step pins a model.
                let models: std::collections::HashMap<String, String> = manifest
                    .steps
                    .iter()
                    .filter_map(|s| s.model.as_ref().map(|m| (s.id.clone(), m.clone())))
                    .collect();
                // Pre-flight: reject a run the team cannot execute before any
                // coord_task is created, so the LLM gets an immediate, actionable
                // error instead of a "success" that the dispatcher then fails
                // task-by-task in the background (P7 / R8). No-op when no team
                // store is wired.
                self.preflight_team_coverage(&def, &team_id).await?;
                // Capture the originating channel so any `clarify` step can reach
                // the user from the autonomous dispatcher, where the launching
                // turn no longer exists. A non-interactive run yields `None`;
                // clarify steps then fail fast at delivery (clear reason) rather
                // than stalling the DAG.
                let clarify_ctx = current_turn_context()
                    .filter(|t| t.is_channel_routable())
                    .map(|t| ClarifyContext {
                        channel_id: t.channel_id.clone(),
                        conversation_id: t.conversation_id.clone(),
                        session_key: t.session_key.to_string(),
                    });
                // Plan ONCE before materialisation: the planner sees the run
                // input + WorkflowDef and produces a run-global Strategy. It does
                // not need the run_id (minted inside materialize). Fail-soft.
                let strategy = self.plan_workflow_strategy(&def, &input).await;
                let mat = workflow::materialize(
                    &def,
                    &input,
                    &team_id,
                    self.coord_store.as_ref(),
                    clarify_ctx.as_ref(),
                    (!models.is_empty()).then_some(&models),
                    strategy.as_ref(),
                )
                .await?;
                // Persist under the now-known run_id so continuations read it via
                // active_strategy/workflow_key. Best-effort; write only when the
                // slot is provably empty (Ok(None)) — never on Ok(Some)/Err (P7).
                if let Some(strategy) = &strategy {
                    if let Some(store) = crate::strategy::global() {
                        let key = crate::strategy::workflow_key(&mat.run_id);
                        if matches!(store.get(&key), Ok(None)) {
                            let _ = store.put(&key, strategy);
                        }
                    }
                }
                if let Some(signal) = &self.dispatch_signal {
                    signal.notify_one();
                }
                let message = format!(
                    "started workflow '{name}' on team '{team_id}': {} task(s) queued \
                     (run_id {}; inspect with action='status', abort with action='cancel')",
                    mat.task_ids.len(),
                    mat.run_id
                );
                Ok(WorkflowToolOutput {
                    task_ids: Some(mat.task_ids),
                    run_id: Some(mat.run_id),
                    ..WorkflowToolOutput::msg("run", message)
                })
            }
            WorkflowArgs::Status {
                name,
                team_id,
                run_id,
            } => {
                debug!(name = %name, team_id = %team_id, "workflow: status");
                let (run_id, tasks) = self.run_tasks(&name, &team_id, run_id.as_deref()).await?;
                let steps: Vec<WorkflowRunStep> = tasks.iter().map(step_row).collect();
                let message = format!(
                    "workflow '{name}' run {run_id}: {}",
                    summarize_statuses(&tasks)
                );
                Ok(WorkflowToolOutput {
                    run_id: Some(run_id),
                    steps: Some(steps),
                    ..WorkflowToolOutput::msg("status", message)
                })
            }
            WorkflowArgs::Cancel {
                name,
                team_id,
                run_id,
            } => {
                debug!(name = %name, team_id = %team_id, "workflow: cancel");
                let (run_id, tasks) = self.run_tasks(&name, &team_id, run_id.as_deref()).await?;
                let mut cancelled: Vec<String> = Vec::new();
                let mut in_flight = 0usize;
                let mut finished = 0usize;
                for task in &tasks {
                    match task.status {
                        // Terminal — already settled, leave untouched.
                        CoordTaskStatus::Completed
                        | CoordTaskStatus::Failed
                        | CoordTaskStatus::Cancelled
                        | CoordTaskStatus::Skipped => finished += 1,
                        status => {
                            if status == CoordTaskStatus::InProgress {
                                in_flight += 1;
                            }
                            self.coord_store
                                .update_task(
                                    &task.id,
                                    CoordTaskUpdate {
                                        status: Some(CoordTaskStatus::Cancelled),
                                        ..Default::default()
                                    },
                                )
                                .await?;
                            cancelled.push(task.id.clone());
                        }
                    }
                }
                let mut message = format!(
                    "cancelled {} step(s) of workflow '{name}' run {run_id} ({finished} already finished)",
                    cancelled.len()
                );
                if in_flight > 0 {
                    message.push_str(&format!(
                        "; {in_flight} in-flight member run(s) will finish on their own but \
                         cannot resurrect their cancelled task(s)"
                    ));
                }
                Ok(WorkflowToolOutput {
                    task_ids: Some(cancelled),
                    run_id: Some(run_id),
                    ..WorkflowToolOutput::msg("cancel", message)
                })
            }
            WorkflowArgs::Export { name, write_file } => {
                debug!(name = %name, write_file, "workflow: export");
                // The stored manifest carries the full `.workflow.js` metadata,
                // so the render is now faithful (phases, per-step
                // label/model/phase/schema) rather than a bare skeleton.
                let manifest = workflow::store::load(&name)?;
                let rendered = workflow::render_workflow_js(&manifest);
                let message = if write_file {
                    let path = workflow::store::write_text(&name, "workflow.js", &rendered)?;
                    format!("exported workflow '{name}' → {}", path.display())
                } else {
                    format!("rendered workflow '{name}' ({} bytes)", rendered.len())
                };
                Ok(WorkflowToolOutput {
                    rendered: Some(rendered),
                    ..WorkflowToolOutput::msg("export", message)
                })
            }
            WorkflowArgs::Import { source, save } => {
                debug!(save, "workflow: import");
                let outcome = workflow::parse_workflow_js(&source)?;
                let def = outcome.manifest.to_def();
                // On validation failure, fold the best-effort scan's `dropped`
                // diagnostics into the error so the user keeps the context that
                // the import was lossy (imperative constructs were skipped) —
                // otherwise `?` would discard `outcome.dropped` silently.
                if let Err(e) = outcome.manifest.validate() {
                    if outcome.dropped.is_empty() {
                        return Err(e);
                    }
                    return Err(AlephError::invalid_input(format!(
                        "{e}; note: import dropped {} imperative construct(s): {}",
                        outcome.dropped.len(),
                        outcome.dropped.join("; ")
                    )));
                }
                let message = if save {
                    // Persist the full manifest so an `import` of a rich
                    // `.workflow.js` keeps its phases/schema/model on disk.
                    let path = workflow::store::save(&outcome.manifest)?;
                    format!(
                        "imported workflow '{}' ({} step(s)) → {}",
                        def.name,
                        def.steps.len(),
                        path.display()
                    )
                } else {
                    format!(
                        "parsed workflow '{}' ({} step(s); not saved)",
                        def.name,
                        def.steps.len()
                    )
                };
                Ok(WorkflowToolOutput {
                    definition: Some(def),
                    dropped: Some(outcome.dropped),
                    ..WorkflowToolOutput::msg("import", message)
                })
            }
            WorkflowArgs::Proposals {} => {
                let names: Vec<String> = workflow::proposal::list_proposals()?
                    .into_iter()
                    .map(|m| m.name)
                    .collect();
                let message = format!(
                    "{} gated MetaSkill proposal(s) — inspect one with \
                     action='describe_proposal', activate with action='accept_proposal'",
                    names.len()
                );
                Ok(WorkflowToolOutput {
                    names: Some(names),
                    ..WorkflowToolOutput::msg("proposals", message)
                })
            }
            WorkflowArgs::DescribeProposal { name } => {
                debug!(name = %name, "workflow: describe_proposal");
                // Read the draft from the gated `proposals/` dir (NOT the active
                // store `describe` uses) so the gate is reviewable before accept.
                // The provenance — observed skill chain + count — rides in the
                // skeleton's `description` (see `proposal::skeleton_from_chain`).
                let manifest = workflow::proposal::load_proposal(&name)?;
                let def = manifest.to_def();
                let provenance = if def.description.trim().is_empty() {
                    "(no provenance recorded)".to_string()
                } else {
                    def.description.clone()
                };
                let message = format!(
                    "proposal '{name}': {} step(s) — {provenance}",
                    def.steps.len()
                );
                Ok(WorkflowToolOutput {
                    definition: Some(def),
                    ..WorkflowToolOutput::msg("describe_proposal", message)
                })
            }
            WorkflowArgs::AcceptProposal { name } => {
                debug!(name = %name, "workflow: accept_proposal");
                let path = workflow::proposal::accept(&name)?;
                Ok(WorkflowToolOutput::msg(
                    "accept_proposal",
                    format!(
                        "accepted MetaSkill '{name}' → active at {} (run with action='run')",
                        path.display()
                    ),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::{store::SqliteCoordTaskStore, CoordTaskStatus};
    use crate::sync_primitives::Mutex;
    use crate::workflow::def::WorkflowStepDef;
    use rusqlite::Connection;
    use tempfile::TempDir;

    // `ALEPH_HOME` is process-global; the file-backed actions (save/list/
    // describe/delete/run-load) resolve their directory from it via
    // `workflow::store::*`. Serialise every test that touches it through this
    // guard so parallel `cargo test` threads can't read/write each other's
    // workflows dir. Pure serde/notify tests below need no env and skip it.
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    async fn setup_store() -> SqliteCoordTaskStore {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.expect("migrate");
        store
    }

    fn linear_def() -> WorkflowDef {
        WorkflowDef {
            name: "pipeline".into(),
            description: "research then write".into(),
            steps: vec![
                WorkflowStepDef {
                    id: "gather".into(),
                    agent: "researcher".into(),
                    prompt: "research {input}".into(),
                    depends_on: vec![],
                    kind: crate::workflow::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                },
                WorkflowStepDef {
                    id: "write".into(),
                    agent: "writer".into(),
                    prompt: "write a report".into(),
                    depends_on: vec!["gather".into()],
                    kind: crate::workflow::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                },
            ],
        }
    }

    fn tool(store: SqliteCoordTaskStore, signal: Option<Arc<tokio::sync::Notify>>) -> WorkflowTool {
        WorkflowTool::new(Arc::new(store), signal)
    }

    // --- serde discriminator: the exact shape the agent loop deserialises ---

    #[test]
    fn deserialize_run_defaults_input() {
        // `input` omitted relies on #[serde(default)] → empty string.
        let args: WorkflowArgs =
            serde_json::from_value(serde_json::json!({"action":"run","name":"p","team_id":"t"}))
                .expect("deserialise run without input");
        match args {
            WorkflowArgs::Run {
                name,
                team_id,
                input,
            } => {
                assert_eq!(name, "p");
                assert_eq!(team_id, "t");
                assert_eq!(input, "", "missing input defaults to empty string");
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_save_nested_definition() {
        let args: WorkflowArgs = serde_json::from_value(serde_json::json!({
            "action": "save",
            "definition": {
                "name": "research-report",
                "steps": [
                    {"id": "gather", "agent": "researcher", "prompt": "research {input}"},
                    {"id": "write", "agent": "writer", "prompt": "write", "depends_on": ["gather"]}
                ]
            }
        }))
        .expect("deserialise save with nested definition");
        match args {
            WorkflowArgs::Save { definition } => {
                assert_eq!(definition.name, "research-report");
                assert_eq!(definition.steps.len(), 2);
                assert_eq!(definition.steps[1].depends_on, vec!["gather".to_string()]);
            }
            other => panic!("expected Save, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_list_unit_variant() {
        let args: WorkflowArgs =
            serde_json::from_value(serde_json::json!({"action":"list"})).expect("deserialise list");
        assert!(matches!(args, WorkflowArgs::List {}));
    }

    #[test]
    fn deserialize_rejects_unknown_action() {
        let err =
            serde_json::from_value::<WorkflowArgs>(serde_json::json!({"action":"frobnicate"}));
        assert!(err.is_err(), "unknown action must not deserialise");
    }

    // --- output shaping: which Option fields each action populates ---

    #[test]
    fn output_msg_helper_leaves_optionals_none() {
        let out = WorkflowToolOutput::msg("save", "ok");
        assert_eq!(out.action, "save");
        assert!(out.names.is_none());
        assert!(out.definition.is_none());
        assert!(out.task_ids.is_none());
    }

    // --- run action (fully injectable: no real team/dispatcher needed) ---

    #[tokio::test]
    async fn run_materializes_tasks_and_returns_ids() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        // `save` then `run` both resolve their dir from ALEPH_HOME; hold the
        // guard across both so the env stays hermetic for the whole sequence.
        let run_out = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored after the await completes.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            workflow::store::save(&WorkflowManifest::from_def(&linear_def()))
                .expect("save under temp ALEPH_HOME");
            let r = t
                .call(WorkflowArgs::Run {
                    name: "pipeline".into(),
                    team_id: "team-7".into(),
                    input: "quantum".into(),
                })
                .await;
            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            r
        }
        .expect("run materialises");

        assert_eq!(run_out.action, "run");
        let ids = run_out.task_ids.as_ref().expect("run populates task_ids");
        assert_eq!(ids.len(), 2, "one task per step");
        // run shapes only task_ids — never names/definition.
        assert!(run_out.names.is_none());
        assert!(run_out.definition.is_none());

        // The returned ids correspond to actually-created, correctly-wired
        // coord_tasks: gather is Pending (no deps), write is Blocked on it.
        let cstore = t.coord_store.clone();
        let gather = cstore.get_task(&ids[0]).await.unwrap().unwrap();
        let write = cstore.get_task(&ids[1]).await.unwrap().unwrap();
        assert_eq!(gather.subject, "pipeline:gather");
        assert_eq!(
            gather.description, "research quantum",
            "{{input}} substituted"
        );
        assert_eq!(gather.status, CoordTaskStatus::Pending);
        assert_eq!(write.subject, "pipeline:write");
        assert_eq!(write.status, CoordTaskStatus::Blocked);
    }

    #[tokio::test]
    async fn run_notifies_dispatcher_when_signal_present() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let signal = Arc::new(tokio::sync::Notify::new());
        let t = tool(store, Some(signal.clone()));

        // Register a waiter BEFORE run so notify_one delivers a persistent
        // permit even if it fires before we await.
        let notified = signal.notified();

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }
        workflow::store::save(&WorkflowManifest::from_def(&linear_def())).expect("save");
        let run = t
            .call(WorkflowArgs::Run {
                name: "pipeline".into(),
                team_id: "team-7".into(),
                input: "x".into(),
            })
            .await;
        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
        run.expect("run");

        // The waiter must resolve promptly; a generous timeout keeps the test
        // from hanging if the notify is ever dropped.
        tokio::time::timeout(std::time::Duration::from_secs(2), notified)
            .await
            .expect("dispatcher was signalled");
    }

    #[tokio::test]
    async fn run_without_signal_still_returns_ids() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }
        workflow::store::save(&WorkflowManifest::from_def(&linear_def())).expect("save");
        let out = t
            .call(WorkflowArgs::Run {
                name: "pipeline".into(),
                team_id: "team-7".into(),
                input: String::new(),
            })
            .await;
        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
        let out = out.expect("run without signal must not panic");
        assert_eq!(out.task_ids.as_ref().map(|v| v.len()), Some(2));
    }

    #[tokio::test]
    async fn run_errors_on_missing_template() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }
        let res = t
            .call(WorkflowArgs::Run {
                name: "does-not-exist".into(),
                team_id: "team-7".into(),
                input: String::new(),
            })
            .await;
        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
        assert!(res.is_err(), "loading a missing template surfaces an error");
    }

    // --- status / cancel: run-grouped lifecycle (no ALEPH_HOME needed) ---

    /// Materialise `def` directly (bypassing the file store) and return the run.
    async fn materialize_run(
        t: &WorkflowTool,
        def: &WorkflowDef,
        team: &str,
    ) -> (String, Vec<String>) {
        let mat = workflow::materialize(def, "x", team, t.coord_store.as_ref(), None, None, None)
            .await
            .expect("materialise");
        (mat.run_id, mat.task_ids)
    }

    #[tokio::test]
    async fn status_reports_per_step_rows_for_run() {
        let store = setup_store().await;
        let t = tool(store, None);
        let (run_id, ids) = materialize_run(&t, &linear_def(), "team-9").await;

        // Fail the root with an error so the row surfaces it.
        t.coord_store
            .update_task(
                &ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Failed),
                    result: Some("provider exploded".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let out = t
            .call(WorkflowArgs::Status {
                name: "pipeline".into(),
                team_id: "team-9".into(),
                run_id: None,
            })
            .await
            .expect("status");
        assert_eq!(out.action, "status");
        assert_eq!(out.run_id.as_deref(), Some(run_id.as_str()));
        let steps = out.steps.as_ref().expect("status populates steps");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].step, "gather");
        assert_eq!(steps[0].status, "failed");
        assert_eq!(steps[0].error.as_deref(), Some("provider exploded"));
        // Root failed → dependent is terminally unsatisfiable (derived).
        assert_eq!(steps[1].step, "write");
        assert_eq!(steps[1].status, "unsatisfiable");
        assert!(steps[1].error.is_none(), "error only for failed steps");
        assert!(
            out.message.contains("1 failed"),
            "summary counts: {}",
            out.message
        );
    }

    #[tokio::test]
    async fn status_selects_explicit_run_among_several() {
        let store = setup_store().await;
        let t = tool(store, None);
        let (first_run, first_ids) = materialize_run(&t, &linear_def(), "team-9").await;
        let (second_run, _) = materialize_run(&t, &linear_def(), "team-9").await;
        assert_ne!(first_run, second_run);

        // Complete the first run's root so the two runs are distinguishable.
        t.coord_store
            .update_task(
                &first_ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let out = t
            .call(WorkflowArgs::Status {
                name: "pipeline".into(),
                team_id: "team-9".into(),
                run_id: Some(first_run.clone()),
            })
            .await
            .expect("status of explicit run");
        assert_eq!(out.run_id.as_deref(), Some(first_run.as_str()));
        let steps = out.steps.as_ref().unwrap();
        assert_eq!(
            steps[0].status, "completed",
            "rows come from the requested run"
        );

        // An unknown run id errors with guidance instead of guessing.
        let err = t
            .call(WorkflowArgs::Status {
                name: "pipeline".into(),
                team_id: "team-9".into(),
                run_id: Some("no-such-run".into()),
            })
            .await
            .expect_err("unknown run id");
        assert!(err.to_string().contains("no run 'no-such-run'"), "{err}");
    }

    #[tokio::test]
    async fn status_errors_when_workflow_never_ran() {
        let store = setup_store().await;
        let t = tool(store, None);
        let err = t
            .call(WorkflowArgs::Status {
                name: "ghost".into(),
                team_id: "team-9".into(),
                run_id: None,
            })
            .await
            .expect_err("no runs");
        assert!(
            err.to_string().contains("no runs of workflow 'ghost'"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn cancel_marks_unfinished_steps_cancelled_and_keeps_finished() {
        let store = setup_store().await;
        let t = tool(store, None);
        let (run_id, ids) = materialize_run(&t, &linear_def(), "team-9").await;

        // Root already finished; dependent now pending.
        t.coord_store
            .update_task(
                &ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    result: Some("done".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let out = t
            .call(WorkflowArgs::Cancel {
                name: "pipeline".into(),
                team_id: "team-9".into(),
                run_id: Some(run_id.clone()),
            })
            .await
            .expect("cancel");
        assert_eq!(out.action, "cancel");
        let cancelled = out.task_ids.as_ref().expect("cancel lists cancelled ids");
        assert_eq!(cancelled, &vec![ids[1].clone()], "only the unfinished step");
        assert!(
            out.message.contains("1 already finished"),
            "{}",
            out.message
        );

        // The finished step keeps its result; the rest is terminally cancelled.
        let root = t.coord_store.get_task(&ids[0]).await.unwrap().unwrap();
        assert_eq!(root.status, CoordTaskStatus::Completed);
        assert_eq!(root.result.as_deref(), Some("done"));
        let dep = t.coord_store.get_task(&ids[1]).await.unwrap().unwrap();
        assert_eq!(dep.status, CoordTaskStatus::Cancelled);

        // Cancel is idempotent: a second call finds nothing left to cancel.
        let again = t
            .call(WorkflowArgs::Cancel {
                name: "pipeline".into(),
                team_id: "team-9".into(),
                run_id: Some(run_id),
            })
            .await
            .expect("cancel again");
        assert_eq!(again.task_ids.as_deref(), Some(&[][..]));
        assert!(
            again.message.contains("cancelled 0 step(s)"),
            "{}",
            again.message
        );
    }

    #[tokio::test]
    async fn cancel_reports_in_flight_member_runs() {
        let store = setup_store().await;
        let t = tool(store, None);
        let (run_id, ids) = materialize_run(&t, &linear_def(), "team-9").await;
        t.coord_store
            .update_task(
                &ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::InProgress),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let out = t
            .call(WorkflowArgs::Cancel {
                name: "pipeline".into(),
                team_id: "team-9".into(),
                run_id: Some(run_id),
            })
            .await
            .expect("cancel with in-flight step");
        assert_eq!(out.task_ids.as_ref().map(|v| v.len()), Some(2));
        assert!(
            out.message.contains("1 in-flight member run(s)"),
            "in-flight semantics surfaced: {}",
            out.message
        );
        let root = t.coord_store.get_task(&ids[0]).await.unwrap().unwrap();
        assert_eq!(root.status, CoordTaskStatus::Cancelled);
    }

    // --- run pre-flight: team coverage validation ---

    /// In-memory team store seeded with `members` (each an in-process agent).
    /// Returns the store plus the freshly-minted team id to target.
    async fn team_with(members: &[&str]) -> (Arc<dyn crate::teams::TeamStore>, String) {
        // Trait in scope so create_team/add_member resolve on the concrete store.
        use crate::teams::types::{NewTeam, NewTeamMember};
        use crate::teams::TeamStore;
        let conn = Connection::open_in_memory().expect("open team db");
        let store = crate::teams::SqliteTeamStore::new(conn);
        store.migrate().await.expect("migrate teams");
        let team = store
            .create_team(NewTeam {
                name: "wf-team".into(),
                description: String::new(),
                leader_id: "leader".into(),
            })
            .await
            .expect("create team");
        for m in members {
            store
                .add_member(NewTeamMember::for_agent(team.id.clone(), *m, "member"))
                .await
                .expect("add member");
        }
        (Arc::new(store), team.id)
    }

    #[tokio::test]
    async fn preflight_passes_when_team_covers_all_agents() {
        let (team_store, team_id) = team_with(&["researcher", "writer"]).await;
        let store = setup_store().await;
        let t = tool(store, None).with_team_store(Some(team_store));
        t.preflight_team_coverage(&linear_def(), &team_id)
            .await
            .expect("team covers both agents → run is allowed");
    }

    #[tokio::test]
    async fn preflight_fails_naming_uncovered_agents() {
        // Team has the researcher but not the writer → run must be rejected
        // before any task is materialised, naming the missing agent and the
        // members that ARE present.
        let (team_store, team_id) = team_with(&["researcher"]).await;
        let store = setup_store().await;
        let t = tool(store, None).with_team_store(Some(team_store));
        let err = t
            .preflight_team_coverage(&linear_def(), &team_id)
            .await
            .expect_err("missing agent rejects the run");
        let msg = err.to_string();
        assert!(msg.contains("writer"), "names the uncovered agent: {msg}");
        assert!(msg.contains("researcher"), "lists current members: {msg}");
    }

    #[tokio::test]
    async fn preflight_reports_no_members_for_unknown_team() {
        // An unknown team id yields an empty roster, so every agent is missing
        // and the message says the team has no members.
        let (team_store, _real) = team_with(&["researcher", "writer"]).await;
        let store = setup_store().await;
        let t = tool(store, None).with_team_store(Some(team_store));
        let err = t
            .preflight_team_coverage(&linear_def(), "ghost-team")
            .await
            .expect_err("unknown team cannot cover any agent");
        assert!(
            err.to_string().contains("current members: none"),
            "empty roster reported: {err}"
        );
    }

    #[tokio::test]
    async fn preflight_exempts_clarify_step_agents() {
        // A clarify step is owned by the sentinel, not a team member, so its
        // (empty) agent must NOT be required. A team covering only the agent
        // step passes even though the clarify step's agent is unset.
        let def = WorkflowDef {
            name: "deploy".into(),
            description: String::new(),
            steps: vec![
                WorkflowStepDef {
                    id: "ask".into(),
                    agent: String::new(),
                    prompt: "Deploy where?".into(),
                    depends_on: vec![],
                    kind: crate::workflow::WorkflowStepKind::Clarify,
                    choices: vec!["staging".into(), "prod".into()],
                    review: false,
                },
                WorkflowStepDef {
                    id: "run".into(),
                    agent: "deployer".into(),
                    prompt: "deploy".into(),
                    depends_on: vec!["ask".into()],
                    kind: crate::workflow::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                },
            ],
        };
        let (team_store, team_id) = team_with(&["deployer"]).await;
        let store = setup_store().await;
        let t = tool(store, None).with_team_store(Some(team_store));
        t.preflight_team_coverage(&def, &team_id)
            .await
            .expect("clarify step's empty agent is exempt from coverage");
    }

    #[tokio::test]
    async fn preflight_noop_without_team_store() {
        // No team store wired → the check is a no-op (legacy behaviour). Even a
        // bogus team id passes, leaving the dispatcher to fail-fast later.
        let store = setup_store().await;
        let t = tool(store, None);
        t.preflight_team_coverage(&linear_def(), "whatever")
            .await
            .expect("no team store → coverage check skipped");
    }

    // --- export / import actions ---

    #[test]
    fn deserialize_export_defaults_write_file_false() {
        let args: WorkflowArgs =
            serde_json::from_value(serde_json::json!({"action":"export","name":"p"}))
                .expect("deserialise export");
        match args {
            WorkflowArgs::Export { name, write_file } => {
                assert_eq!(name, "p");
                assert!(!write_file, "write_file defaults to false");
            }
            other => panic!("expected Export, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_import_defaults_save_false() {
        let args: WorkflowArgs =
            serde_json::from_value(serde_json::json!({"action":"import","source":"x"}))
                .expect("deserialise import");
        match args {
            WorkflowArgs::Import { source, save } => {
                assert_eq!(source, "x");
                assert!(!save, "save defaults to false");
            }
            other => panic!("expected Import, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn export_renders_without_writing_then_import_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        // Capture both call results under the env guard, restore ALEPH_HOME,
        // then assert — so a failing assertion can't panic before restore and
        // leak a dead-TempDir env into the next guarded test.
        let (exported, imported) = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }

            workflow::store::save(&WorkflowManifest::from_def(&linear_def()))
                .expect("save template");

            // export (no write_file) populates `rendered`, not task_ids/definition.
            let exported = t
                .call(WorkflowArgs::Export {
                    name: "pipeline".into(),
                    write_file: false,
                })
                .await
                .expect("export");
            // import the rendered text back (no save) → definition equals the
            // core, dropped is empty for the lossless embedded path.
            let js = exported
                .rendered
                .clone()
                .expect("export populates rendered");
            let imported = t
                .call(WorkflowArgs::Import {
                    source: js,
                    save: false,
                })
                .await
                .expect("import");

            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            (exported, imported)
        };

        assert_eq!(exported.action, "export");
        let js = exported
            .rendered
            .as_ref()
            .expect("export populates rendered");
        assert!(js.contains("export const meta = {"));
        assert!(exported.task_ids.is_none() && exported.definition.is_none());

        assert_eq!(imported.action, "import");
        let def = imported
            .definition
            .as_ref()
            .expect("import populates definition");
        assert_eq!(def, &linear_def());
        assert_eq!(imported.dropped.as_deref(), Some(&[][..]));
    }

    #[tokio::test]
    async fn import_rich_manifest_then_export_reproduces_metadata() {
        // The headline fidelity guarantee: importing a rich AWI manifest
        // (per-step schema/model/phase + meta phases) with save=true, then
        // exporting it, reproduces that metadata — because the store now
        // persists the manifest superset, not just the executable core. (Bare
        // hand-written `.workflow.js` opts are out of scope for the scanner;
        // the lossless rich channels are manifest JSON and the embed block.)
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let rich_manifest_json = r#"{
  "name": "audit",
  "description": "two-phase audit",
  "whenToUse": "on any subsystem",
  "phases": [
    { "title": "Scan", "detail": "look", "model": "opus" },
    { "title": "Fix", "detail": "patch" }
  ],
  "steps": [
    { "id": "a", "agent": "scanner", "prompt": "scan {input}", "label": "scan:a", "phase": "Scan", "model": "haiku", "schema": {"type":"object"}, "isolation": "worktree", "agentType": "Explore" },
    { "id": "b", "agent": "fixer", "prompt": "fix it", "dependsOn": ["a"], "label": "fix:b", "phase": "Fix" }
  ]
}"#;

        let exported = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            // Import the rich AWI manifest JSON (lossless) and persist it.
            t.call(WorkflowArgs::Import {
                source: rich_manifest_json.into(),
                save: true,
            })
            .await
            .expect("import rich manifest + save");
            // Re-export from disk — must reproduce phases/schema/model/label.
            let exported = t
                .call(WorkflowArgs::Export {
                    name: "audit".into(),
                    write_file: false,
                })
                .await
                .expect("export");
            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            exported
        };

        let js = exported
            .rendered
            .as_ref()
            .expect("export populates rendered");
        // meta block carries whenToUse + both phases.
        assert!(
            js.contains("whenToUse: \"on any subsystem\""),
            "whenToUse: {js}"
        );
        assert!(
            js.contains("title: \"Scan\"") && js.contains("title: \"Fix\""),
            "phases: {js}"
        );
        // per-step metadata survived: schema, model, label, phase markers, plus
        // the engineering-format agent-opts isolation + agentType.
        assert!(js.contains("schema: {\"type\":\"object\"}"), "schema: {js}");
        assert!(js.contains("model: \"haiku\""), "model: {js}");
        assert!(js.contains("label: \"scan:a\""), "label: {js}");
        assert!(js.contains("isolation: \"worktree\""), "isolation: {js}");
        assert!(js.contains("agentType: \"Explore\""), "agentType: {js}");
        // the Scan phase carries its per-phase model override in the meta block.
        assert!(
            js.contains("title: \"Scan\", detail: \"look\", model: \"opus\""),
            "phase model: {js}"
        );
        assert!(
            js.contains("phase(\"Scan\")") && js.contains("phase(\"Fix\")"),
            "phase markers: {js}"
        );
    }

    #[tokio::test]
    async fn import_with_save_persists_template() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        // Capture both call results under the env guard, restore ALEPH_HOME,
        // then assert (see sibling roundtrip test for rationale).
        let (imported, listed) = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }

            let source = "export const meta = { name: 'scanned' }\nawait agent('do the thing')";
            let imported = t
                .call(WorkflowArgs::Import {
                    source: source.into(),
                    save: true,
                })
                .await
                .expect("import + save");
            let listed = t.call(WorkflowArgs::List {}).await.expect("list");

            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            (imported, listed)
        };

        assert!(imported.message.contains("imported"));
        assert_eq!(listed.names.as_deref(), Some(&["scanned".to_string()][..]));
    }

    #[tokio::test]
    async fn import_validate_failure_preserves_dropped_diagnostics() {
        // A bare scan can yield a structurally-invalid def (here: a whitespace
        // meta.name) while ALSO dropping imperative constructs. The error must
        // carry BOTH — the validation cause and the dropped note — so the lossy
        // import context isn't silently discarded by `?`. Pure parse/validate,
        // no store or ALEPH_HOME touched (save=false).
        let store = setup_store().await;
        let t = tool(store, None);
        let source = "export const meta = { name: '  ' }\n\
                      for (const x of items) { await agent('do thing') }";
        let err = t
            .call(WorkflowArgs::Import {
                source: source.into(),
                save: false,
            })
            .await
            .expect_err("whitespace name must fail validation");
        let msg = err.to_string();
        assert!(
            msg.contains("name must not be empty"),
            "validation cause: {msg}"
        );
        assert!(
            msg.contains("dropped"),
            "dropped diagnostics preserved: {msg}"
        );
        assert!(
            msg.contains("for loop"),
            "specific dropped construct named: {msg}"
        );
    }

    #[tokio::test]
    async fn export_missing_template_errors() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }
        let res = t
            .call(WorkflowArgs::Export {
                name: "ghost".into(),
                write_file: false,
            })
            .await;
        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
        assert!(res.is_err(), "exporting a missing template errors");
    }

    // --- file-backed lifecycle: save → list → describe → delete ---
    //
    // One combined #[tokio::test] keeps every ALEPH_HOME-touching assertion in
    // a single env scope, so there is no cross-test race on the process-global
    // var and the round-trip ordering is deterministic.
    #[tokio::test]
    async fn file_actions_lifecycle_and_output_shapes() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored at end of the test.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }

        // describe of an absent template errors.
        let missing = t
            .call(WorkflowArgs::Describe {
                name: "ghost".into(),
            })
            .await;
        assert!(missing.is_err(), "describe of missing template errors");

        // empty list before any save.
        let empty = t.call(WorkflowArgs::List {}).await.expect("list");
        assert_eq!(empty.action, "list");
        assert_eq!(empty.names.as_deref(), Some(&[][..]));
        assert!(empty.definition.is_none() && empty.task_ids.is_none());

        // save → only the message is shaped (no optionals).
        let saved = t
            .call(WorkflowArgs::Save {
                definition: linear_def(),
            })
            .await
            .expect("save");
        assert_eq!(saved.action, "save");
        assert!(saved.message.contains("pipeline"));
        assert!(saved.names.is_none() && saved.definition.is_none() && saved.task_ids.is_none());

        // list reflects the saved template — only names populated.
        let listed = t.call(WorkflowArgs::List {}).await.expect("list");
        assert_eq!(listed.names.as_deref(), Some(&["pipeline".to_string()][..]));
        assert!(listed.definition.is_none() && listed.task_ids.is_none());

        // describe round-trips the definition — only definition populated.
        let described = t
            .call(WorkflowArgs::Describe {
                name: "pipeline".into(),
            })
            .await
            .expect("describe");
        assert_eq!(described.action, "describe");
        let def = described
            .definition
            .as_ref()
            .expect("describe populates definition");
        assert_eq!(def, &linear_def());
        assert!(described.message.contains("2 step"));
        assert!(described.names.is_none() && described.task_ids.is_none());

        // serde wire shape: describe omits the None fields entirely.
        let wire = serde_json::to_value(&described).unwrap();
        assert!(wire.get("definition").is_some());
        assert!(
            wire.get("names").is_none(),
            "skip_serializing_if drops None names"
        );
        assert!(wire.get("task_ids").is_none());

        // delete present → "deleted" message; delete again → idempotent branch.
        let del1 = t
            .call(WorkflowArgs::Delete {
                name: "pipeline".into(),
            })
            .await
            .expect("delete present");
        assert!(del1.message.contains("deleted"));
        let del2 = t
            .call(WorkflowArgs::Delete {
                name: "pipeline".into(),
            })
            .await
            .expect("delete absent");
        assert!(
            del2.message.contains("did not exist"),
            "idempotent delete branch"
        );

        // after delete the list is empty again.
        let after = t.call(WorkflowArgs::List {}).await.expect("list");
        assert_eq!(after.names.as_deref(), Some(&[][..]));

        // SAFETY: guarded single mutator; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }
    }

    // --- proposals: the gate is reviewable before accept ---

    #[tokio::test]
    async fn describe_proposal_surfaces_steps_and_provenance() {
        // The gate's whole point is review-before-accept. A drafted proposal
        // lives in the `proposals/` dir, which plain `describe` (active store)
        // cannot see — `describe_proposal` reads it and surfaces both its steps
        // and the provenance the skeleton recorded in its description.
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let (described, missing) = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            // Draft a proposal exactly as the dream pipeline would.
            let draft =
                workflow::proposal::skeleton_from_chain(&["research".into(), "write".into()], 3)
                    .expect("two-skill chain drafts a skeleton");
            let name = draft.name.clone();
            workflow::proposal::save_proposal(&draft).expect("persist gated draft");

            let described = t
                .call(WorkflowArgs::DescribeProposal { name })
                .await
                .expect("describe the gated proposal");
            // An unknown proposal name errors rather than guessing.
            let missing = t
                .call(WorkflowArgs::DescribeProposal {
                    name: "metaskill-nope".into(),
                })
                .await;

            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            (described, missing)
        };

        assert_eq!(described.action, "describe_proposal");
        let def = described
            .definition
            .as_ref()
            .expect("describe_proposal populates definition");
        assert_eq!(def.steps.len(), 2, "both skills became steps");
        // Provenance (observation count) rides in the message + description.
        assert!(
            described.message.contains("3 observed"),
            "provenance surfaced: {}",
            described.message
        );
        assert!(missing.is_err(), "unknown proposal errors, not guessed");
    }

    #[test]
    fn with_planner_provider_builds() {
        // SqliteCoordTaskStore has no open(path) — use the same in-memory
        // pattern as setup_store() above (no migration needed for a build test).
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteCoordTaskStore::new(conn);
        let provider: Arc<dyn crate::providers::AiProvider> =
            Arc::new(crate::providers::MockProvider::new("x"));
        let _tool = WorkflowTool::new(Arc::new(store), None).with_planner_provider(Some(provider));
    }

    #[tokio::test]
    async fn workflow_no_provider_plans_no_strategy() {
        let store = setup_store().await;
        let t = tool(store, None); // no planner provider injected
        let strategy = t.plan_workflow_strategy(&linear_def(), "do the thing").await;
        assert!(strategy.is_none(), "no provider => no strategy planned");
    }

    #[tokio::test]
    async fn workflow_with_provider_plans_concrete_strategy() {
        let store = setup_store().await;
        let json = r#"{"objective":"o","approach":"a","phases":["p"],
            "guardrails":["do not touch the billing module"],"success_criteria":"done"}"#;
        let provider: Arc<dyn crate::providers::AiProvider> =
            Arc::new(crate::providers::MockProvider::new(json));
        let t = tool(store, None).with_planner_provider(Some(provider));
        let strategy = t.plan_workflow_strategy(&linear_def(), "do the thing").await;
        assert!(strategy.is_some(), "provider + concrete guardrail => Some(strategy)");
    }
}
