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

use futures::future::join_all;
use serde_json::json;
use tracing::warn;

use crate::agents::swarm::tasks::acceptance::LEAD_REVIEW_METADATA_KEY;
use crate::agents::swarm::tasks::{
    CoordTaskId, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, NewCoordTask, Priority,
};
use crate::error::Result;
use crate::json_canvas_io::sanitise_name;
use crate::strategy::{render_workflow_global_frame, Strategy};
use crate::teams::dispatcher::{MANAGED_BY_DISPATCHER, MANAGED_BY_KEY};
use crate::workflow::clarify::{ClarifyContext, ClarifyTaskMeta, CLARIFY_META_KEY, CLARIFY_OWNER};
use crate::workflow::def::{render_prompt, RunInputs, WorkflowDef};

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
/// Metadata key carrying a step's phase title (the manifest's per-step `phase`,
/// i.e. the `.workflow.js` `phase("…")` marker the step sits under). Read back
/// by `workflow(action='status')` to group the run's steps the way the
/// `.workflow.js` live view does. Interchange-only until this key existed: the
/// phase plan round-tripped through `import`/`export` and was invisible to
/// every runtime face. Absent on steps with no phase (byte-identical rows).
pub const WORKFLOW_PHASE_KEY: &str = "workflow_phase";
/// Metadata key carrying a step's requested output contract — the manifest's
/// opaque `schema` (a JSON Schema). `build_handoff_context` renders it as an
/// `## Output Contract` section so the member run is *told* what shape to
/// return.
///
/// **This is a request, not an enforcement.** Aleph does not validate the
/// member's reply against it: doing so would need a structured-output channel
/// on `RunRequest` and a terminating tool inside the harness (R10's 12-file
/// budget). The honest contract is "the model was asked"; the previous
/// contract was "the field is carried and nothing whatsoever happens", which
/// read the same from the outside. Absent on steps with no schema.
pub const WORKFLOW_SCHEMA_KEY: &str = "workflow_schema";
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

/// Which stamper wrote [`WORKFLOW_NOTIFIED_KEY`] — [`NOTIFIED_BY_SETTLE`] or
/// [`NOTIFIED_BY_CANCEL`].
///
/// The re-arm rule needs to know whether the marked run was fully settled at
/// stamp time, and the stamp's AGE was standing in for that. It cannot: the
/// grace exists because `cancel` stamps BEFORE its status writes land, so for a
/// few moments a marked run legitimately has unsettled tasks — but applying that
/// window to the settle sweep's OWN marker blinds it to the most likely reopen
/// of all, "it failed" → the user says retry, seconds later. The corrected run's
/// real outcome then never reaches anyone. Provenance answers the question the
/// clock was being asked to guess.
///
/// Absent on rows stamped before this key existed → provenance unknown → the age
/// grace still applies, i.e. exactly the previous behaviour.
pub const WORKFLOW_NOTIFIED_BY_KEY: &str = "workflow_notified_by";
/// Stamped by the dispatcher's settle sweep, which only ever writes the marker
/// after observing the run fully settled. Any later unsettled task is therefore
/// a genuine reopen, with no window to protect.
pub const NOTIFIED_BY_SETTLE: &str = "settle";
/// Stamped by the `workflow` tool's `cancel` (and by `materialize`'s partial
/// rollback) BEFORE / INSTEAD OF the status writes, so unsettled tasks may
/// legitimately linger for a moment. This is the marker the grace is for.
pub const NOTIFIED_BY_CANCEL: &str = "cancel";

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

