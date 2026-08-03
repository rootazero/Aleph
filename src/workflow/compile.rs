//! Compile a [`WorkflowDef`] into runnable `coord_tasks`.
//!
//! This is the whole point of the workflow layer: a declarative template is
//! *materialised* into the existing coordination-task DAG, then executed by
//! the existing [`TeamDispatcher`](crate::teams::dispatcher::TeamDispatcher).
//! No new scheduler, no reasoning — a pure mapping (R10 / R7 safe):
//!
//! - each [`WorkflowStepDef`] → one `coord_task` owned by `step.agent`
//! - `step.depends_on` → `coord_task.blocked_by` (cycle-checked by the store)
//! - tasks are tagged `{"managed_by": "dispatcher"}` so the autonomous loop
//!   picks them up; upstream step outputs flow into each step automatically
//!   via the dispatcher's `build_handoff_context`.
//!
//! Tasks are created in topological order so each `blocked_by` references an
//! already-minted task id.

use serde_json::json;

use crate::agents::swarm::tasks::acceptance::LEAD_REVIEW_METADATA_KEY;
use crate::agents::swarm::tasks::{
    CoordTaskId, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, NewCoordTask, Priority,
};
use crate::error::Result;
use crate::strategy::{render_workflow_global_frame, Strategy};
use crate::teams::dispatcher::{MANAGED_BY_DISPATCHER, MANAGED_BY_KEY};
use crate::workflow::clarify::{ClarifyContext, ClarifyTaskMeta, CLARIFY_META_KEY, CLARIFY_OWNER};
use crate::workflow::def::{render_prompt, WorkflowDef};

/// Metadata key carrying the workflow template name on every materialised task.
pub const WORKFLOW_NAME_KEY: &str = "workflow";
/// Metadata key carrying the step-local id on every materialised task.
pub const WORKFLOW_STEP_KEY: &str = "workflow_step";
/// Metadata key carrying the per-run identity on every materialised task.
/// Two runs of the same template on the same team are only distinguishable by
/// this id — it is what `workflow(action='status'|'cancel')` groups on.
pub const WORKFLOW_RUN_ID_KEY: &str = "workflow_run_id";
/// Metadata key carrying a step's per-step model override (when the template's
/// AWI manifest set one). The dispatcher reads it via [`workflow_model_override`]
/// and turns it into a `RunRequest.model_override` so the member run executes on
/// the requested model — the executable wiring of the manifest's `model` field.
/// Absent on steps with no override (byte-identical legacy rows).
pub const WORKFLOW_MODEL_KEY: &str = "workflow_model";
/// Metadata key carrying a step's per-step reasoning-effort override (when the
/// template's AWI manifest set one, e.g. `"low"`/`"max"`). The dispatcher reads
/// it via [`workflow_effort_think_level`] and threads it into the member run's
/// `RunRequest.metadata["think_level"]` — the executable wiring of the
/// manifest's `effort` field, exactly mirroring [`WORKFLOW_MODEL_KEY`].
/// Absent on steps with no override (byte-identical legacy rows).
pub const WORKFLOW_EFFORT_KEY: &str = "workflow_effort";
/// Metadata key carrying the run-global welded strategy frame on every
/// materialised **agent** step. Stamped once per run (beside [`WORKFLOW_RUN_ID_KEY`])
/// from the planned [`Strategy`](crate::strategy::Strategy) via
/// [`render_workflow_global_frame`](crate::strategy::render_workflow_global_frame);
/// `build_handoff_context` renders it as a `## Global Strategy` section after the
/// task block. Absent when no strategy was planned (byte-identical legacy rows).
/// Clarify steps run no agent, so they are never stamped.
pub const WORKFLOW_STRATEGY_KEY: &str = "workflow_strategy";
/// Metadata key carrying the originating channel address
/// (`{"channel_id", "conversation_id"}`) on every materialised task of an
/// interactively-launched run. The dispatcher's settle sweep
/// (`notify_settled_workflow_runs`) reads it to push the run's terminal
/// summary back to the user's channel (R5 — autonomous terminal states never
/// die silently). Absent for non-interactive runs (byte-identical legacy rows).
pub const WORKFLOW_ORIGIN_KEY: &str = "workflow_origin";
/// Metadata key marking a run whose terminal notification has already been
/// delivered (or deliberately suppressed — the `workflow` tool's `cancel`
/// stamps it because the cancelling user already knows). Stamped on one task
/// of the run with the epoch-seconds stamp time; its presence on ANY task
/// silences the settle sweep, making the notification once-only across daemon
/// restarts. The sweep clears it (re-arms) when a marked run is reopened by a
/// step retry, grace-gated on the stamp's age so a mid-cancel window is never
/// mistaken for a reopen.
pub const WORKFLOW_NOTIFIED_KEY: &str = "workflow_notified";