/// Every per-step override a template can push past [`WorkflowDef`] into the
/// materialised task's metadata, as **one** value.
///
/// Before this type these were parallel `HashMap<String, String>` arguments to
/// [`materialize`], and the shape was simple enough that each new override got
/// copied rather than shared. That copy is what went wrong: `model` grew a
/// projection (`describe`/`run`/`status` all report it) and `effort` — stamped,
/// consumed by the dispatcher, and equally executable — grew none, so a
/// template could pin a step to `max` reasoning and no surface in the product
/// could say so. A struct makes the next override a field on a type every call
/// site already threads, and [`StepPins::census`] makes forgetting a face a
/// test failure rather than an invisible one.
///
/// `phase` and `schema` are the two members that were previously
/// interchange-only: they round-tripped through `import`/`export` and had zero
/// runtime consumers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StepPins {
    /// `"model"` or `"provider/model"` → `RunRequest.model_override`.
    pub model: Option<String>,
    /// `low`..`max` → the member run's `think_level`.
    pub effort: Option<String>,
    /// Phase title this step sits under → grouped `status` reporting.
    pub phase: Option<String>,
    /// Requested output contract (JSON Schema) → an `## Output Contract`
    /// section in the member's handoff. Requested, never validated — see
    /// [`WORKFLOW_SCHEMA_KEY`].
    pub schema: Option<serde_json::Value>,
}

impl StepPins {
    /// True when this step pins nothing — the caller then leaves the task row
    /// byte-identical to a legacy materialisation.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.effort.is_none()
            && self.phase.is_none()
            && self.schema.is_none()
    }

    /// Name every pin this value carries, in declaration order.
    ///
    /// Implemented by **exhaustive destructuring** on purpose: adding a field
    /// to [`StepPins`] without adding it here is a compile error, which forces
    /// its author to answer "does this need a surface?" at the moment they add
    /// it. The `workflow` tool's projection test asserts against this list, so
    /// a pin that reaches task metadata but no reporting face fails a test
    /// **by name** instead of shipping invisible (which is exactly how `effort`
    /// shipped).
    #[must_use]
    pub fn census(&self) -> Vec<&'static str> {
        let Self {
            model,
            effort,
            phase,
            schema,
        } = self;
        let mut out = Vec::new();
        if model.is_some() {
            out.push("model");
        }
        if effort.is_some() {
            out.push("effort");
        }
        if phase.is_some() {
            out.push("phase");
        }
        if schema.is_some() {
            out.push("schema");
        }
        out
    }

    /// The full field vocabulary, independent of what this value holds.
    /// Derived from `census` on a fully-populated value so the two can never
    /// disagree.
    #[must_use]
    pub fn all_fields() -> Vec<&'static str> {
        Self {
            model: Some(String::new()),
            effort: Some(String::new()),
            phase: Some(String::new()),
            schema: Some(serde_json::Value::Null),
        }
        .census()
    }

    /// Stamp this step's pins onto `meta` (a task-metadata object). A `None`
    /// pin writes nothing, so a step that pins nothing leaves the row
    /// byte-identical. Blank strings are treated as absent — a manifest field
    /// set to `""` is "unset spelled differently", and letting it through
    /// would put an empty `workflow_model` on the wire that every reader then
    /// has to re-filter.
    pub fn stamp(&self, meta: &mut serde_json::Value) {
        let Some(obj) = meta.as_object_mut() else {
            return;
        };
        for (key, value) in [
            (WORKFLOW_MODEL_KEY, self.model.as_deref()),
            (WORKFLOW_EFFORT_KEY, self.effort.as_deref()),
            (WORKFLOW_PHASE_KEY, self.phase.as_deref()),
        ] {
            if let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) {
                obj.insert(key.to_string(), serde_json::json!(v));
            }
        }
        if let Some(schema) = self.schema.as_ref().filter(|s| !s.is_null()) {
            obj.insert(WORKFLOW_SCHEMA_KEY.to_string(), schema.clone());
        }
    }
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