/// Read the originating channel address stamped on a materialised workflow
/// task under [`WORKFLOW_ORIGIN_KEY`]. Returns `(channel_id, conversation_id)`;
/// `None` for legacy rows or non-interactive runs. Pure.
#[must_use]
pub fn workflow_origin(metadata: &serde_json::Value) -> Option<(String, String)> {
    let origin = metadata.get(WORKFLOW_ORIGIN_KEY)?;
    let channel = origin.get("channel_id")?.as_str()?.trim();
    let conversation = origin.get("conversation_id")?.as_str()?.trim();
    if channel.is_empty() || conversation.is_empty() {
        return None;
    }
    Some((channel.to_string(), conversation.to_string()))
}

/// Read a step's per-step model override off its materialised `coord_task`
/// metadata and build a [`ModelOverride`](crate::gateway::model_override::ModelOverride).
///
/// `"provider/model"` pins both (Qualified); a bare `"model"` is Raw and lets the
/// provider registry resolve the provider. Empty / missing → `None` (the run uses
/// the agent's default model). Pure — the dispatcher calls it at launch time.
#[must_use]
pub fn workflow_model_override(
    metadata: &serde_json::Value,
) -> Option<crate::gateway::model_override::ModelOverride> {
    use crate::gateway::model_override::ModelOverride;
    let raw = metadata.get(WORKFLOW_MODEL_KEY).and_then(|v| v.as_str())?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.split_once('/') {
        Some((provider, model)) if !provider.trim().is_empty() && !model.trim().is_empty() => {
            Some(ModelOverride::Qualified {
                provider: provider.trim().to_string(),
                model: model.trim().to_string(),
            })
        }
        _ => Some(ModelOverride::Raw {
            model: raw.to_string(),
        }),
    }
}

/// Read a step's per-step reasoning-effort override off its materialised
/// `coord_task` metadata and normalise it against the live think-level
/// vocabulary (`normalize_think_level` — the same table the run-time channel
/// consumes, so `"max"` maps to `High` here exactly as it would in a turn).
/// Missing / unknown → `None` (the run keeps the session's default depth).
/// Pure — the dispatcher calls it at launch time.
#[must_use]
pub fn workflow_effort_think_level(
    metadata: &serde_json::Value,
) -> Option<crate::agents::thinking::ThinkLevel> {
    let raw = metadata.get(WORKFLOW_EFFORT_KEY).and_then(|v| v.as_str())?;
    crate::agents::thinking::normalize_think_level(raw)
}

/// The set of `coord_task` ids minted for one workflow run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedWorkflow {
    /// Identity of this run, stamped into every task's metadata under
    /// [`WORKFLOW_RUN_ID_KEY`].
    pub run_id: String,
    /// Task ids in creation (topological) order.
    pub task_ids: Vec<CoordTaskId>,
}