/// Materialise `def` into `coord_tasks` under `team_id`, substituting the run's
/// [`RunInputs`] into each step's prompt. Returns the created task ids in
/// topological order.
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
/// `pins` maps step-local id → [`StepPins`]: the per-step model / effort /
/// phase / output-contract overrides the lean [`WorkflowDef`] cannot carry.
/// Each present pin is stamped onto that **agent** step's task metadata
/// ([`StepPins::stamp`]) where the dispatcher and `workflow(action='status')`
/// read it back. A step with no entry (or `None` here) leaves the row
/// byte-identical to a legacy materialisation. Clarify steps run no agent, so
/// they are never stamped.
///
/// `origin_session` is the launching session's key captured at run start (the
/// tool's turn context); it is stamped on every task as the `origin_session`
/// goal-budget anchor so autonomously-dispatched member runs enroll into the
/// creating session's goal tree budget — the same anchor `task_create` and the
/// workflow canvas stamp. `None` (non-interactive run) leaves rows
/// byte-identical and the children run unaccounted, exactly as before.
/// `inputs` carries both placeholder forms the run substitutes: the anonymous
/// `{input}` and the named `{{var}}` args. It is one parameter rather than two
/// because this signature is already at eight, and because the two are one
/// fact — see [`RunInputs`].
pub async fn materialize(
    def: &WorkflowDef,
    inputs: &RunInputs,
    team_id: &str,
    store: &dyn CoordTaskStore,
    clarify_ctx: Option<&ClarifyContext>,
    pins: Option<&std::collections::HashMap<String, StepPins>>,
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
        let rendered = render_prompt(&step.prompt, inputs);

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
            // Per-step pins (from the AWI manifest): model / effort / phase /
            // output-contract, stamped through the ONE carrier so a new pin
            // reaches metadata by adding a field rather than by remembering to
            // add a fourth block here. Pins the step does not set write
            // nothing → byte-identical rows.
            if let Some(step_pins) = pins.and_then(|m| m.get(step.id.as_str())) {
                step_pins.stamp(&mut meta);
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
            // Tolerant fan-in: the readiness derivation (which never sees the
            // template) reads this stamp off the row, so it must be written
            // here or the flag is inert. `false` is a pass-through, so an
            // ordinary step's row is byte-identical.
            let meta = crate::agents::swarm::tasks::acceptance::with_tolerate_failed_deps(
                meta,
                step.tolerate_failed_deps,
            );
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
                // Sanitise both parts so a `my:workflow` name or `step:1` id
                // can't produce a subject with multiple colons (downstream
                // tooling treats the FIRST `:` as the name/step boundary). The
                // raw name/id live in the persisted manifest unchanged — this
                // is a display-subject invariant only.
                subject: format!("{}:{}", sanitise_name(&def.name), sanitise_name(&step.id)),
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
    // Stamp the once-only marker FIRST, on the same anchor rule the other two
    // stampers use (smallest id). Cancelled is a settled status, and these rows
    // already carry `WORKFLOW_RUN_ID_KEY` + `WORKFLOW_ORIGIN_KEY` — so without
    // this the settle sweep sees a fully-settled interactive run and pushes
    // "⚠️ Workflow 'x' finished … cancelled" to the user, seconds after the tool
    // already returned an error saying the run could not be started. Two
    // contradictory messages about a run that never executed a step.
    //
    // Marked `cancel` for the same reason the `cancel` action is: the status
    // writes below are not atomic with it.
    if let Some(anchor) = ids.iter().min() {
        if let Ok(Some(task)) = store.get_task(anchor).await {
            let merged = crate::agents::swarm::tasks::merge_metadata_patch(
                &task.metadata,
                serde_json::json!({
                    WORKFLOW_NOTIFIED_KEY: now_epoch_secs(),
                    WORKFLOW_NOTIFIED_BY_KEY: NOTIFIED_BY_CANCEL,
                }),
            );
            if let Err(e) = store
                .update_task(
                    anchor,
                    CoordTaskUpdate {
                        metadata: Some(merged),
                        ..Default::default()
                    },
                )
                .await
            {
                warn!(
                    anchor = %anchor,
                    error = %e,
                    "cancel_partial: anchor notified-stamp failed"
                );
            }
        }
    }
    // Concurrent status updates: the per-task write is independent (no shared
    // metadata between these rows), so the N sequential round-trips collapse
    // to one wall-clock round-trip via `join_all`. We are already on the
    // error path of `materialise` and these are best-effort cancellations, so
    // concurrent execution costs nothing and lets the dispatcher observe a
    // fully-cancelled partial run in a single window instead of a sweep that
    // may straddle a follow-up user prompt.
    join_all(ids.iter().map(|id| async move {
        if let Err(e) = store
            .update_task(
                id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Cancelled),
                    ..Default::default()
                },
            )
            .await
        {
            warn!(task = %id, error = %e, "cancel_partial: status cancel failed");
        }
    }))
    .await;
}