/// Materialise `def` into `coord_tasks` under `team_id`, substituting `input`
/// into each step's prompt. Returns the created task ids in topological order.
///
/// The caller is responsible for ensuring `team_id` refers to a team whose
/// members cover every agent step's `step.agent` (create one with `team_create`
/// first). After this returns, signal the dispatcher (or let its fallback tick
/// fire) to begin execution.
///
/// `clarify_ctx` carries the originating channel address captured at run start;
/// it is stamped into every clarify step so the dispatcher knows where to push
/// the question and the inbound router knows which session's reply completes it.
/// When `None` (e.g. a non-interactive run), clarify steps still materialise but
/// have no channel to reach — the dispatcher fails them with a clear reason
/// rather than stalling the DAG.
///
/// `models` maps step-local id → model override (e.g. `"opus"`,
/// `"openai/gpt-5"`); a present entry is stamped under [`WORKFLOW_MODEL_KEY`] on
/// that agent step so its member run executes on the requested model. `None` (or
/// a step with no entry) leaves the run on the agent's default model — the
/// pre-existing behaviour. Clarify steps run no agent, so they are never stamped.
///
/// `efforts` maps step-local id → reasoning-effort override (`low`..`max`),
/// stamped under [`WORKFLOW_EFFORT_KEY`] with the exact same contract as
/// `models` (agent steps only, absent entries byte-identical).
///
/// `origin_session` is the launching session's key captured at run start (the
/// tool's turn context); it is stamped on every task as the `origin_session`
/// goal-budget anchor so autonomously-dispatched member runs enroll into the
/// creating session's goal tree budget — the same anchor `task_create` and the
/// workflow canvas stamp. `None` (non-interactive run) leaves rows
/// byte-identical and the children run unaccounted, exactly as before.
pub async fn materialize(
    def: &WorkflowDef,
    input: &str,
    team_id: &str,
    store: &dyn CoordTaskStore,
    clarify_ctx: Option<&ClarifyContext>,
    models: Option<&std::collections::HashMap<String, String>>,
    efforts: Option<&std::collections::HashMap<String, String>>,
    strategy: Option<&Strategy>,
    origin_session: Option<&str>,
) -> Result<MaterializedWorkflow> {
    def.validate()?;
    let order = def.topo_order()?;

    // One identity per materialisation: stamped into every task so the run
    // can be observed and cancelled as a unit after the launching turn ends.
    let run_id = uuid::Uuid::new_v4().to_string();

    // Run-wide welded strategy frame, rendered once and stamped onto every
    // agent step's metadata. `None` (no planned strategy) leaves rows
    // byte-identical to the legacy materialisation.
    let strategy_frame: Option<String> = strategy.map(render_workflow_global_frame);

    // step-local id → freshly-minted coord_task id.
    let mut id_map: std::collections::HashMap<&str, CoordTaskId> =
        std::collections::HashMap::with_capacity(def.steps.len());
    let mut task_ids = Vec::with_capacity(def.steps.len());

    for &idx in &order {
        let step = &def.steps[idx];

        // depends_on resolves to already-created task ids because we iterate
        // in topological order — a dependency is always materialised first.
        // De-duplicate: a step listing the same dependency twice would emit a
        // duplicate `(task_id, depends_on)` edge, which violates the dependency
        // table's PRIMARY KEY and aborts `create_task`. `validate()` permits
        // duplicate `depends_on` (semantically a no-op), so collapse them here.
        let mut blocked_by: Vec<CoordTaskId> = Vec::with_capacity(step.depends_on.len());
        for dep in &step.depends_on {
            let Some(dep_id) = id_map.get(dep.as_str()).cloned() else {
                cancel_partial(store, &task_ids).await;
                return Err(crate::error::AlephError::invalid_input(format!(
                    "internal: dependency '{dep}' of step '{}' not yet materialised",
                    step.id
                )));
            };
            if !blocked_by.contains(&dep_id) {
                blocked_by.push(dep_id);
            }
        }

        // The rendered prompt doubles as the clarify question.
        let rendered = render_prompt(&step.prompt, input);

        // A clarify step is owned by the sentinel and carries its awaiting
        // record in metadata; an agent step is owned by its agent. Both keep the
        // dispatcher-managed + workflow provenance tags.
        let (owner, metadata) = if step.is_clarify() {
            let ctx = clarify_ctx.cloned().unwrap_or_default();
            let clarify_meta = ClarifyTaskMeta {
                question: rendered.clone(),
                choices: step.choices.clone(),
                channel_id: ctx.channel_id,
                conversation_id: ctx.conversation_id,
                session_key: ctx.session_key,
            };
            (
                CLARIFY_OWNER.to_string(),
                json!({
                    MANAGED_BY_KEY: MANAGED_BY_DISPATCHER,
                    WORKFLOW_NAME_KEY: def.name,
                    WORKFLOW_STEP_KEY: step.id,
                    WORKFLOW_RUN_ID_KEY: run_id,
                    CLARIFY_META_KEY: clarify_meta.to_value(),
                }),
            )
        } else {
            let mut meta = json!({
                MANAGED_BY_KEY: MANAGED_BY_DISPATCHER,
                WORKFLOW_NAME_KEY: def.name,
                WORKFLOW_STEP_KEY: step.id,
                WORKFLOW_RUN_ID_KEY: run_id,
            });
            // Review-gated step: stamp the flag the dispatcher reads at
            // completion time to park the run in WaitingReview instead of
            // Completed. Absent for non-reviewed steps (byte-identical rows).
            if step.review {
                if let Some(obj) = meta.as_object_mut() {
                    obj.insert(LEAD_REVIEW_METADATA_KEY.to_string(), json!(true));
                    // The anchor demand rides the SAME metadata channel
                    // `task_create(require_grounding=…)` writes, so the review
                    // tools' existing bounce reads it with zero new plumbing.
                    // `validate()` already refuses grounding without review, so
                    // this is the only place it can be stamped.
                    if step.require_grounding {
                        obj.insert(
                            crate::agents::swarm::tasks::acceptance::REQUIRE_GROUNDING_METADATA_KEY
                                .to_string(),
                            json!(true),
                        );
                    }
                }
            }
            // Per-step model override (from the AWI manifest): stamp the model
            // string the dispatcher turns into a RunRequest.model_override at
            // launch. Absent when the step has no override (byte-identical rows).
            if let Some(model) = models
                .and_then(|m| m.get(step.id.as_str()))
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                if let Some(obj) = meta.as_object_mut() {
                    obj.insert(WORKFLOW_MODEL_KEY.to_string(), json!(model));
                }
            }
            // Per-step reasoning-effort override (from the AWI manifest): same
            // contract as the model stamp above — the dispatcher reads it via
            // `workflow_effort_think_level` and threads it into the member
            // run's think-level channel. Absent → byte-identical rows.
            if let Some(effort) = efforts
                .and_then(|m| m.get(step.id.as_str()))
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                if let Some(obj) = meta.as_object_mut() {
                    obj.insert(WORKFLOW_EFFORT_KEY.to_string(), json!(effort));
                }
            }
            // Run-global strategy frame: the same welded objective + cross-cutting
            // guardrails on every agent step (the DAG itself is the phase
            // structure, so no phase list). Absent when no strategy was planned.
            if let Some(frame) = strategy_frame.as_deref() {
                if let Some(obj) = meta.as_object_mut() {
                    obj.insert(WORKFLOW_STRATEGY_KEY.to_string(), json!(frame));
                }
            }
            // Per-step execution-budget overrides: stamped through the SAME
            // metadata helpers `task_create` uses, so the dispatcher's
            // existing consumers (`effective_timeout_secs` at launch,
            // `read_max_retries` on failure) pick them up with zero new
            // plumbing. `None` leaves the row byte-identical (helpers are
            // no-ops on None).
            let meta =
                crate::agents::swarm::tasks::timeout::with_task_timeout(meta, step.timeout_seconds);
            let meta = crate::agents::swarm::tasks::retry::with_max_retries(meta, step.max_retries);
            (step.agent.clone(), meta)
        };

        // Origin stamp: the originating channel captured at run start rides on
        // EVERY task (agent and clarify alike) so the dispatcher's settle sweep
        // can push the run's terminal summary back to the user (R5). Absent
        // when the run was launched non-interactively (byte-identical legacy
        // rows) — the sweep then stays silent for this run.
        let metadata = {
            let mut metadata = metadata;
            if let Some(ctx) = clarify_ctx {
                if !ctx.channel_id.is_empty() && !ctx.conversation_id.is_empty() {
                    if let Some(obj) = metadata.as_object_mut() {
                        obj.insert(
                            WORKFLOW_ORIGIN_KEY.to_string(),
                            json!({
                                "channel_id": ctx.channel_id,
                                "conversation_id": ctx.conversation_id,
                            }),
                        );
                    }
                }
            }
            // Goal-tree budget anchor: the launching session captured at run
            // start rides on every task so the dispatcher's member runs enroll
            // into the creating session's goal tree budget (same key as
            // `task_create` / the workflow canvas). Absent when the run has no
            // session context (byte-identical legacy rows) — the children then
            // run unaccounted, exactly as before this anchor existed.
            if let Some(session) = origin_session.map(str::trim).filter(|s| !s.is_empty()) {
                if let Some(obj) = metadata.as_object_mut() {
                    obj.insert(
                        crate::gateway::goal_budget::ORIGIN_SESSION_METADATA_KEY.to_string(),
                        json!(session),
                    );
                }
            }
            metadata
        };

        let created = match store
            .create_task(NewCoordTask {
                team_id: Some(team_id.to_string()),
                subject: format!("{}:{}", def.name, step.id),
                description: rendered,
                owner: Some(owner),
                priority: Priority::Normal,
                blocked_by,
                metadata,
            })
            .await
        {
            Ok(created) => created,
            Err(e) => {
                // A mid-loop failure leaves the steps created so far as live,
                // dispatcher-managed tasks. Cancel them best-effort so a failed
                // run does not execute a half-materialised workflow.
                cancel_partial(store, &task_ids).await;
                return Err(e);
            }
        };

        id_map.insert(step.id.as_str(), created.id.clone());
        task_ids.push(created.id);
    }

    Ok(MaterializedWorkflow { run_id, task_ids })
}