/// Epoch seconds, the unit [`WORKFLOW_NOTIFIED_KEY`] is stored in.
///
/// `pub(crate)` because the key has more than one stamper: `materialize`'s
/// partial rollback (below) and the `workflow` tool's `cancel` arm both write
/// it, and the settle sweep then subtracts one from `now` to grace-gate the
/// re-arm. A unit derived independently at each write site is one edit away
/// from a comparison between two different units, so the derivation lives with
/// the key it is the unit of.
pub(crate) fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
            tolerate_failed_deps: false,
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
            tolerate_failed_deps: false,
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
            &RunInputs::from_input("the topic"),
            "team-1",
            &store,
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
            &RunInputs::from_input("quantum computing"),
            "team-1",
            &store,
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
            &RunInputs::from_input("x"),
            "team-1",
            &store,
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
            &RunInputs::from_input("x"),
            "team-1",
            &store,
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
            &RunInputs::from_input("x"),
            "team-1",
            &store,
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
        let mat = materialize(
            &def,
            &RunInputs::from_input("x"),
            "t",
            &store,
            None,
            None,
            None,
            None,
        )
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
        assert!(materialize(
            &def,
            &RunInputs::from_input("x"),
            "team-1",
            &store,
            None,
            None,
            None,
            None
        )
        .await
        .is_err());
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
        let mat = materialize(
            &def,
            &RunInputs::from_input("x"),
            "t",
            &store,
            None,
            None,
            None,
            None,
        )
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
            &RunInputs::from_input("us-east"),
            "team-1",
            &store,
            Some(&ctx),
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
        let mat = materialize(
            &def,
            &RunInputs::from_input("x"),
            "team-1",
            &store,
            None,
            None,
            None,
            None,
        )
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


    /// The tolerant-fan-in flag only exists at run time as a metadata stamp:
    /// readiness is derived from the stored row + its edges, and that
    /// derivation never sees the template. An unstamped step must stay
    /// byte-identical (no key at all), or every legacy row acquires a field.
    #[tokio::test]
    async fn materialize_tolerant_step_stamps_the_readiness_flag() {
        use crate::agents::swarm::tasks::acceptance::{
            tolerate_failed_deps, TOLERATE_FAILED_DEPS_METADATA_KEY,
        };
        let store = setup_store().await;
        let mut def = linear_def();
        def.steps[1].tolerate_failed_deps = true;
        let mat = materialize(
            &def,
            &RunInputs::from_input("x"),
            "team-1",
            &store,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let first = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        assert!(
            first
                .metadata
                .get(TOLERATE_FAILED_DEPS_METADATA_KEY)
                .is_none(),
            "an ordinary step's row carries no key (byte-identical legacy row)"
        );
        assert!(!tolerate_failed_deps(&first.metadata));

        let second = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert!(
            tolerate_failed_deps(&second.metadata),
            "the tolerant step's row carries the stamp the store reads: {:?}",
            second.metadata
        );
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
        let mat = materialize(
            &def,
            &RunInputs::from_input("x"),
            "t",
            &store,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let ask = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        let meta = ClarifyTaskMeta::from_metadata(&ask.metadata).expect("clarify meta present");
        assert!(meta.channel_id.is_empty());
        assert!(meta.session_key.is_empty());
    }

    #[tokio::test]
    async fn materialize_stamps_per_step_model_override() {
        // A pins map keyed by step id stamps WORKFLOW_MODEL_KEY onto exactly
        // the matching agent step; an unlisted step stays byte-identical (no
        // key), so non-overridden runs keep the agent's default model.
        let store = setup_store().await;
        let mut pins = std::collections::HashMap::new();
        pins.insert(
            "gather".to_string(),
            StepPins {
                model: Some("opus".into()),
                ..Default::default()
            },
        );
        let mat = materialize(
            &linear_def(),
            &RunInputs::from_input("x"),
            "team-1",
            &store,
            None,
            Some(&pins),
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
        // A pins map keyed by step id stamps WORKFLOW_EFFORT_KEY onto exactly
        // the matching agent step; an unlisted step stays byte-identical (no
        // key) — the exact WORKFLOW_MODEL_KEY contract.
        let store = setup_store().await;
        let mut pins = std::collections::HashMap::new();
        pins.insert(
            "gather".to_string(),
            StepPins {
                effort: Some("max".into()),
                ..Default::default()
            },
        );
        let mat = materialize(
            &linear_def(),
            &RunInputs::from_input("x"),
            "team-1",
            &store,
            None,
            Some(&pins),
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
            &RunInputs::from_input("x"),
            "team-1",
            &store,
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
        let mat = materialize(
            &def,
            &RunInputs::from_input("x"),
            "team-1",
            &store,
            None,
            None,
            None,
            None,
        )
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
            &RunInputs::from_input("x"),
            "team-1",
            &store,
            Some(&ctx),
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
            &RunInputs::from_input("x"),
            "team-1",
            &store,
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
            &RunInputs::from_input("x"),
            "team-1",
            &store,
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
            &RunInputs::from_input("x"),
            "team-1",
            &store,
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
            &RunInputs::from_input("x"),
            "team-1",
            &store,
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

    #[tokio::test]
    async fn materialize_stamps_phase_and_schema_pins() {
        // `phase` and `schema` ride the same StepPins carrier as model/effort.
        // Phase feeds grouped `status` reporting; schema feeds the handoff's
        // `## Output Contract` section. An unpinned step stays byte-identical.
        let store = setup_store().await;
        let mut pins = std::collections::HashMap::new();
        pins.insert(
            "gather".to_string(),
            StepPins {
                phase: Some("Scan".into()),
                schema: Some(json!({"type": "object", "required": ["paths"]})),
                ..Default::default()
            },
        );
        let mat = materialize(
            &linear_def(),
            &RunInputs::from_input("x"),
            "team-1",
            &store,
            None,
            Some(&pins),
            None,
            None,
        )
        .await
        .unwrap();

        let gather = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        assert_eq!(
            gather
                .metadata
                .get(WORKFLOW_PHASE_KEY)
                .and_then(|v| v.as_str()),
            Some("Scan")
        );
        assert_eq!(
            gather
                .metadata
                .get(WORKFLOW_SCHEMA_KEY)
                .and_then(|v| v.get("required")),
            Some(&json!(["paths"]))
        );
        let write = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert!(write.metadata.get(WORKFLOW_PHASE_KEY).is_none());
        assert!(write.metadata.get(WORKFLOW_SCHEMA_KEY).is_none());
    }

    #[test]
    fn step_pins_census_is_exhaustive_and_stamp_covers_it() {
        // `census()` destructures exhaustively, so a new StepPins field is a
        // compile error until it is named there. This test closes the second
        // half: every named pin must actually LAND in metadata when stamped —
        // a field that is censused but not stamped would report itself as
        // carried while writing nothing (the `effort` failure shape, one layer
        // down).
        let pins = StepPins {
            model: Some("opus".into()),
            effort: Some("max".into()),
            phase: Some("Scan".into()),
            schema: Some(json!({"type": "object"})),
        };
        let mut meta = json!({});
        pins.stamp(&mut meta);
        let obj = meta.as_object().unwrap();
        assert_eq!(
            pins.census(),
            StepPins::all_fields(),
            "fully-set = full census"
        );
        for field in StepPins::all_fields() {
            let key = match field {
                "model" => WORKFLOW_MODEL_KEY,
                "effort" => WORKFLOW_EFFORT_KEY,
                "phase" => WORKFLOW_PHASE_KEY,
                "schema" => WORKFLOW_SCHEMA_KEY,
                other => panic!("StepPins grew `{other}` with no metadata key mapping"),
            };
            assert!(obj.contains_key(key), "pin `{field}` did not stamp `{key}`");
        }
        // Blank strings are "unset spelled differently" — nothing lands.
        let mut blank_meta = json!({});
        StepPins {
            model: Some("  ".into()),
            ..Default::default()
        }
        .stamp(&mut blank_meta);
        assert!(blank_meta.as_object().unwrap().is_empty());
    }
}