/// Best-effort rollback: mark already-created tasks `Cancelled` so a failed
/// partial materialisation leaves no live dispatcher-managed orphans. Errors
/// are swallowed — we are already on an error path and a terminal status is the
/// most we can guarantee without a batch/transaction API on the store.
async fn cancel_partial(store: &dyn CoordTaskStore, ids: &[CoordTaskId]) {
    for id in ids {
        let _ = store
            .update_task(
                id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Cancelled),
                    ..Default::default()
                },
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::{store::SqliteCoordTaskStore, CoordTaskStatus};
    use crate::workflow::def::WorkflowStepDef;
    use rusqlite::Connection;

    async fn setup_store() -> SqliteCoordTaskStore {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.expect("migrate");
        store
    }

    fn step(id: &str, agent: &str, deps: &[&str]) -> WorkflowStepDef {
        WorkflowStepDef {
            id: id.into(),
            agent: agent.into(),
            prompt: format!("handle {{input}} for {id}"),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            kind: crate::workflow::def::WorkflowStepKind::Agent,
            choices: vec![],
            review: false,
            require_grounding: false,
            timeout_seconds: None,
            max_retries: None,
        }
    }

    fn clarify_step(id: &str, question: &str, choices: &[&str], deps: &[&str]) -> WorkflowStepDef {
        WorkflowStepDef {
            id: id.into(),
            agent: String::new(),
            prompt: question.into(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            kind: crate::workflow::def::WorkflowStepKind::Clarify,
            choices: choices.iter().map(|s| s.to_string()).collect(),
            review: false,
            require_grounding: false,
            timeout_seconds: None,
            max_retries: None,
        }
    }

    fn linear_def() -> WorkflowDef {
        WorkflowDef {
            name: "pipeline".into(),
            description: String::new(),
            steps: vec![
                step("gather", "researcher", &[]),
                step("write", "writer", &["gather"]),
            ],
        }
    }

    #[tokio::test]
    async fn materialize_creates_one_task_per_step() {
        let store = setup_store().await;
        let mat = materialize(
            &linear_def(),
            "the topic",
            "team-1",
            &store,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("materialise");
        assert_eq!(mat.task_ids.len(), 2);
    }

    #[tokio::test]
    async fn materialize_substitutes_input_and_tags_dispatcher() {
        let store = setup_store().await;
        let mat = materialize(
            &linear_def(),
            "quantum computing",
            "team-1",
            &store,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let first = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        assert_eq!(first.subject, "pipeline:gather");
        assert_eq!(first.description, "handle quantum computing for gather");
        assert_eq!(first.owner.as_deref(), Some("researcher"));
        assert_eq!(
            first.metadata.get(MANAGED_BY_KEY).and_then(|v| v.as_str()),
            Some(MANAGED_BY_DISPATCHER)
        );
        assert_eq!(
            first.metadata.get("workflow_step").and_then(|v| v.as_str()),
            Some("gather")
        );
    }

    #[tokio::test]
    async fn materialize_stamps_one_run_id_on_every_task() {
        let store = setup_store().await;
        let first = materialize(
            &linear_def(),
            "x",
            "team-1",
            &store,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(!first.run_id.is_empty(), "run id is minted");
        for id in &first.task_ids {
            let task = store.get_task(id).await.unwrap().unwrap();
            assert_eq!(
                task.metadata
                    .get(WORKFLOW_RUN_ID_KEY)
                    .and_then(|v| v.as_str()),
                Some(first.run_id.as_str()),
                "every task carries the run id"
            );
        }
        // A second run of the same template mints a distinct identity.
        let second = materialize(
            &linear_def(),
            "x",
            "team-1",
            &store,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_ne!(first.run_id, second.run_id, "runs are distinguishable");
    }

    #[tokio::test]
    async fn materialize_wires_dependency_so_dependent_is_blocked() {
        let store = setup_store().await;
        let mat = materialize(
            &linear_def(),
            "x",
            "team-1",
            &store,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // task_ids[0] is "gather" (root), [1] is "write" (depends on gather).
        let root = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        let dependent = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert_eq!(root.status, CoordTaskStatus::Pending, "root has no deps");
        assert_eq!(
            dependent.status,
            CoordTaskStatus::Blocked,
            "dependent waits on gather"
        );

        // Completing the root unblocks the dependent.
        store
            .update_task(
                &mat.task_ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let after = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert_eq!(after.status, CoordTaskStatus::Pending);
    }

    #[tokio::test]
    async fn materialize_collapses_duplicate_dependency() {
        // A step listing the same dependency twice must NOT emit a duplicate
        // (task_id, depends_on) edge — that would hit the dependency table's
        // PRIMARY KEY and abort materialisation. `validate()` allows the
        // duplicate (it is semantically a no-op), so the compiler collapses it.
        let store = setup_store().await;
        let def = WorkflowDef {
            name: "dup".into(),
            description: String::new(),
            steps: vec![step("a", "w", &[]), step("b", "w", &["a", "a"])],
        };
        let mat = materialize(&def, "x", "t", &store, None, None, None, None, None)
            .await
            .expect("duplicate dep collapses instead of aborting");
        assert_eq!(mat.task_ids.len(), 2);
        let dependent = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert_eq!(dependent.subject, "dup:b");
        assert_eq!(dependent.status, CoordTaskStatus::Blocked);
    }

    #[tokio::test]
    async fn materialize_rejects_invalid_def() {
        let store = setup_store().await;
        let mut def = linear_def();
        def.steps[1].depends_on = vec!["ghost".into()];
        assert!(
            materialize(&def, "x", "team-1", &store, None, None, None, None, None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn materialize_diamond_orders_dependencies_first() {
        let store = setup_store().await;
        let def = WorkflowDef {
            name: "diamond".into(),
            description: String::new(),
            steps: vec![
                step("a", "w", &[]),
                step("b", "w", &["a"]),
                step("c", "w", &["a"]),
                step("d", "w", &["b", "c"]),
            ],
        };
        let mat = materialize(&def, "x", "t", &store, None, None, None, None, None)
            .await
            .unwrap();
        assert_eq!(mat.task_ids.len(), 4);
        // The final task "d" must be blocked until both b and c complete.
        let last = store
            .get_task(mat.task_ids.last().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(last.subject, "diamond:d");
        assert_eq!(last.status, CoordTaskStatus::Blocked);
    }

    #[tokio::test]
    async fn materialize_clarify_step_stamps_owner_and_meta() {
        use crate::workflow::clarify::{ClarifyContext, ClarifyTaskMeta, CLARIFY_OWNER};
        let store = setup_store().await;
        let def = WorkflowDef {
            name: "deploy".into(),
            description: String::new(),
            steps: vec![
                clarify_step("ask", "Deploy to {input}?", &["staging", "prod"], &[]),
                step("run", "deployer", &["ask"]),
            ],
        };
        let ctx = ClarifyContext {
            channel_id: "telegram".into(),
            conversation_id: "user-1".into(),
            session_key: "telegram:bot:1:user-1".into(),
        };
        let mat = materialize(
            &def,
            "us-east",
            "team-1",
            &store,
            Some(&ctx),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let ask = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        // Owned by the sentinel — never routed to a team member.
        assert_eq!(ask.owner.as_deref(), Some(CLARIFY_OWNER));
        // The awaiting record carries the rendered question, choices, and the
        // originating channel address.
        let meta = ClarifyTaskMeta::from_metadata(&ask.metadata).expect("clarify meta present");
        assert_eq!(meta.question, "Deploy to us-east?");
        assert_eq!(meta.choices, vec!["staging", "prod"]);
        assert_eq!(meta.channel_id, "telegram");
        assert_eq!(meta.session_key, "telegram:bot:1:user-1");

        // The downstream agent step waits on the clarify answer.
        let run = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert_eq!(run.status, CoordTaskStatus::Blocked);
    }

    #[tokio::test]
    async fn materialize_review_step_stamps_lead_review_flag() {
        use crate::agents::swarm::tasks::acceptance::lead_review_required;
        let store = setup_store().await;
        let mut def = linear_def();
        def.steps[1].review = true;
        let mat = materialize(&def, "x", "team-1", &store, None, None, None, None, None)
            .await
            .unwrap();

        // Non-reviewed step: no flag key at all (byte-identical to legacy rows).
        let first = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        assert!(first.metadata.get(LEAD_REVIEW_METADATA_KEY).is_none());
        assert!(!lead_review_required(&first.metadata));

        // Reviewed step: flag stamped true alongside the dispatcher marker.
        let second = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert!(lead_review_required(&second.metadata));
        assert_eq!(
            second.metadata.get(MANAGED_BY_KEY).and_then(|v| v.as_str()),
            Some(MANAGED_BY_DISPATCHER)
        );
    }

    #[tokio::test]
    async fn materialize_clarify_without_context_has_empty_address() {
        use crate::workflow::clarify::ClarifyTaskMeta;
        let store = setup_store().await;
        let def = WorkflowDef {
            name: "wf".into(),
            description: String::new(),
            steps: vec![clarify_step("ask", "Which file?", &[], &[])],
        };
        let mat = materialize(&def, "x", "t", &store, None, None, None, None, None)
            .await
            .unwrap();
        let ask = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        let meta = ClarifyTaskMeta::from_metadata(&ask.metadata).expect("clarify meta present");
        assert!(meta.channel_id.is_empty());
        assert!(meta.session_key.is_empty());
    }

    #[tokio::test]
    async fn materialize_stamps_per_step_model_override() {
        // A model map keyed by step id stamps WORKFLOW_MODEL_KEY onto exactly
        // the matching agent step; an unlisted step stays byte-identical (no
        // key), so non-overridden runs keep the agent's default model.
        let store = setup_store().await;
        let mut models = std::collections::HashMap::new();
        models.insert("gather".to_string(), "opus".to_string());
        let mat = materialize(
            &linear_def(),
            "x",
            "team-1",
            &store,
            None,
            Some(&models),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let gather = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        assert_eq!(
            gather
                .metadata
                .get(WORKFLOW_MODEL_KEY)
                .and_then(|v| v.as_str()),
            Some("opus"),
            "listed step carries its model override"
        );
        let write = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert!(
            write.metadata.get(WORKFLOW_MODEL_KEY).is_none(),
            "unlisted step has no override key (legacy byte-identical row)"
        );
    }

    #[test]
    fn workflow_model_override_parses_raw_and_qualified() {
        use crate::gateway::model_override::ModelOverride;
        // Bare model → Raw (registry resolves the provider).
        let raw = workflow_model_override(&json!({ WORKFLOW_MODEL_KEY: "opus" }));
        assert_eq!(
            raw,
            Some(ModelOverride::Raw {
                model: "opus".into()
            })
        );
        // "provider/model" → Qualified (both pinned).
        let qual = workflow_model_override(&json!({ WORKFLOW_MODEL_KEY: "openai/gpt-5" }));
        assert_eq!(
            qual,
            Some(ModelOverride::Qualified {
                provider: "openai".into(),
                model: "gpt-5".into(),
            })
        );
        // Missing / empty → None (run stays on the default model).
        assert!(workflow_model_override(&json!({})).is_none());
        assert!(workflow_model_override(&json!({ WORKFLOW_MODEL_KEY: "  " })).is_none());
    }

    #[tokio::test]
    async fn materialize_stamps_per_step_effort_override() {
        // An effort map keyed by step id stamps WORKFLOW_EFFORT_KEY onto exactly
        // the matching agent step; an unlisted step stays byte-identical (no
        // key) — the exact WORKFLOW_MODEL_KEY contract.
        let store = setup_store().await;
        let mut efforts = std::collections::HashMap::new();
        efforts.insert("gather".to_string(), "max".to_string());
        let mat = materialize(
            &linear_def(),
            "x",
            "team-1",
            &store,
            None,
            None,
            Some(&efforts),
            None,
            None,
        )
        .await
        .unwrap();

        let gather = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        assert_eq!(
            gather
                .metadata
                .get(WORKFLOW_EFFORT_KEY)
                .and_then(|v| v.as_str()),
            Some("max"),
            "listed step carries its effort override"
        );
        // The dispatcher-side reader normalises the .workflow.js vocabulary
        // through the live think-level table ("max" → High).
        assert_eq!(
            workflow_effort_think_level(&gather.metadata),
            Some(crate::agents::thinking::ThinkLevel::High)
        );
        let write = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert!(
            write.metadata.get(WORKFLOW_EFFORT_KEY).is_none(),
            "unlisted step has no override key (legacy byte-identical row)"
        );
        assert!(workflow_effort_think_level(&write.metadata).is_none());
    }

    #[test]
    fn workflow_effort_think_level_normalizes_and_rejects() {
        use crate::agents::thinking::ThinkLevel;
        assert_eq!(
            workflow_effort_think_level(&json!({ WORKFLOW_EFFORT_KEY: "low" })),
            Some(ThinkLevel::Low)
        );
        assert_eq!(
            workflow_effort_think_level(&json!({ WORKFLOW_EFFORT_KEY: "xhigh" })),
            Some(ThinkLevel::XHigh)
        );
        // Missing / unknown → None (run keeps the default depth).
        assert!(workflow_effort_think_level(&json!({})).is_none());
        assert!(workflow_effort_think_level(&json!({ WORKFLOW_EFFORT_KEY: "turbo" })).is_none());
    }

    #[tokio::test]
    async fn materialize_stamps_strategy_frame_on_agent_steps_only() {
        let store = setup_store().await;
        let strategy = crate::strategy::Strategy {
            objective: "ship the pipeline".into(),
            approach: "incremental".into(),
            phases: vec!["phase a".into(), "phase b".into()],
            guardrails: vec!["no network in tests".into()],
            success_criteria: "all green".into(),
            goal_id: None,
        };
        let mut def = linear_def();
        def.steps
            .push(clarify_step("ask", "which mode?", &["A", "B"], &["gather"]));

        let mat = materialize(
            &def,
            "x",
            "team-1",
            &store,
            None,
            None,
            None,
            Some(&strategy),
            None,
        )
        .await
        .unwrap();

        let mut saw_agent_stamp = false;
        let mut saw_clarify_stamp = false;
        for id in &mat.task_ids {
            let task = store.get_task(id).await.unwrap().unwrap();
            let stamped = task
                .metadata
                .get(WORKFLOW_STRATEGY_KEY)
                .and_then(|v| v.as_str());
            if task.owner.as_deref() == Some(CLARIFY_OWNER) {
                saw_clarify_stamp |= stamped.is_some();
            } else if let Some(frame) = stamped {
                saw_agent_stamp = true;
                // Global frame = objective + guardrails, NO phase list.
                assert!(frame.contains("ship the pipeline"));
                assert!(!frame.contains("phase a"));
            }
        }
        assert!(saw_agent_stamp, "agent steps must carry the strategy frame");
        assert!(!saw_clarify_stamp, "clarify steps must NOT be stamped");
    }

    #[tokio::test]
    async fn materialize_stamps_per_step_timeout_and_retries() {
        use crate::agents::swarm::tasks::retry::read_max_retries;
        use crate::agents::swarm::tasks::timeout::effective_timeout_secs;
        let store = setup_store().await;
        let mut def = linear_def();
        def.steps[0].timeout_seconds = Some(1800);
        def.steps[0].max_retries = Some(0);
        let mat = materialize(&def, "x", "team-1", &store, None, None, None, None, None)
            .await
            .unwrap();

        // The overridden step carries both keys, readable through the exact
        // dispatcher-side consumers.
        let gather = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        assert_eq!(effective_timeout_secs(&gather.metadata, 600), 1800);
        assert_eq!(read_max_retries(&gather.metadata), Some(0));

        // The unlisted step stays byte-identical (global defaults apply).
        let write = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert!(write.metadata.get("timeout_secs").is_none());
        assert!(write.metadata.get("max_retries").is_none());
        assert_eq!(effective_timeout_secs(&write.metadata, 600), 600);
    }

    #[tokio::test]
    async fn materialize_stamps_origin_on_every_task() {
        use crate::workflow::clarify::ClarifyContext;
        let store = setup_store().await;
        let ctx = ClarifyContext {
            channel_id: "telegram".into(),
            conversation_id: "user-1".into(),
            session_key: "telegram:bot:1:user-1".into(),
        };
        let mat = materialize(
            &linear_def(),
            "x",
            "team-1",
            &store,
            Some(&ctx),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        for id in &mat.task_ids {
            let task = store.get_task(id).await.unwrap().unwrap();
            assert_eq!(
                workflow_origin(&task.metadata),
                Some(("telegram".to_string(), "user-1".to_string())),
                "every task carries the origin stamp"
            );
        }
        // Non-interactive runs stay byte-identical (no origin key).
        let silent = materialize(
            &linear_def(),
            "x",
            "team-1",
            &store,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        for id in &silent.task_ids {
            let task = store.get_task(id).await.unwrap().unwrap();
            assert!(task.metadata.get(WORKFLOW_ORIGIN_KEY).is_none());
            assert!(workflow_origin(&task.metadata).is_none());
        }
    }

    #[tokio::test]
    async fn materialize_stamps_origin_session_on_every_task() {
        use crate::gateway::goal_budget::origin_session_from_metadata;
        let store = setup_store().await;
        let mat = materialize(
            &linear_def(),
            "x",
            "team-1",
            &store,
            None,
            None,
            None,
            None,
            Some("channel:telegram:user-1"),
        )
        .await
        .unwrap();
        for id in &mat.task_ids {
            let task = store.get_task(id).await.unwrap().unwrap();
            assert_eq!(
                origin_session_from_metadata(&task.metadata).as_deref(),
                Some("channel:telegram:user-1"),
                "every task carries the goal-budget anchor"
            );
        }
        // No session context → byte-identical rows (no key at all).
        let silent = materialize(
            &linear_def(),
            "x",
            "team-1",
            &store,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        for id in &silent.task_ids {
            let task = store.get_task(id).await.unwrap().unwrap();
            assert!(origin_session_from_metadata(&task.metadata).is_none());
        }
    }

    #[tokio::test]
    async fn materialize_without_strategy_is_byte_identical() {
        let store = setup_store().await;
        let mat = materialize(
            &linear_def(),
            "x",
            "team-1",
            &store,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        for id in &mat.task_ids {
            let task = store.get_task(id).await.unwrap().unwrap();
            assert!(task.metadata.get(WORKFLOW_STRATEGY_KEY).is_none());
        }
    }
}
