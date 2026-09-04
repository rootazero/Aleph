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
    self, ClarifyContext, RunInputs, WorkflowDef, WorkflowManifest, WORKFLOW_MODEL_KEY,
    WORKFLOW_NAME_KEY, WORKFLOW_RUN_ID_KEY, WORKFLOW_STEP_KEY,
};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum WorkflowArgs {
    /// Save (create or overwrite) a reusable workflow template to disk.
    Save { definition: WorkflowDef },
    /// List every saved workflow template with its description, `whenToUse`
    /// guidance and step count — enough to pick one without a `describe` per
    /// candidate. Files that will not parse are named in `problems` and still
    /// listed, so a corrupt template is never mistaken for a missing one.
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
        /// Named values substituted for `{{name}}` placeholders in the step
        /// prompts. The names are read off the prompts themselves (`describe`
        /// and `list` report them as `vars`), so this map must cover every one
        /// of them — a run missing an arg is refused rather than launched with
        /// the placeholder left in the prompt.
        #[serde(default)]
        args: std::collections::HashMap<String, String>,
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
        /// Also return each completed step's (bounded) output. Off by default:
        /// a poll should stay cheap. Turn it on when you are ready to read what
        /// the run produced — this is the only face that hands back a workflow
        /// run's actual results, so it is how you collect a fan-out before
        /// synthesizing.
        #[serde(default)]
        include_output: bool,
    },
    /// Cancel the remaining steps of a workflow run: every not-yet-finished
    /// task (pending / blocked / paused / `waiting_review` / `in_progress`) is
    /// marked Cancelled, and an in-progress step's member run is stopped
    /// within a tick — it does not keep burning tokens to the timeout.
    /// Finished steps keep their results.
    Cancel {
        /// Name of the workflow template the run was started from.
        name: String,
        /// Team hosting the run.
        team_id: String,
        /// Specific run to cancel; omitted → the latest run.
        #[serde(default)]
        run_id: Option<String>,
    },
    /// Suspend the unfinished steps of a workflow run: every pending /
    /// blocked / `waiting_review` task is parked Paused so the dispatcher
    /// stops advancing the DAG (a review-parked step remembers its origin
    /// and resumes back into `waiting_review`; verdicts still land while
    /// paused). A step already executing finishes on its own (its result is
    /// kept), but the pause is recorded against it, so a daemon restart while
    /// it runs parks that step Paused instead of restarting it. Steps already
    /// settled are untouched. Undo with `action='resume'`.
    Pause {
        /// Name of the workflow template the run was started from.
        name: String,
        /// Team hosting the run.
        team_id: String,
        /// Specific run to pause; omitted → the latest run.
        #[serde(default)]
        run_id: Option<String>,
    },
    /// Resume a paused workflow run: paused steps return to their pause
    /// origin (`waiting_review` for review-parked steps, pending otherwise)
    /// and the dispatcher picks the DAG back up. A clarify step parked
    /// awaiting the user's answer stays parked — it resumes when they reply.
    Resume {
        /// Name of the workflow template the run was started from.
        name: String,
        /// Team hosting the run.
        team_id: String,
        /// Specific run to resume; omitted → the latest run.
        #[serde(default)]
        run_id: Option<String>,
    },
    /// List every run of a workflow on a team — one row per run id, newest
    /// first, with its step count, per-status summary and whether it has
    /// settled. `status` inspects ONE run (the latest by default); this is how
    /// you find the older ones, and how you tell "that run finished" from
    /// "that run is still going" without polling each in turn.
    Runs {
        /// Name of the workflow template.
        name: String,
        /// Team hosting the runs.
        team_id: String,
    },
    /// Re-queue the failed steps of a run: every step that failed, plus every
    /// step left `unsatisfiable` by one, goes back to pending with a fresh
    /// retry budget, and the dispatcher picks the DAG back up. Completed steps
    /// keep their results and are not re-run. Use after fixing whatever made
    /// the step fail (a missing team member, a wrong model pin, a service that
    /// was down).
    RerunFailed {
        /// Name of the workflow template the run was started from.
        name: String,
        /// Team hosting the run.
        team_id: String,
        /// Specific run to re-arm; omitted → the latest run.
        #[serde(default)]
        run_id: Option<String>,
    },
    /// Render a saved template into a Claude-Code-compatible dynamic-workflow
    /// `.mjs` (the extension Claude Code's workflow loader recognises).
    Export {
        /// Name of the saved template to render.
        name: String,
        /// Also write it to `$ALEPH_HOME/workflows/<name>.mjs`.
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
    /// Reject (dismiss) a gated `MetaSkill` proposal: remove the draft from
    /// the `proposals/` dir without activating it. Idempotent. The miner may
    /// re-draft the same chain on a later dream cycle if it keeps recurring.
    RejectProposal {
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
    /// Phase title this step sits under (`workflow_phase` metadata), so a
    /// status report can be read the way the `.workflow.js` live view groups
    /// work. Absent for templates that declare no phases.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Per-step model override the dispatcher resolves at launch (read from the
    /// `workflow_model` metadata the compiler stamped). Present only for steps
    /// that pin a model — so the inspecting LLM sees which model a step is (or
    /// was) running on without exporting the template to a `.mjs` file (R8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Per-step reasoning-effort override, the exact twin of
    /// [`model`](Self::model): stamped by the compiler under `workflow_effort`,
    /// turned into the member run's `think_level` by the dispatcher. It was
    /// executable and reported by nothing for as long as it existed, because it
    /// was a second parallel map beside `models` rather than a field on the
    /// same carrier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// For failed steps: the (bounded) error text, so the LLM can decide
    /// retry / skip / cancel without an extra lookup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The step's recorded output, bounded, present only when the caller passed
    /// `include_output: true` and the step actually produced one.
    ///
    /// Without this the `workflow` tool could start a fan-out, watch it finish,
    /// and never read what it produced: `error` was populated for `Failed`
    /// steps only, and the sole alternative route (`team_status`) dumps every
    /// task of the whole team with unbounded results. Off by default because a
    /// status poll is a poll — you pay for the outputs when you synthesize, not
    /// on every tick.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// A step's per-step pins, projected for `describe` / `run` / `status` so the
/// inspecting LLM sees what each step is pinned to *before* (and just after)
/// launching — the executable half of the manifest's per-step metadata that
/// `to_def` otherwise drops (R8 model-perceivable surface).
///
/// One row type for every pin: the previous shape was `WorkflowStepModel`,
/// carrying `model` alone, and `effort` — stamped, executed, equally
/// user-authored — had no row of its own and therefore no surface anywhere in
/// the product.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowStepPin {
    /// Step-local id from the template.
    pub step: String,
    /// `"model"` or `"provider/model"` — resolved by the dispatcher into a
    /// `RunRequest.model_override` at member-run launch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `low`..`max` — resolved into the member run's `think_level`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Phase title, for grouped reporting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// `true` when the step pins an output contract (the schema itself is not
    /// echoed — it can be large, and `export` is the place to read it).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub schema: bool,
}

/// One entry of a `list` result — enough to *choose* a workflow without a
/// round-trip per candidate.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowListEntry {
    /// Storage key: pass this verbatim to `describe` / `run` / `delete`.
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The template's `whenToUse` — the field the `.workflow.js` format exists
    /// to put in front of this decision. It had no runtime reader at all before
    /// this row: neither `list` nor `describe` surfaced it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub when_to_use: String,
    pub steps: usize,
    /// The `{{name}}` placeholders this template's prompts reference — every
    /// key `run` will demand in `args`. Derived from the prompts, so it cannot
    /// disagree with them. Empty (and omitted from the wire) for a template
    /// that uses no named args.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vars: Vec<String>,
}

/// One run of a workflow in a `runs` listing — the identity plus enough state
/// to decide whether it is worth a `status` call.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowRunSummary {
    /// Pass verbatim to `status` / `cancel` / `rerun_failed` as `run_id`.
    pub run_id: String,
    /// Epoch seconds of the run's earliest task — when it was materialised.
    pub started_at: u64,
    /// How many steps the run materialised.
    pub steps: usize,
    /// Whether every step has settled, read through
    /// `CoordTaskStatus::is_settled` — the same predicate the dispatcher's
    /// settle sweep uses, not a hand-listed set of statuses that would go
    /// stale the next time one is added.
    pub settled: bool,
    /// Per-status tally, the same rendering `status` puts in its message.
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct WorkflowToolOutput {
    pub action: String,
    pub message: String,
    /// Populated by `list` — one row per saved template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflows: Option<Vec<WorkflowListEntry>>,
    /// Populated by `describe`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<WorkflowDef>,
    /// Populated by `describe` — the template's `whenToUse` selection guidance,
    /// which the lean `WorkflowDef` cannot carry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    /// Populated by `describe` — the declared phase plan (title + optional
    /// detail), in declaration order. Also `WorkflowDef`-inexpressible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phases: Option<Vec<String>>,
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
    /// Populated by `export` — the rendered `.mjs` (dynamic-workflow) text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered: Option<String>,
    /// Populated by `import` — imperative constructs that could not be mapped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped: Option<Vec<String>>,
    /// Populated by `describe` (the template's pins) and `run` (the pins
    /// actually applied to the launched steps) — the per-step overrides
    /// `definition` (a lean `WorkflowDef`) cannot carry.
    /// Empty when no step pins anything; omitted from the wire then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pins: Option<Vec<WorkflowStepPin>>,
    /// Populated by `list` — files in the workflow directory that could not be
    /// read or parsed, named. A corrupt template is otherwise
    /// indistinguishable from one that was never saved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problems: Option<Vec<String>>,
    /// Populated by `describe` — the `{{name}}` placeholders the template's
    /// prompts reference, i.e. exactly the keys `run` will require in `args`.
    /// Omitted for a template that uses none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vars: Option<Vec<String>>,
    /// Populated by `runs` — one row per run of the template on the team,
    /// newest first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs: Option<Vec<WorkflowRunSummary>>,
}

impl WorkflowToolOutput {
    /// The two fields every action populates; every other field defaults to
    /// absent. Hand-writing `None` for the rest made adding a field an edit to
    /// this function as well as to the struct — and the struct is where the
    /// compiler would otherwise have caught the omission.
    fn msg(action: &str, message: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            message: message.into(),
            ..Default::default()
        }
    }
}

/// The `{{name}}` placeholders a stored template's prompts reference — the
/// exact key set `run` will demand in `args`.
///
/// Derived from the prompts by [`WorkflowDef::referenced_vars`], never from a
/// declared list: a declared list is a second spelling of a fact the prompts
/// already state, and it would go stale the first time a prompt is edited
/// without it.
fn referenced_vars(manifest: &WorkflowManifest) -> Vec<String> {
    manifest.to_def().referenced_vars().into_iter().collect()
}

/// Same, for a template addressed by its storage name. A template that will
/// not load reports NO vars rather than an error: the `list` row it decorates
/// is already named in `problems`, and the caller's question there is "which
/// workflow do I want", not "why is this one broken".
fn stored_vars(name: &str) -> Vec<String> {
    workflow::store::load(name)
        .map(|m| referenced_vars(&m))
        .unwrap_or_default()
}

/// Report a step's `effort` pin the way the runtime will actually treat it.
///
/// `save` and `import` validate the effort vocabulary, but `run` loads the
/// stored manifest and validates only the lean [`WorkflowDef`] — which knows
/// nothing about `effort` — so a hand-edited (or pre-vocabulary) template can
/// carry `effort: "turbo"` all the way to `StepPins::stamp`. The dispatcher
/// then resolves it through `workflow_effort_think_level`, gets `None`, and
/// applies nothing. Echoing the raw word back on three reporting faces told
/// the model the step was pinned to `turbo` while nothing was pinned; a wrong
/// label reads as fact where a missing one reads as "no value".
///
/// Recognised values are returned verbatim (the vocabulary is a synonym table,
/// so `high` must not be rewritten into an internal id the template never
/// used); anything the dispatcher will discard says so in the same breath.
fn effort_disclosure(raw: &str) -> String {
    if crate::agents::thinking::normalize_think_level(raw).is_some() {
        raw.to_string()
    } else {
        format!("{raw} (unrecognised — not applied)")
    }
}

/// Project a manifest's per-step pins into `pins` rows in step order. Returns
/// `None` when no step pins anything so the field is omitted from the wire
/// (byte-identical output for plain templates).
///
/// Derived from [`WorkflowManifest::step_pins`] — the same call `run` feeds to
/// `materialize` — so the reported pins and the stamped pins cannot disagree.
/// The previous projection read `s.model` directly off the manifest, which is
/// how it stayed a model-only projection while a second executable pin was
/// added beside it.
fn manifest_step_pins(manifest: &WorkflowManifest) -> Option<Vec<WorkflowStepPin>> {
    let pins = manifest.step_pins();
    let rows: Vec<WorkflowStepPin> = manifest
        .steps
        .iter()
        .filter_map(|s| {
            pins.get(&s.id).map(|p| WorkflowStepPin {
                step: s.id.clone(),
                model: p.model.clone(),
                effort: p.effort.as_deref().map(effort_disclosure),
                phase: p.phase.clone(),
                schema: p.schema.is_some(),
            })
        })
        .collect();
    (!rows.is_empty()).then_some(rows)
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
    /// The registry the dispatcher will actually deliver a `clarify` question
    /// through. Held ONLY so the run pre-flight can ask it the same question
    /// `handle_clarify_task` will ask later — see `clarify_is_deliverable`.
    /// `None` → the pre-flight cannot answer and does not refuse.
    channels: Option<Arc<crate::gateway::channel_registry::ChannelRegistry>>,
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
            channels: None,
        }
    }

    #[must_use]
    pub fn with_planner_provider(mut self, provider: Option<Arc<dyn AiProvider>>) -> Self {
        self.planner_provider = provider;
        self
    }

    /// Wire the channel registry the clarify pre-flight consults. Mirrors
    /// [`Self::with_team_store`]: an optional capability whose absence only
    /// costs the pre-flight, never correctness (the dispatcher still fail-fasts
    /// an undeliverable clarify, just asynchronously and after `run` already
    /// reported success).
    #[must_use]
    pub fn with_channels(
        mut self,
        channels: Option<Arc<crate::gateway::channel_registry::ChannelRegistry>>,
    ) -> Self {
        self.channels = channels;
        self
    }

    /// Will a `clarify` step actually reach the user?
    ///
    /// The pre-flight used to answer this with
    /// [`TurnContext::is_channel_routable`], which is
    /// `!channel_id.is_empty() && !conversation_id.is_empty()` — a question
    /// about string emptiness, and structurally true for the surface most runs
    /// are launched from. Every Panel turn carries `channel_id = "gui:chat"`, a
    /// pseudo-channel that is deliberately NEVER registered, so the pre-flight
    /// passed, `materialize` created the tasks, `run` reported success, and the
    /// dispatcher then failed the clarify step at `channels.send(...)` and
    /// cascaded its dependents `Unsatisfiable`. The guard written to prevent
    /// exactly that outcome could not see it.
    ///
    /// So ask the delivery face's own question: is this channel in the registry
    /// the dispatcher will send through. A missing registry answers "yes" —
    /// refusing on a capability we simply do not hold would ground workflows
    /// that work today.
    async fn clarify_is_deliverable(&self, ctx: Option<&ClarifyContext>) -> bool {
        let Some(ctx) = ctx else {
            return false;
        };
        let Some(channels) = self.channels.as_ref() else {
            return true;
        };
        channels
            .get(&crate::gateway::channel::ChannelId::new(&ctx.channel_id))
            .await
            .is_some()
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

    /// Re-read `task` immediately before writing to it, degrading to the
    /// snapshot the `run_tasks` listing produced when the store cannot answer.
    ///
    /// Every terminal-write arm (`cancel` / `pause` / `resume`) needs this and
    /// needs it for the same reason: the listing is a snapshot, and a step can
    /// complete, be cancelled, start, or have a verdict land between the
    /// listing and this iteration's write. Writing against the stale row
    /// clobbers finished work — `Completed → Cancelled`, or a reviewed step
    /// flattened back to `Pending` and re-executed.
    ///
    /// The degradation rule, documented once instead of three times: a fetch
    /// FAILURE falls back to the snapshot (P7 — an unreachable store must not
    /// turn every step into an unknown and abort the whole action); it does not
    /// fail closed, because the snapshot is a real observation of this run made
    /// moments ago and the per-status guards downstream still apply to it.
    async fn live_or_snapshot(&self, task: &CoordTask) -> CoordTask {
        self.coord_store
            .get_task(&task.id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| task.clone())
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
        let mut groups = self.run_groups(name, team_id).await?;
        // "This workflow has never run here" is an answer only the faces that
        // NEED a run can treat as a failure — `run_groups` itself must not
        // decide that for the listing face (which answers "none"). Raised here,
        // where the caller is asking about one specific run.
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
                .then_with(|| {
                    let rank_a = ranks
                        .get(&a.id)
                        .expect("invariant: rank computed for every task");
                    let rank_b = ranks
                        .get(&b.id)
                        .expect("invariant: rank computed for every task");
                    rank_a.cmp(rank_b)
                })
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok((selected, tasks))
    }

    /// Every materialised run of `name` on `team_id`, grouped by the
    /// `workflow_run_id` the compiler stamped. An empty map means "no runs" —
    /// a fact, not an error: `runs` reports it as an empty listing, while
    /// `run_tasks` (which cannot proceed without one) turns it into the error.
    ///
    /// Extracted from [`Self::run_tasks`], which computed exactly this map and
    /// then discarded every group but one — so "which runs of this workflow
    /// exist" was data the tool already held and no face could ask for.
    async fn run_groups(
        &self,
        name: &str,
        team_id: &str,
    ) -> Result<std::collections::HashMap<String, Vec<CoordTask>>> {
        let tasks = self
            .coord_store
            .list_tasks(CoordTaskFilter {
                team_id: Some(team_id.to_string()),
                ..Default::default()
            })
            .await?;

        // Compare CANONICAL names on both sides: the store saves under
        // `sanitise_name(name)` (so `list` shows e.g. `research_report`), but
        // materialisation stamps the manifest's raw inner name (`research
        // report`). A raw string compare against the only discoverable
        // (sanitised) name would report "no runs found" for every template
        // whose name contains a char outside [A-Za-z0-9._-] — right after a
        // successful `run`. Canonicalising both sides matches every historic
        // row regardless of which form was stamped.
        let wanted = crate::json_canvas_io::sanitise_name(name);
        let mut groups: std::collections::HashMap<String, Vec<CoordTask>> =
            std::collections::HashMap::new();
        for task in tasks {
            if task
                .metadata
                .get(WORKFLOW_NAME_KEY)
                .and_then(|v| v.as_str())
                .map(crate::json_canvas_io::sanitise_name)
                .as_deref()
                != Some(wanted.as_str())
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
        Ok(groups)
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

/// Bound applied to a step's echoed output in a `status` report. Enough for a
/// synthesis step to read a real finding, small enough that a twenty-step run
/// stays a readable tool result rather than a transcript.
const MAX_STEP_OUTPUT_CHARS: usize = 1200;

/// Project one materialised task into a `status` row.
///
/// `include_output` gates only the (bounded) `output` field — everything else is
/// unconditional, so a plain poll is byte-identical to the legacy row plus the
/// two pins that were previously stamped and reported by nothing.
fn step_row(task: &CoordTask, include_output: bool) -> WorkflowRunStep {
    // A pin the compiler stamped, read back as a trimmed non-empty string.
    // One helper for all three so a fourth reported pin is one line, not a
    // fourth hand-rolled `get().and_then().map().filter()` chain — that shape
    // is exactly why `effort` was never given one.
    let pin = |key: &str| {
        task.metadata
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string)
    };
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
        phase: pin(crate::workflow::WORKFLOW_PHASE_KEY),
        // The per-step model the dispatcher resolves at launch — read from the
        // same `workflow_model` metadata `workflow_model_override` consumes, so
        // status reports exactly the model the run uses. Absent → agent default.
        model: pin(WORKFLOW_MODEL_KEY),
        // Its twin: `workflow_effort` → the member run's think_level. Passed
        // through the same honesty filter `describe`/`run` use, so a value the
        // dispatcher discards is never reported as an applied pin.
        effort: pin(crate::workflow::WORKFLOW_EFFORT_KEY)
            .as_deref()
            .map(effort_disclosure),
        error: match task.status {
            CoordTaskStatus::Failed => task
                .result
                .as_deref()
                .filter(|r| !r.trim().is_empty())
                .map(|r| bound_chars(r, 400)),
            _ => None,
        },
        // Gate on the STATUS, the same way `error:` above does. `result` is the
        // task's one free-text slot and several non-terminal transitions write
        // it — a retry notice, a cancellation reason, an error — so reading it
        // unconditionally hands the model a dispatcher diagnostic under a field
        // documented as "the step's recorded output". Only the two statuses
        // that mean "this step produced its deliverable" may speak here.
        output: match task.status {
            CoordTaskStatus::Completed | CoordTaskStatus::WaitingReview if include_output => task
                .result
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(|r| bound_chars(r, MAX_STEP_OUTPUT_CHARS)),
            _ => None,
        },
    }
}

/// Per-phase tallies for one run. `done + failed + skipped` need not equal
/// `settled`: a status can be settled without being any of the three (there
/// is no such status today, and writing the marker off `settled` rather than
/// off a sum keeps that true tomorrow).
#[derive(Default, Clone, Copy)]
struct PhaseTally {
    done: usize,
    failed: usize,
    skipped: usize,
    settled: usize,
    total: usize,
}

impl PhaseTally {
    /// One character for the whole phase: has it stopped, and if so, how.
    ///
    /// The three predicates are deliberately distinct: "produced a result"
    /// (`Completed`), "stopped badly" (`Failed`/`Cancelled`/`Unsatisfiable`),
    /// and "stopped at all" (`CoordTaskStatus::is_settled`, whose own doc
    /// names it the honest completion predicate for a workflow run). An
    /// earlier draft of this had only the first two and inferred the third
    /// from `done == total`, which renders a phase whose step was
    /// deliberately `Skipped` as `0/1 ▶` — running, forever, when it had in
    /// fact finished. That is the same "stopped is not succeeded" mistake as
    /// counting a cancelled step as done, pointed the other way.
    /// `✗` is reserved for a phase that has actually STOPPED: a failure inside
    /// a phase whose other steps are still executing used to short-circuit the
    /// settled check, so a four-step phase with one failure and three runs in
    /// flight rendered `Analyze 0/4 ✗` — "stopped badly" about work that had
    /// not stopped. The two axes are now read in order: has it stopped, and
    /// then did anything fail. The failure stays visible while running via
    /// [`Self::failed_note`], not by borrowing the terminal marker.
    const fn marker(self) -> &'static str {
        if self.settled != self.total {
            "▶"
        } else if self.failed > 0 {
            "✗"
        } else {
            "✓"
        }
    }

    /// The failure count, spelled out while the phase is still running — the
    /// marker cannot carry it there without lying about whether the phase has
    /// stopped. Empty once settled: `✗` already says it.
    fn failed_note(self) -> String {
        if self.failed > 0 && self.settled != self.total {
            format!(" ({} failed)", self.failed)
        } else {
            String::new()
        }
    }
}

/// Render a run's progress grouped by phase, the way the `.workflow.js` live
/// view does: `Scan 1/1 ✓ · Analyze 1/2 ▶`.
///
/// Returns `None` when no step carries a phase — the overwhelming majority of
/// hand-written templates — so a phase-less run's message is byte-identical to
/// what it always was. Phases appear in first-seen order, which is creation
/// (topological) order, i.e. the order the run actually walks them.
///
/// Pure aggregation. It says how many steps of each phase have settled, not
/// whether the phase "went well" — judging that is the LLM's job (R7). A
/// non-zero skip count is spelled out rather than folded into `done`, because
/// `1/2 ✓` is only honest if the reader can see where the other step went.
fn summarize_phases(tasks: &[CoordTask]) -> Option<String> {
    let mut order: Vec<String> = Vec::new();
    let mut counts: std::collections::HashMap<String, PhaseTally> =
        std::collections::HashMap::new();
    for task in tasks {
        let Some(phase) = task
            .metadata
            .get(crate::workflow::WORKFLOW_PHASE_KEY)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|p| !p.is_empty())
        else {
            continue;
        };
        let entry = counts.entry(phase.to_string()).or_insert_with(|| {
            order.push(phase.to_string());
            PhaseTally::default()
        });
        entry.total += 1;
        if task.status.is_settled() {
            entry.settled += 1;
        }
        match task.status {
            CoordTaskStatus::Completed => entry.done += 1,
            // "Stopped" and "succeeded" are two predicates: a cancelled or
            // unsatisfiable step has settled without producing anything, and
            // folding it into `done` would let a phase report ✓ for work that
            // never happened.
            CoordTaskStatus::Failed
            | CoordTaskStatus::Cancelled
            | CoordTaskStatus::Unsatisfiable => entry.failed += 1,
            // Deliberately not run. Settled, not failed, produced nothing.
            CoordTaskStatus::Skipped => entry.skipped += 1,
            _ => {}
        }
    }
    if order.is_empty() {
        return None;
    }
    let parts: Vec<String> = order
        .iter()
        .map(|phase| {
            let t = counts[phase];
            let skipped = if t.skipped > 0 {
                format!(" ({} skipped)", t.skipped)
            } else {
                String::new()
            };
            format!(
                "{phase} {}/{} {}{skipped}{}",
                t.done,
                t.total,
                t.marker(),
                t.failed_note()
            )
        })
        .collect();
    Some(parts.join(" · "))
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
         dependencies); running it compiles the steps into a task DAG that \
         runs concurrently where dependencies allow. \
         Actions: save / list / describe / delete / run / status / runs / \
         rerun_failed / cancel / pause / resume / export / import / proposals / \
         describe_proposal / accept_proposal / reject_proposal. \
         `run` returns a run_id plus the backing task_ids — to block until the \
         run settles, pass those task_ids (or the team_id) to the `task_wait` \
         tool. `status` reports the per-step task states of \
         a run (latest by default), grouped by phase when the template declares \
         phases. \
         `runs` lists a template's runs on a team (`status` defaults to the \
         latest), `rerun_failed` re-queues a run's failed (and consequently \
         unsatisfiable) steps, and a step prompt's `{{name}}` placeholders are \
         supplied per run in `args` (names reported as `vars`). \
         `cancel` aborts a run's unfinished steps — \
         finished steps keep their results, and a step caught mid-execution \
         stays cancelled once its member run ends. `pause` parks a run's \
         not-yet-started steps and `resume` releases them (a clarify step \
         awaiting a reply stays parked). \
         `export` renders a template to a Claude Code dynamic-workflow \
         .mjs (writes `<name>.mjs` when write_file=true); `import` parses the \
         raw text of one (`.mjs` source or manifest JSON) back into a template. \
         `proposals` lists MetaSkill \
         drafts the dream pipeline grew from recurring skill use; \
         `describe_proposal` reviews one's steps + provenance before \
         `accept_proposal` activates it or `reject_proposal` dismisses the \
         draft. For `run`, create a team first so \
         each step's agent resolves to a member. `describe`, `run` and `status` \
         report each step's pins (model, effort, phase, output contract). A \
         step's output contract is a shape the step's agent is asked to \
         return; it is not validated for you.";

    type Args = WorkflowArgs;
    type Output = WorkflowToolOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args {
            WorkflowArgs::Save { definition } => {
                debug!(name = %definition.name, "workflow: save");
                // `save` authors the lean executable core — but it must not
                // DELETE what the core cannot express. Overwriting a stored
                // manifest with `from_def` stripped each step's `model` /
                // `effort` pin (both executable: they become the
                // WORKFLOW_MODEL_KEY / WORKFLOW_EFFORT_KEY stamps at
                // materialisation), and import's own "edit + save to retarget
                // the agents" advice walked users straight into it.
                //
                // Three-way, not two-way: `store::load` errors on BOTH "no such
                // file" and "the file is there and did not parse", and `.ok()`
                // collapsed those into "nothing to preserve" — the fail-closed
                // answer consumed as a value, on the one path that DELETES
                // (criterion 8). A step carries `deny_unknown_fields`, so one
                // typo'd key in a hand-edited file makes a template that still
                // holds every model/effort/schema/phase pin unreadable; the
                // save then wrote a lean copy over it and said `saved …` with
                // no `(preserved: …)` suffix, which is byte-identical to a
                // first-ever save. So probe the path first — the same
                // discipline `loop_graph_manage`'s workflow arm applies to this
                // very store — and refuse rather than overwrite what we cannot
                // read.
                let stored_path = workflow::store::resolve_path_at(
                    &workflow::store::workflow_dir(),
                    &definition.name,
                );
                let existing = if stored_path.exists() {
                    match workflow::store::load(&definition.name) {
                        Ok(prev) => Some(prev),
                        Err(e) => {
                            return Err(AlephError::invalid_input(format!(
                                "refusing to overwrite workflow '{}': {} exists but could not be \
                                 read ({e}). Saving now would delete whatever model / effort / \
                                 schema / phase / whenToUse it still holds. Repair the file, or \
                                 discard it with action='delete' first, or re-author it with \
                                 action='import' (save=true).",
                                definition.name,
                                stored_path.display()
                            )));
                        }
                    }
                } else {
                    None
                };
                let manifest = match &existing {
                    Some(prev) => prev.with_core_from(&definition),
                    None => WorkflowManifest::from_def(&definition),
                };
                let path = workflow::store::save(&manifest)?;
                // Name what was preserved, derived from the manifest rather
                // than from a remembered pair: the old message said
                // "model/effort pins preserved" and stayed silent about the
                // five other extras `with_core_from` also carries across.
                let kept = existing
                    .as_ref()
                    .map(WorkflowManifest::def_inexpressible_extras)
                    .unwrap_or_default();
                Ok(WorkflowToolOutput::msg(
                    "save",
                    format!(
                        "saved workflow '{}' → {}{}",
                        definition.name,
                        path.display(),
                        if kept.is_empty() {
                            String::new()
                        } else {
                            format!(" (preserved: {})", kept.join(", "))
                        }
                    ),
                ))
            }
            WorkflowArgs::List {} => {
                let listing = workflow::store::list()?;
                let workflows: Vec<WorkflowListEntry> = listing
                    .entries
                    .into_iter()
                    .map(|m| WorkflowListEntry {
                        // The listing carries no prompts, so the vars are read
                        // back off the stored manifest. A second read of a file
                        // the listing already opened, deliberately: the
                        // alternative is a `vars` column on the listing row,
                        // which would be a second derivation of a fact the
                        // prompts own. A row that will not load is already
                        // named in `problems`; it reports no vars rather than
                        // guessing at them.
                        vars: stored_vars(&m.name),
                        name: m.name,
                        description: m.description,
                        when_to_use: m.when_to_use,
                        steps: m.steps,
                    })
                    .collect();
                // Problems are named in the message too, not only in the field:
                // a caller that reads the summary line and stops must not be
                // told "3 workflow(s)" when one of them will not load.
                let message = if listing.problems.is_empty() {
                    format!("{} workflow(s)", workflows.len())
                } else {
                    format!(
                        "{} workflow(s); {} unreadable — see `problems`",
                        workflows.len(),
                        listing.problems.len()
                    )
                };
                Ok(WorkflowToolOutput {
                    workflows: Some(workflows),
                    problems: (!listing.problems.is_empty()).then_some(listing.problems),
                    ..WorkflowToolOutput::msg("list", message)
                })
            }
            WorkflowArgs::Describe { name } => {
                let manifest = workflow::store::load(&name)?;
                // Output the executable projection — the tool's `definition`
                // field is a `WorkflowDef` — plus everything `to_def` drops
                // that a caller needs in order to decide anything: the per-step
                // pins (model / effort / phase / output contract), the
                // `whenToUse` guidance, and the declared phase plan. Before
                // these fields, reading any of it meant rendering the template
                // to a `.mjs` file, and `whenToUse` could not be read at all.
                let pins = manifest_step_pins(&manifest);
                let when_to_use =
                    Some(manifest.when_to_use.clone()).filter(|s| !s.trim().is_empty());
                let phases: Vec<String> = manifest
                    .phases
                    .iter()
                    .map(|p| {
                        if p.detail.trim().is_empty() {
                            p.title.clone()
                        } else {
                            format!("{} — {}", p.title, p.detail)
                        }
                    })
                    .collect();
                let vars = referenced_vars(&manifest);
                let message = format!(
                    "workflow '{name}' has {} step(s){}",
                    manifest.steps.len(),
                    if vars.is_empty() {
                        String::new()
                    } else {
                        format!(" and requires args: {}", vars.join(", "))
                    }
                );
                Ok(WorkflowToolOutput {
                    definition: Some(manifest.to_def()),
                    when_to_use,
                    phases: (!phases.is_empty()).then_some(phases),
                    pins,
                    vars: (!vars.is_empty()).then_some(vars),
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
                args,
            } => {
                debug!(name = %name, team_id = %team_id, "workflow: run");
                // Load the full manifest: the executable core (`to_def`) drives
                // materialisation, while the per-step pins the lean
                // `WorkflowDef` cannot carry ride in beside it as ONE map — see
                // `StepPins`. `label` / `agentType` / `phase.model` remain
                // interchange-only (R10).
                let manifest = workflow::store::load(&name)?;
                let def = manifest.to_def();
                // Fail closed on a missing named arg (P7). An unsupplied
                // `{{region}}` renders as the literal `{{region}}`, which the
                // step's agent then reads as part of its instruction — a run
                // that looks launched, produces output, and answers a question
                // nobody asked. The names come off the prompts, so this cannot
                // demand a var no step uses.
                let missing = def.missing_vars(&args);
                if !missing.is_empty() {
                    return Err(AlephError::invalid_input(format!(
                        "workflow '{name}' needs run args [{}] that were not supplied — its step \
                         prompts reference {{{{{}}}}}. Pass them in `args` (see `vars` in \
                         action='describe').",
                        missing.join(", "),
                        missing.join("}}, {{"),
                    )));
                }
                let inputs = RunInputs { input, args };
                // step-local id → StepPins; empty when nothing is pinned.
                let pins = manifest.step_pins();
                // Deterministic, step-ordered projection of the SAME map to echo
                // back in the output — so the LLM sees what each step launched
                // with, without a follow-up `describe`/`status`, and so the
                // reported pins cannot drift from the stamped ones.
                let pin_rows = manifest_step_pins(&manifest);
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
                // Preflight: a clarify-bearing template launched with no
                // routable channel is born dead — the dispatcher would fail
                // each clarify step ("no originating channel") and cascade
                // its dependents Unsatisfiable, all AFTER this tool already
                // reported success. Reject before any coord_task exists (P7
                // boundary validation, same contract as team coverage above).
                if !self.clarify_is_deliverable(clarify_ctx.as_ref()).await {
                    let clarify_steps: Vec<&str> = def
                        .steps
                        .iter()
                        .filter(|s| s.is_clarify())
                        .map(|s| s.id.as_str())
                        .collect();
                    if !clarify_steps.is_empty() {
                        let surface = clarify_ctx.as_ref().map_or_else(
                            || "this run has no originating channel".to_string(),
                            |c| {
                                format!(
                                    "'{}' is not a channel the dispatcher can deliver to",
                                    c.channel_id
                                )
                            },
                        );
                        return Err(AlephError::invalid_input(format!(
                            "workflow '{name}' has clarify step(s) [{}] but there is no \
                             interactive channel to ask the user on ({surface}) — run it from a \
                             channel conversation (Telegram / Slack / …), or remove/replace the \
                             clarify step(s). Note the Panel's `gui:chat` is not a deliverable \
                             channel; use `ask_user` for an in-Panel question.",
                            clarify_steps.join(", "),
                        )));
                    }
                }
                // Plan ONCE before materialisation: the planner sees the run
                // input + WorkflowDef and produces a run-global Strategy. It does
                // not need the run_id (minted inside materialize). Fail-soft.
                let strategy = self.plan_workflow_strategy(&def, &inputs.input).await;
                // Capture the launching session for the goal-tree budget anchor
                // (`origin_session`) — the same wire `task_create` stamps, so a
                // goal-budgeted session launching a workflow has its member-run
                // spend accounted. None outside a turn (byte-identical rows).
                let origin_session = crate::tools::turn_context::current_session_key();
                let mat = workflow::materialize(
                    &def,
                    &inputs,
                    &team_id,
                    self.coord_store.as_ref(),
                    clarify_ctx.as_ref(),
                    (!pins.is_empty()).then_some(&pins),
                    strategy.as_ref(),
                    origin_session.as_deref(),
                )
                .await?;
                // Strategy delivery is metadata-only: `materialize` stamps the
                // rendered frame under `WORKFLOW_STRATEGY_KEY` on every agent
                // step and `handoff.rs` renders it as `## Global Strategy`. A
                // `strategies`-table row under `workflow:<run_id>` was also
                // written here for a while, but nothing ever read it
                // (`resolve_active_strategy` has no workflow tier) — each run
                // leaked one permanent orphan row. Removed (R10 zero-consumer).
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
                    pins: pin_rows,
                    ..WorkflowToolOutput::msg("run", message)
                })
            }
            WorkflowArgs::Status {
                name,
                team_id,
                run_id,
                include_output,
            } => {
                debug!(name = %name, team_id = %team_id, "workflow: status");
                let (run_id, tasks) = self.run_tasks(&name, &team_id, run_id.as_deref()).await?;
                let steps: Vec<WorkflowRunStep> =
                    tasks.iter().map(|t| step_row(t, include_output)).collect();
                // Phase grouping when the template declares phases — the shape
                // the `.workflow.js` live view reports and the one Aleph's flat
                // per-step list could not. Appended, not substituted: the
                // per-status counts stay the authoritative summary, and a
                // phase-less run's message is byte-identical to before.
                let message = match summarize_phases(&tasks) {
                    Some(phases) => format!(
                        "workflow '{name}' run {run_id}: {} | phases: {phases}",
                        summarize_statuses(&tasks)
                    ),
                    None => format!(
                        "workflow '{name}' run {run_id}: {}",
                        summarize_statuses(&tasks)
                    ),
                };
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
                // Mark-then-cancel: the cancelling user already knows this
                // run's fate, so stamp the durable notified marker BEFORE any
                // Cancelled write. The status writes below wake the
                // event-driven dispatcher within milliseconds — stamping
                // afterwards leaves a live window where the settle sweep sees
                // a fully-settled, unmarked run and pushes exactly the
                // redundant terminal summary this marker suppresses. A cancel
                // that errors midway leaves the marker set, which is fine —
                // the user initiated the cancel and has this tool's output.
                if let Some(anchor) = tasks.iter().min_by(|a, b| a.id.cmp(&b.id)) {
                    // Epoch-seconds value (not `true`): the settle sweep's
                    // reopen re-arm grace-gates on the stamp's age. Derived by
                    // the key's own module rather than re-derived here — the
                    // unit and the thing it is the unit of belong in one place.
                    let stamped_at = crate::workflow::compile::now_epoch_secs();
                    let merged = crate::agents::swarm::tasks::merge_metadata_patch(
                        &anchor.metadata,
                        serde_json::json!({
                            crate::workflow::WORKFLOW_NOTIFIED_KEY: stamped_at,
                            // This IS the stamper the re-arm grace exists for —
                            // written before the status writes below land. Say
                            // so, so the sweep applies the grace here and only
                            // here.
                            crate::workflow::WORKFLOW_NOTIFIED_BY_KEY:
                                crate::workflow::NOTIFIED_BY_CANCEL,
                        }),
                    );
                    if let Err(e) = self
                        .coord_store
                        .update_task(
                            &anchor.id,
                            CoordTaskUpdate {
                                metadata: Some(merged),
                                ..Default::default()
                            },
                        )
                        .await
                    {
                        tracing::warn!(run_id = %run_id, error = %e, "workflow cancel: failed to stamp notified marker");
                    }
                }
                let mut cancelled: Vec<String> = Vec::new();
                let mut in_flight = 0usize;
                let mut finished = 0usize;
                for task in &tasks {
                    // A step completing between the run_tasks snapshot and
                    // this iteration must not be clobbered Completed →
                    // Cancelled (mirror of the dispatcher's terminal-sticky
                    // guard). See `live_or_snapshot` for the degradation rule.
                    let live_status = self.live_or_snapshot(task).await.status;
                    match live_status {
                        // Terminal — already settled, leave untouched.
                        s if s.is_terminal() => finished += 1,
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
                        "; {in_flight} in-flight member run(s) are being stopped (the dispatcher \
                         cancels the live run of a task that went terminal on its next tick)"
                    ));
                }
                Ok(WorkflowToolOutput {
                    task_ids: Some(cancelled),
                    run_id: Some(run_id),
                    ..WorkflowToolOutput::msg("cancel", message)
                })
            }
            WorkflowArgs::Pause {
                name,
                team_id,
                run_id,
            } => {
                debug!(name = %name, team_id = %team_id, "workflow: pause");
                let (run_id, tasks) = self.run_tasks(&name, &team_id, run_id.as_deref()).await?;
                let mut paused: Vec<String> = Vec::new();
                let mut in_flight = 0usize;
                for task in &tasks {
                    // A step that completed / was cancelled / started between
                    // the snapshot and this iteration must not be clobbered to
                    // Paused — resume would then RE-EXECUTE finished work, and
                    // a paused terminal step blocks the run's settle accounting
                    // forever.
                    let live = self.live_or_snapshot(task).await;
                    match live.status {
                        // Not yet started — park it. (Unsatisfiable, though
                        // also stored as pending, is structurally dead and is
                        // deliberately NOT parked: pausing it would flip a
                        // settled corpse back to unsettled and block the
                        // run's terminal accounting.) Pending/Blocked need no
                        // origin stamp: Blocked is re-derived from stored
                        // pending at read time, so the Pending restore is
                        // lossless. Write an explicit NULL stamp anyway
                        // (mirror of `team_task_control` pause): a stale
                        // `paused_from=waiting_review` left by an earlier
                        // park→verdict-while-paused→retry chain would
                        // otherwise mis-restore this never-run step to
                        // WaitingReview on resume.
                        CoordTaskStatus::Pending | CoordTaskStatus::Blocked => {
                            let cleared = crate::agents::swarm::tasks::merge_metadata_patch(
                                &live.metadata,
                                serde_json::json!({
                                    crate::agents::swarm::tasks::PAUSED_FROM_KEY:
                                        serde_json::Value::Null,
                                }),
                            );
                            self.coord_store
                                .update_task(
                                    &task.id,
                                    CoordTaskUpdate {
                                        status: Some(CoordTaskStatus::Paused),
                                        metadata: Some(cleared),
                                        ..Default::default()
                                    },
                                )
                                .await?;
                            paused.push(task.id.clone());
                        }
                        // Already ran, awaiting the lead's verdict — park it
                        // WITH an origin stamp so resume restores
                        // WaitingReview instead of flattening to Pending
                        // (which would re-execute the finished run). Atomic
                        // park+stamp in one update. Verdicts still land while
                        // paused (review is not execution); resume then skips
                        // the no-longer-Paused task and the stale stamp is
                        // inert.
                        CoordTaskStatus::WaitingReview => {
                            let stamped = crate::agents::swarm::tasks::merge_metadata_patch(
                                &live.metadata,
                                serde_json::json!({
                                    crate::agents::swarm::tasks::PAUSED_FROM_KEY:
                                        crate::agents::swarm::tasks::PAUSED_FROM_WAITING_REVIEW,
                                }),
                            );
                            self.coord_store
                                .update_task(
                                    &task.id,
                                    CoordTaskUpdate {
                                        status: Some(CoordTaskStatus::Paused),
                                        metadata: Some(stamped),
                                        ..Default::default()
                                    },
                                )
                                .await?;
                            paused.push(task.id.clone());
                        }
                        // Running right now. The status is deliberately NOT
                        // touched — writing `Paused` over a live run makes the
                        // dispatcher's finalize fence keep the "foreign" state
                        // and throw the finished work away. Record the pause
                        // INTENT instead: `paused_from = "in_progress"` is
                        // durable, so if the daemon dies mid-run the orphan
                        // reclaim parks the step `Paused` rather than resetting
                        // it to `Pending` — which is how a pause used to be
                        // silently undone by a restart. If the run instead
                        // finishes normally, the stamp is inert (it is only ever
                        // read while a task is Paused, or by the orphan reclaim).
                        CoordTaskStatus::InProgress => {
                            in_flight += 1;
                            let stamped = crate::agents::swarm::tasks::merge_metadata_patch(
                                &live.metadata,
                                serde_json::json!({
                                    crate::agents::swarm::tasks::PAUSED_FROM_KEY:
                                        crate::agents::swarm::tasks::PAUSED_FROM_IN_PROGRESS,
                                }),
                            );
                            self.coord_store
                                .update_task(
                                    &task.id,
                                    CoordTaskUpdate {
                                        metadata: Some(stamped),
                                        ..Default::default()
                                    },
                                )
                                .await?;
                        }
                        _ => {}
                    }
                }
                let mut message = format!(
                    "paused {} step(s) of workflow '{name}' run {run_id}",
                    paused.len()
                );
                if in_flight > 0 {
                    message.push_str(&format!(
                        "; {in_flight} in-flight member run(s) will finish on their own — their \
                         downstream steps are paused, and a restart mid-run parks them paused \
                         instead of restarting them"
                    ));
                }
                message.push_str(" (resume with action='resume')");
                Ok(WorkflowToolOutput {
                    task_ids: Some(paused),
                    run_id: Some(run_id),
                    ..WorkflowToolOutput::msg("pause", message)
                })
            }
            WorkflowArgs::Resume {
                name,
                team_id,
                run_id,
            } => {
                debug!(name = %name, team_id = %team_id, "workflow: resume");
                let (run_id, tasks) = self.run_tasks(&name, &team_id, run_id.as_deref()).await?;
                let mut resumed: Vec<String> = Vec::new();
                let mut awaiting_reply = 0usize;
                for task in &tasks {
                    // A verdict landing between the snapshot and this write
                    // moves the task out of Paused — clobbering it back to
                    // Pending would re-execute the just-reviewed step.
                    let live = self.live_or_snapshot(task).await;
                    if live.status != CoordTaskStatus::Paused {
                        // Not paused — but it may still be carrying the pause
                        // INTENT this resume exists to cancel. `pause` stamps
                        // `paused_from = "in_progress"` on a live step without
                        // touching its status, and this loop's only writer sat
                        // behind the `Paused` filter, so the one stamp that is
                        // written to a non-Paused row was the one nothing ever
                        // cleared. The stamp is durable and the orphan reclaim
                        // reads it forever after: every future crash of this
                        // task parks it `Paused` — a pause nobody asked for,
                        // invisible to every janitor (they skip paused rows) —
                        // while `resume` has already reported success. The
                        // reclaim's own doc says "the stamp rides along and
                        // `workflow(action='resume')` clears it"; this is the
                        // line that makes that sentence true.
                        if crate::agents::swarm::tasks::paused_from(&live.metadata).is_some() {
                            let cleared = crate::agents::swarm::tasks::merge_metadata_patch(
                                &live.metadata,
                                serde_json::json!({
                                    crate::agents::swarm::tasks::PAUSED_FROM_KEY:
                                        serde_json::Value::Null,
                                }),
                            );
                            if let Err(e) = self
                                .coord_store
                                .update_task(
                                    &task.id,
                                    CoordTaskUpdate {
                                        metadata: Some(cleared),
                                        ..Default::default()
                                    },
                                )
                                .await
                            {
                                tracing::warn!(
                                    task_id = %task.id, error = %e,
                                    "workflow resume: could not clear stale pause intent"
                                );
                            }
                        }
                        continue;
                    }
                    // A clarify step the dispatcher parked is Paused because it
                    // AWAITS the user's reply (delivered marker) or is mid-
                    // delivery (pending marker, the janitor's business) — its
                    // Paused state is not ours to undo.
                    if crate::workflow::clarify::clarify_delivered(&live.metadata) {
                        awaiting_reply += 1;
                        continue;
                    }
                    if crate::workflow::clarify::clarify_delivery_pending_at(&live.metadata)
                        .is_some()
                    {
                        continue;
                    }
                    // Restore the pause-origin status (WaitingReview for
                    // review-parked steps; Pending otherwise — Blocked is
                    // re-derived from stored pending) and clear the stamp in
                    // the same atomic write.
                    let restore = match crate::agents::swarm::tasks::paused_from(&live.metadata) {
                        Some(crate::agents::swarm::tasks::PAUSED_FROM_WAITING_REVIEW) => {
                            CoordTaskStatus::WaitingReview
                        }
                        _ => CoordTaskStatus::Pending,
                    };
                    let cleared = crate::agents::swarm::tasks::merge_metadata_patch(
                        &live.metadata,
                        serde_json::json!({
                            crate::agents::swarm::tasks::PAUSED_FROM_KEY: serde_json::Value::Null,
                        }),
                    );
                    self.coord_store
                        .update_task(
                            &task.id,
                            CoordTaskUpdate {
                                status: Some(restore),
                                metadata: Some(cleared),
                                ..Default::default()
                            },
                        )
                        .await?;
                    resumed.push(task.id.clone());
                }
                if let Some(signal) = &self.dispatch_signal {
                    signal.notify_one();
                }
                let mut message = format!(
                    "resumed {} step(s) of workflow '{name}' run {run_id}",
                    resumed.len()
                );
                if awaiting_reply > 0 {
                    message.push_str(&format!(
                        "; {awaiting_reply} clarify step(s) stay parked awaiting the user's reply"
                    ));
                }
                Ok(WorkflowToolOutput {
                    task_ids: Some(resumed),
                    run_id: Some(run_id),
                    ..WorkflowToolOutput::msg("resume", message)
                })
            }
            WorkflowArgs::Runs { name, team_id } => {
                debug!(name = %name, team_id = %team_id, "workflow: runs");
                let groups = self.run_groups(&name, &team_id).await?;
                let mut runs: Vec<WorkflowRunSummary> = groups
                    .into_iter()
                    .map(|(run_id, tasks)| WorkflowRunSummary {
                        run_id,
                        // The run's BIRTH, not its latest activity: `status`
                        // selects the latest run by max(created_at), and a row
                        // labelled "started_at" that moved as steps were
                        // created would be a different fact wearing the same
                        // name.
                        started_at: tasks.iter().map(|t| t.created_at).min().unwrap_or(0),
                        steps: tasks.len(),
                        // The dispatcher's own completion predicate. Hand-listing
                        // the terminal statuses here would be a second copy of a
                        // set that has already grown once (`Unsatisfiable` is
                        // settled without being terminal).
                        settled: tasks.iter().all(|t| t.status.is_settled()),
                        summary: summarize_statuses(&tasks),
                    })
                    .collect();
                // Newest first — the run a caller means when they say "the
                // run". `created_at` is epoch seconds, so runs started in the
                // same second tie; the run id breaks it deterministically.
                runs.sort_by(|a, b| {
                    b.started_at
                        .cmp(&a.started_at)
                        .then_with(|| b.run_id.cmp(&a.run_id))
                });
                let unsettled = runs.iter().filter(|r| !r.settled).count();
                // "None" is an answer this face can give; it is not a failure.
                // Erroring here (inherited from `run_groups`) made "never run
                // on this team" read like a broken listing, and a caller
                // cannot tell those apart from an `Err`.
                let message = if runs.is_empty() {
                    format!("no runs of '{name}' on team '{team_id}' — start one with action='run'")
                } else {
                    format!(
                        "{} run(s) of workflow '{name}' on team '{team_id}', newest first \
                         ({unsettled} still running) — inspect one with action='status', re-arm \
                         its failures with action='rerun_failed'",
                        runs.len()
                    )
                };
                Ok(WorkflowToolOutput {
                    runs: Some(runs),
                    ..WorkflowToolOutput::msg("runs", message)
                })
            }
            WorkflowArgs::RerunFailed {
                name,
                team_id,
                run_id,
            } => {
                debug!(name = %name, team_id = %team_id, "workflow: rerun_failed");
                let (run_id, tasks) = self.run_tasks(&name, &team_id, run_id.as_deref()).await?;
                let mut rearmed: Vec<String> = Vec::new();
                // Decide the SET from the one snapshot, before any write.
                //
                // `Unsatisfiable` rides along with `Failed` on purpose: it is
                // not a failure of its own, it is the shadow one casts down the
                // DAG. But it is also DERIVED — the row is stored pending and
                // reads unsatisfiable only while an upstream is terminally
                // failed — so the first write of this loop stops the downstream
                // rows from matching. Re-reading membership per iteration would
                // therefore make the set depend on the order the steps happen
                // to be visited rather than on the run's state: select in one
                // order and apply in another and you have named a different set
                // (the run's own criterion 12).
                let targets: Vec<&CoordTask> = tasks
                    .iter()
                    .filter(|t| {
                        matches!(
                            t.status,
                            CoordTaskStatus::Failed | CoordTaskStatus::Unsatisfiable
                        )
                    })
                    .collect();
                for task in targets {
                    let live = self.live_or_snapshot(task).await;
                    // The live read guards only against the step having moved
                    // on under us between the listing and this write — a late
                    // retry that succeeded, a step someone restarted, a verdict
                    // that landed. `Pending`/`Blocked` here are this loop's own
                    // earlier writes rippling down the DAG, not a change of
                    // mind, so they must not disqualify a selected step.
                    if matches!(
                        live.status,
                        CoordTaskStatus::InProgress
                            | CoordTaskStatus::Completed
                            | CoordTaskStatus::WaitingReview
                            | CoordTaskStatus::Skipped
                            | CoordTaskStatus::Cancelled
                    ) {
                        continue;
                    }
                    // Snapshot the leftover lock holder BEFORE the reset, so it
                    // can be released with its ACTUAL holder below — the store
                    // checks holder equality, so releasing with "" never clears
                    // a genuinely held lock and would leave the step
                    // pending-but-unschedulable until the stale-lock sweep runs
                    // (the same reason `workflow_step_review`'s retry arm does
                    // this).
                    let locked_by = live.locked_by.clone();
                    // Re-arm the automatic retry ladder: without the budget
                    // anchor the step dies on its first new failure, having
                    // already spent its attempts on the cause the caller just
                    // fixed.
                    let metadata = crate::agents::swarm::tasks::retry::with_retry_budget_reset_at(
                        live.metadata.clone(),
                        crate::workflow::compile::now_epoch_secs(),
                    );
                    self.coord_store
                        .update_task(
                            &live.id,
                            CoordTaskUpdate {
                                status: Some(CoordTaskStatus::Pending),
                                // Clear the failure text: it describes the
                                // attempt being replaced, and `status` would
                                // otherwise report it against a step that is
                                // queued to run again.
                                result: Some(String::new()),
                                metadata: Some(metadata),
                                ..Default::default()
                            },
                        )
                        .await?;
                    if let Some(holder) = locked_by.as_deref() {
                        if let Err(e) = self.coord_store.release_lock(&live.id, holder).await {
                            tracing::warn!(
                                task_id = %live.id, holder = %holder, error = %e,
                                "workflow rerun_failed: could not release leftover lock"
                            );
                        }
                    }
                    rearmed.push(live.id.clone());
                }
                // Deliberately NO `workflow_notified` stamp — the opposite of
                // `cancel`. The settle sweep re-arms its own terminal
                // notification the moment the run stops being fully settled,
                // which is precisely what these writes just did; stamping here
                // would suppress the summary for the re-run.
                if !rearmed.is_empty() {
                    if let Some(signal) = &self.dispatch_signal {
                        signal.notify_one();
                    }
                }
                // Zero matches is an honest answer, not an error: "there is
                // nothing failed in this run" is exactly what the caller asked
                // and exactly what they want to hear.
                let message = if rearmed.is_empty() {
                    format!(
                        "nothing to rerun in workflow '{name}' run {run_id}: no step is failed or \
                         unsatisfiable ({})",
                        summarize_statuses(&tasks)
                    )
                } else {
                    format!(
                        "re-armed {} step(s) of workflow '{name}' run {run_id} — they are queued \
                         again with a fresh retry budget (inspect with action='status')",
                        rearmed.len()
                    )
                };
                Ok(WorkflowToolOutput {
                    task_ids: Some(rearmed),
                    run_id: Some(run_id),
                    ..WorkflowToolOutput::msg("rerun_failed", message)
                })
            }
            WorkflowArgs::Export { name, write_file } => {
                debug!(name = %name, write_file, "workflow: export");
                // The stored manifest carries the full `.workflow.js` metadata,
                // so the render is now faithful (phases, per-step
                // label/model/phase/schema) rather than a bare skeleton.
                let manifest = workflow::store::load(&name)?;
                let rendered = workflow::render_workflow_js(&manifest);
                // The rendered file already discloses partial fan-in in a `//`
                // block — but a caller exporting through this tool reads the
                // MESSAGE, not the file, and would be told nothing about the
                // one thing the body cannot express. Same predicate, second
                // face (criterion 9): re-importing a header-stripped copy of
                // this file widens those steps' dependency sets.
                let lossy = crate::workflow::interop::export::partial_fan_in_notes(&manifest);
                let message = if write_file {
                    // `.mjs` — the extension Claude Code's workflow menu / the
                    // `~/.claude/workflows` loader recognise for a dynamic
                    // workflow (the reference engineering files are `*.mjs`). The
                    // rendered body/embed-header are unchanged; only the on-disk
                    // extension moves off Aleph's legacy `.workflow.js`.
                    let path = workflow::store::write_text(&name, "mjs", &rendered)?;
                    format!("exported workflow '{name}' → {}", path.display())
                } else {
                    format!("rendered workflow '{name}' ({} bytes)", rendered.len())
                };
                let message = if lossy.is_empty() {
                    message
                } else {
                    format!(
                        "{message} — NOTE: partial fan-in, the body skeleton cannot express it \
                         ({}); keep the @aleph-workflow header at the top of the file or a \
                         re-import will widen those steps' dependencies",
                        lossy.join("; ")
                    )
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
                    // Name what only survives on disk. `import(save=false)`
                    // hands back a lean `WorkflowDef`, and this tool's own
                    // remediation advice is "retarget the agents (edit + save)"
                    // — which, with nothing stored yet, routes every extra
                    // through `from_def` and deletes it. `save`'s preservation
                    // path only fires on OVERWRITE, so the flow the import
                    // face advertises is exactly the one it cannot protect.
                    let extras = outcome.manifest.def_inexpressible_extras();
                    if extras.is_empty() {
                        format!(
                            "parsed workflow '{}' ({} step(s); not saved)",
                            def.name,
                            def.steps.len()
                        )
                    } else {
                        format!(
                            "parsed workflow '{}' ({} step(s); not saved). It carries {} that a \
                             plain definition cannot hold — re-run with save=true FIRST, then \
                             edit and save, or those are dropped on the first save.",
                            def.name,
                            def.steps.len(),
                            extras.join(", ")
                        )
                    }
                };
                Ok(WorkflowToolOutput {
                    definition: Some(def),
                    dropped: Some(outcome.dropped),
                    ..WorkflowToolOutput::msg("import", message)
                })
            }
            WorkflowArgs::Proposals {} => {
                let listing = workflow::proposal::list_proposals()?;
                // Same row shape as `list`: a draft's provenance (which skill
                // chain, how many observations) lives in its `description`, so
                // returning bare names forced a `describe_proposal` per draft
                // just to see what any of them were about.
                let workflows: Vec<WorkflowListEntry> = listing
                    .entries
                    .into_iter()
                    .map(|m| WorkflowListEntry {
                        // A draft is not runnable until accepted, so its vars
                        // are informational — but omitting them here would make
                        // `proposals` the one listing face that hides an input
                        // requirement.
                        vars: workflow::proposal::load_proposal(&m.name)
                            .map(|man| referenced_vars(&man))
                            .unwrap_or_default(),
                        name: m.name,
                        description: m.description,
                        when_to_use: m.when_to_use,
                        steps: m.steps,
                    })
                    .collect();
                let message = format!(
                    "{} gated MetaSkill proposal(s) — inspect one with \
                     action='describe_proposal', activate with action='accept_proposal'",
                    workflows.len()
                );
                Ok(WorkflowToolOutput {
                    workflows: Some(workflows),
                    problems: (!listing.problems.is_empty()).then_some(listing.problems),
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
            WorkflowArgs::RejectProposal { name } => {
                debug!(name = %name, "workflow: reject_proposal");
                // Tombstone tradeoff, decided deliberately: deleting the draft
                // also deletes the miner's name-based dedup anchor, so the same
                // skill chain CAN be re-drafted on a later dream cycle. That is
                // the intended reading — a rejection means "not now", and a
                // chain that keeps recurring is worth re-surfacing, rather than
                // being silenced forever by an invisible tombstone file.
                let removed = workflow::proposal::delete_proposal(&name)?;
                let message = if removed {
                    format!(
                        "rejected proposal '{name}' — draft removed (the dream miner may \
                         re-draft this chain if it keeps recurring)"
                    )
                } else {
                    format!("proposal '{name}' did not exist")
                };
                Ok(WorkflowToolOutput::msg("reject_proposal", message))
            }
        }
    }
}

#[cfg(test)]
mod catalog_contract {
    /// The catalog entry must POINT AT the tool's own `DESCRIPTION`, never
    /// restate it. A hand-written literal there SHADOWS the const — `agent_init`
    /// builds the model's tool table from this catalog first and only appends
    /// names the catalog lacks — and the literal that used to sit here
    /// enumerated five of the fifteen actions, so cancel / pause / resume /
    /// status / export / import / the proposal family were never advertised at
    /// all. Assert on the shipped catalog, not on the const: asserting on the
    /// const is exactly what stayed green while the model received nothing.
    #[test]
    fn the_shipped_workflow_entry_is_the_tools_own_description() {
        let entry = crate::executor::BUILTIN_TOOL_DEFINITIONS
            .iter()
            .find(|d| d.name == "workflow")
            .expect("the workflow tool is in the static catalog");
        for action in ["cancel", "pause", "resume", "status", "import", "export"] {
            assert!(
                entry.description.contains(action),
                "the model never learns about `{action}`: {}",
                entry.description
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::{store::SqliteCoordTaskStore, CoordTaskStatus};

    use crate::workflow::def::WorkflowStepDef;
    use rusqlite::Connection;
    use tempfile::TempDir;

    // `ALEPH_HOME` is process-global; the file-backed actions (save/list/
    // describe/delete/run-load) resolve their directory from it via
    // `workflow::store::*`. Serialise every test that touches it through the
    // shared guard so parallel `cargo test` threads can't read/write each other's
    // workflows dir. Pure serde/notify tests below need no env and skip it.
    use crate::utils::paths::ALEPH_HOME_TEST_GUARD as ENV_GUARD;

    async fn setup_store() -> SqliteCoordTaskStore {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.expect("migrate");
        store
    }

    /// The names of a `list` / `proposals` result, in listing order. `None`
    /// when the output carries no listing at all (non-list actions).
    fn listed_names(out: &WorkflowToolOutput) -> Option<Vec<String>> {
        out.workflows
            .as_ref()
            .map(|rows| rows.iter().map(|r| r.name.clone()).collect())
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
                    require_grounding: false,
                    tolerate_failed_deps: false,
                    timeout_seconds: None,
                    max_retries: None,
                },
                WorkflowStepDef {
                    id: "write".into(),
                    agent: "writer".into(),
                    prompt: "write a report".into(),
                    depends_on: vec!["gather".into()],
                    kind: crate::workflow::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                    require_grounding: false,
                    tolerate_failed_deps: false,
                    timeout_seconds: None,
                    max_retries: None,
                },
            ],
        }
    }

    /// One agent step — the shapes below differ only in id/agent/deps, and a
    /// literal per step means every new `WorkflowStepDef` field edits a dozen
    /// fixtures instead of one.
    fn step(id: &str, agent: &str, deps: &[&str]) -> WorkflowStepDef {
        WorkflowStepDef {
            id: id.into(),
            agent: agent.into(),
            prompt: format!("do {id}"),
            depends_on: deps.iter().map(|d| (*d).to_string()).collect(),
            kind: crate::workflow::WorkflowStepKind::Agent,
            choices: vec![],
            review: false,
            require_grounding: false,
            tolerate_failed_deps: false,
            timeout_seconds: None,
            max_retries: None,
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
                args,
            } => {
                assert_eq!(name, "p");
                assert_eq!(team_id, "t");
                assert_eq!(input, "", "missing input defaults to empty string");
                assert!(args.is_empty(), "missing args defaults to no named vars");
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
        assert!(out.workflows.is_none());
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
                    args: std::collections::HashMap::new(),
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
        assert!(run_out.workflows.is_none());
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
                args: std::collections::HashMap::new(),
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
                args: std::collections::HashMap::new(),
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
                args: std::collections::HashMap::new(),
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
        let mat = workflow::materialize(
            def,
            &RunInputs::from_input("x"),
            team,
            t.coord_store.as_ref(),
            None,
            None,
            None,
            None,
        )
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
                include_output: false,
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
                include_output: false,
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
                include_output: false,
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
                include_output: false,
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
    async fn pause_parks_waiting_review_and_resume_restores_it() {
        let store = setup_store().await;
        let t = tool(store, None);
        let (run_id, ids) = materialize_run(&t, &linear_def(), "team-9").await;

        // Root ran and awaits the lead's verdict; dependent still pending.
        t.coord_store
            .update_task(
                &ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::WaitingReview),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let out = t
            .call(WorkflowArgs::Pause {
                name: "pipeline".into(),
                team_id: "team-9".into(),
                run_id: Some(run_id.clone()),
            })
            .await
            .expect("pause");
        let paused = out.task_ids.as_ref().expect("paused ids");
        assert!(
            paused.contains(&ids[0]),
            "review-parked step must be paused too: {paused:?}"
        );
        let root = t.coord_store.get_task(&ids[0]).await.unwrap().unwrap();
        assert_eq!(root.status, CoordTaskStatus::Paused);
        assert_eq!(
            crate::agents::swarm::tasks::paused_from(&root.metadata),
            Some(crate::agents::swarm::tasks::PAUSED_FROM_WAITING_REVIEW)
        );

        let out = t
            .call(WorkflowArgs::Resume {
                name: "pipeline".into(),
                team_id: "team-9".into(),
                run_id: Some(run_id),
            })
            .await
            .expect("resume");
        assert!(out.task_ids.as_ref().unwrap().contains(&ids[0]));
        let root = t.coord_store.get_task(&ids[0]).await.unwrap().unwrap();
        assert_eq!(
            root.status,
            CoordTaskStatus::WaitingReview,
            "resume must restore the review gate, not flatten to Pending"
        );
        assert_eq!(
            crate::agents::swarm::tasks::paused_from(&root.metadata),
            None,
            "stamp cleared on restore"
        );
    }

    /// W15b: pausing a run whose step is mid-flight used to be pure
    /// bookkeeping — the step was counted as "in flight" and nothing was
    /// written, so a daemon restart put it back to Pending via
    /// `reclaim_orphaned` and the dispatcher re-ran the step the user had
    /// paused. Silent: no error, and `workflow status` still said paused.
    ///
    /// The whole chain is asserted, including the part that is easy to leave
    /// out: a task parked `Paused` is invisible to BOTH janitors
    /// (`reclaim_zombies` / `abandon_orphaned_runs` are InProgress-scoped), so
    /// the stamp has to provably disappear on resume or this step loses its
    /// watchdog forever.
    #[tokio::test]
    async fn a_pause_during_a_live_step_survives_the_restart_and_is_cleared_by_resume() {
        use crate::agents::swarm::tasks::{paused_from, PAUSED_FROM_IN_PROGRESS};
        use crate::teams::dispatcher::schedule::orphan_reset_status;

        let store = setup_store().await;
        let t = tool(store, None);
        let (run_id, ids) = materialize_run(&t, &linear_def(), "team-9").await;

        // The root step is executing; the pause cannot stop it.
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

        t.call(WorkflowArgs::Pause {
            name: "pipeline".into(),
            team_id: "team-9".into(),
            run_id: Some(run_id.clone()),
        })
        .await
        .expect("pause with a live step");

        let root = t.coord_store.get_task(&ids[0]).await.unwrap().unwrap();
        assert_eq!(
            root.status,
            CoordTaskStatus::InProgress,
            "the live run's status must not be clobbered — the finalize fence \
             would throw its finished work away"
        );
        assert_eq!(
            paused_from(&root.metadata),
            Some(PAUSED_FROM_IN_PROGRESS),
            "the pause intent must be durable, not just counted in the message"
        );

        // Daemon dies mid-run; the next boot's orphan reclaim decides where the
        // row goes. Asserted through the dispatcher's own decision function.
        assert_eq!(
            orphan_reset_status(&root),
            CoordTaskStatus::Paused,
            "a restart must not resurrect a paused step"
        );
        t.coord_store
            .update_task(
                &ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Paused),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Resume: back to Pending AND the stamp gone. Without the clear, this
        // row could be parked Paused again by any later crash while no janitor
        // can see it.
        t.call(WorkflowArgs::Resume {
            name: "pipeline".into(),
            team_id: "team-9".into(),
            run_id: Some(run_id),
        })
        .await
        .expect("resume");
        let root = t.coord_store.get_task(&ids[0]).await.unwrap().unwrap();
        assert_eq!(root.status, CoordTaskStatus::Pending);
        assert_eq!(
            paused_from(&root.metadata),
            None,
            "resume must clear the stamp — it is the only thing that returns \
             this row to janitor visibility"
        );
        assert_eq!(
            orphan_reset_status(&root),
            CoordTaskStatus::Pending,
            "and the reclaim decision must follow the cleared stamp"
        );
    }

    /// The other half of the pause-during-a-live-step chain: the daemon does
    /// **not** crash.
    ///
    /// The test above forces the row to `Paused` before resuming, so it only
    /// ever exercised the post-restart path. In the ordinary case the step is
    /// still `InProgress` when the user resumes, and the resume loop's
    /// `status != Paused` short-circuit skipped it — leaving the pause intent
    /// stamped forever on a row that `resume` had just reported as resumed.
    /// From then on every crash of that task parks it `Paused`, which is
    /// invisible to both janitors, so the step loses its watchdog for a pause
    /// that was explicitly lifted.
    #[tokio::test]
    async fn resuming_while_the_step_is_still_live_clears_the_pause_intent() {
        use crate::agents::swarm::tasks::{paused_from, PAUSED_FROM_IN_PROGRESS};
        use crate::teams::dispatcher::schedule::orphan_reset_status;

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

        t.call(WorkflowArgs::Pause {
            name: "pipeline".into(),
            team_id: "team-9".into(),
            run_id: Some(run_id.clone()),
        })
        .await
        .expect("pause with a live step");
        let root = t.coord_store.get_task(&ids[0]).await.unwrap().unwrap();
        assert_eq!(paused_from(&root.metadata), Some(PAUSED_FROM_IN_PROGRESS));

        // No crash: the step is still running when the user changes their mind.
        t.call(WorkflowArgs::Resume {
            name: "pipeline".into(),
            team_id: "team-9".into(),
            run_id: Some(run_id),
        })
        .await
        .expect("resume");

        let root = t.coord_store.get_task(&ids[0]).await.unwrap().unwrap();
        assert_eq!(
            root.status,
            CoordTaskStatus::InProgress,
            "resume must not clobber a live run's status"
        );
        assert_eq!(
            paused_from(&root.metadata),
            None,
            "the pause intent must be gone — resume is what cancels it"
        );
        assert_eq!(
            orphan_reset_status(&root),
            CoordTaskStatus::Pending,
            "a later crash must reclaim this step normally, not park it Paused"
        );
    }

    #[tokio::test]
    async fn pause_after_stale_review_stamp_restores_pending_not_waiting_review() {
        // Regression (W4): the full chain park → verdict-while-paused → retry →
        // pause → resume. The review pause stamps `paused_from=waiting_review`;
        // a verdict landing while paused moves the task out of Paused, and NO
        // retry face nulls the stamp. The later run-level pause of the (now
        // Pending, never-run) step must therefore write an explicit null stamp,
        // or resume mis-restores it to WaitingReview — inviting the lead to
        // approve a step whose result was cleared.
        let store = setup_store().await;
        let t = tool(store, None);
        let (run_id, ids) = materialize_run(&t, &linear_def(), "team-9").await;

        // 1. Root ran and awaits the lead's verdict.
        t.coord_store
            .update_task(
                &ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::WaitingReview),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // 2. Run-level pause parks it WITH the waiting_review origin stamp.
        t.call(WorkflowArgs::Pause {
            name: "pipeline".into(),
            team_id: "team-9".into(),
            run_id: Some(run_id.clone()),
        })
        .await
        .expect("pause review-parked step");
        // 3. A reject verdict lands while paused (verdicts land regardless of
        //    pause — review is not execution). The stale stamp stays behind.
        t.coord_store
            .update_task(
                &ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Failed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // 4. Operator hard-retries through the real admin face (which re-arms
        //    the retry budget but does NOT touch the pause stamp).
        let control = crate::builtin_tools::team::task_control::TeamTaskControlTool::new(
            t.coord_store.clone(),
        );
        control
            .call(
                crate::builtin_tools::team::task_control::TeamTaskControlArgs::Retry {
                    task_id: ids[0].clone(),
                },
            )
            .await
            .expect("hard-retry the rejected step");
        // 5. Run-level pause again — now via the Pending/Blocked arm, which
        //    must null the stale stamp.
        t.call(WorkflowArgs::Pause {
            name: "pipeline".into(),
            team_id: "team-9".into(),
            run_id: Some(run_id.clone()),
        })
        .await
        .expect("pause the retried (pending) step");
        // 6. Resume must restore Pending — NOT the stale WaitingReview, whose
        //    run result was cleared by the retry.
        t.call(WorkflowArgs::Resume {
            name: "pipeline".into(),
            team_id: "team-9".into(),
            run_id: Some(run_id),
        })
        .await
        .expect("resume");
        let root = t.coord_store.get_task(&ids[0]).await.unwrap().unwrap();
        assert_eq!(
            root.status,
            CoordTaskStatus::Pending,
            "a never-run pending step must not resume into WaitingReview"
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
                    require_grounding: false,
                    tolerate_failed_deps: false,
                    timeout_seconds: None,
                    max_retries: None,
                },
                WorkflowStepDef {
                    id: "run".into(),
                    agent: "deployer".into(),
                    prompt: "deploy".into(),
                    depends_on: vec!["ask".into()],
                    kind: crate::workflow::WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                    require_grounding: false,
                    tolerate_failed_deps: false,
                    timeout_seconds: None,
                    max_retries: None,
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

    #[tokio::test]
    async fn run_via_call_rejected_by_preflight_creates_zero_tasks() {
        // End-to-end ordering guarantee through call(): preflight runs BEFORE
        // materialize, so a rejected run leaves the coord store empty instead
        // of half-queuing a DAG the dispatcher then fails in the background.
        let tmp = TempDir::new().unwrap();
        let (team_store, team_id) = team_with(&["researcher"]).await; // no writer
        let store = setup_store().await;
        let t = tool(store, None).with_team_store(Some(team_store));

        let res = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            workflow::store::save(&WorkflowManifest::from_def(&linear_def())).expect("save");
            let r = t
                .call(WorkflowArgs::Run {
                    name: "pipeline".into(),
                    team_id,
                    input: "x".into(),
                    args: std::collections::HashMap::new(),
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
        };

        let err = res.expect_err("uncovered agent rejects the run via call()");
        assert!(
            err.to_string().contains("writer"),
            "error names the missing agent: {err}"
        );
        let tasks = t
            .coord_store
            .list_tasks(CoordTaskFilter::default())
            .await
            .expect("list tasks");
        assert!(
            tasks.is_empty(),
            "preflight rejection must not materialise any coord_task"
        );
    }

    #[tokio::test]
    async fn run_via_call_succeeds_when_team_covers_all_agents() {
        // Companion positive path: with a team store wired and full coverage,
        // run proceeds through preflight and materialises one task per step.
        let tmp = TempDir::new().unwrap();
        let (team_store, team_id) = team_with(&["researcher", "writer"]).await;
        let store = setup_store().await;
        let t = tool(store, None).with_team_store(Some(team_store));

        let out = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            workflow::store::save(&WorkflowManifest::from_def(&linear_def())).expect("save");
            let r = t
                .call(WorkflowArgs::Run {
                    name: "pipeline".into(),
                    team_id,
                    input: "x".into(),
                    args: std::collections::HashMap::new(),
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
        .expect("covered team → run succeeds");

        assert_eq!(out.action, "run");
        assert_eq!(
            out.task_ids.as_ref().map(|v| v.len()),
            Some(2),
            "one task per step"
        );
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
        assert_eq!(
            listed_names(&listed).as_deref(),
            Some(&["scanned".to_string()][..])
        );
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
        assert_eq!(listed_names(&empty).as_deref(), Some(&[][..]));
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
        assert!(
            saved.workflows.is_none() && saved.definition.is_none() && saved.task_ids.is_none()
        );

        // list reflects the saved template — only names populated.
        let listed = t.call(WorkflowArgs::List {}).await.expect("list");
        assert_eq!(
            listed_names(&listed).as_deref(),
            Some(&["pipeline".to_string()][..])
        );
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
        assert!(described.workflows.is_none() && described.task_ids.is_none());

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
        assert_eq!(listed_names(&after).as_deref(), Some(&[][..]));

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

    #[tokio::test]
    async fn proposals_then_accept_promotes_draft_to_active_store() {
        // The gate transition through the tool: a drafted proposal is listed by
        // `proposals`, promoted by `accept_proposal` (draft removed, active copy
        // visible to list/describe), and an unknown name errors.
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let (name, listed, accepted, after, active, described, missing) = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            let draft =
                workflow::proposal::skeleton_from_chain(&["research".into(), "write".into()], 2)
                    .expect("two-skill chain drafts a skeleton");
            let name = draft.name.clone();
            workflow::proposal::save_proposal(&draft).expect("persist gated draft");

            let listed = t.call(WorkflowArgs::Proposals {}).await.expect("proposals");
            let accepted = t
                .call(WorkflowArgs::AcceptProposal { name: name.clone() })
                .await
                .expect("accept the gated draft");
            let after = t
                .call(WorkflowArgs::Proposals {})
                .await
                .expect("proposals after accept");
            let active = t.call(WorkflowArgs::List {}).await.expect("list");
            let described = t
                .call(WorkflowArgs::Describe { name: name.clone() })
                .await
                .expect("describe the now-active workflow");
            let missing = t
                .call(WorkflowArgs::AcceptProposal {
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
            (name, listed, accepted, after, active, described, missing)
        };

        assert_eq!(listed.action, "proposals");
        assert!(
            listed_names(&listed).is_some_and(|n| n.contains(&name)),
            "draft is listed before accept"
        );
        assert_eq!(accepted.action, "accept_proposal");
        assert!(
            accepted.message.contains(&name),
            "accept names the promoted workflow: {}",
            accepted.message
        );
        assert_eq!(
            listed_names(&after).as_deref(),
            Some(&[][..]),
            "accepted draft is removed from the gated dir"
        );
        assert!(
            listed_names(&active).is_some_and(|n| n.contains(&name)),
            "accepted workflow appears in the active store"
        );
        assert!(
            described.definition.is_some(),
            "describe resolves the promoted workflow"
        );
        assert!(missing.is_err(), "accepting an unknown proposal errors");
    }

    #[tokio::test]
    async fn reject_proposal_removes_draft_without_activating() {
        // The R8 counterpart to accept: dismissing a bad auto-drafted MetaSkill
        // deletes the gated draft (idempotently) and never touches the active
        // store. Note the tombstone tradeoff documented at the handler: once
        // the file is gone, name-based dedup allows the miner to re-draft the
        // same chain on a later dream cycle.
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let (rejected, after, active, again) = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            let draft =
                workflow::proposal::skeleton_from_chain(&["research".into(), "write".into()], 2)
                    .expect("two-skill chain drafts a skeleton");
            let name = draft.name.clone();
            workflow::proposal::save_proposal(&draft).expect("persist gated draft");

            let rejected = t
                .call(WorkflowArgs::RejectProposal { name: name.clone() })
                .await
                .expect("reject the gated draft");
            let after = t
                .call(WorkflowArgs::Proposals {})
                .await
                .expect("proposals after reject");
            let active = t.call(WorkflowArgs::List {}).await.expect("list");
            // Idempotent: rejecting again reports the draft is gone, no error.
            let again = t
                .call(WorkflowArgs::RejectProposal { name })
                .await
                .expect("second reject is idempotent");

            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            (rejected, after, active, again)
        };

        assert_eq!(rejected.action, "reject_proposal");
        assert!(
            rejected.message.contains("rejected proposal"),
            "{}",
            rejected.message
        );
        assert_eq!(
            listed_names(&after).as_deref(),
            Some(&[][..]),
            "rejected draft is removed from the gated dir"
        );
        assert_eq!(
            listed_names(&active).as_deref(),
            Some(&[][..]),
            "rejection never touches the active store"
        );
        assert!(
            again.message.contains("did not exist"),
            "idempotent branch: {}",
            again.message
        );
    }

    #[tokio::test]
    async fn run_stamps_per_step_effort_onto_task_metadata() {
        // The executable effort wire end-to-end through the tool: a manifest
        // pinning `effort` on one step materialises that step with
        // WORKFLOW_EFFORT_KEY (readable by the dispatcher's think-level
        // resolver); the unpinned step stays byte-identical.
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let run_out = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            let mut m = WorkflowManifest::from_def(&linear_def());
            m.steps[0].effort = Some("max".into());
            workflow::store::save(&m).expect("save effort-pinned template");
            let r = t
                .call(WorkflowArgs::Run {
                    name: "pipeline".into(),
                    team_id: "team-7".into(),
                    input: "x".into(),
                    args: std::collections::HashMap::new(),
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
        .expect("run");

        let ids = run_out.task_ids.as_ref().expect("run populates task_ids");
        let gather = t.coord_store.get_task(&ids[0]).await.unwrap().unwrap();
        assert_eq!(
            crate::workflow::workflow_effort_think_level(&gather.metadata),
            Some(crate::agents::thinking::ThinkLevel::High),
            "pinned step carries the effort override ('max' → High)"
        );
        let write = t.coord_store.get_task(&ids[1]).await.unwrap().unwrap();
        assert!(
            crate::workflow::workflow_effort_think_level(&write.metadata).is_none(),
            "unpinned step has no effort key"
        );
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
        let strategy = t
            .plan_workflow_strategy(&linear_def(), "do the thing")
            .await;
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
        let strategy = t
            .plan_workflow_strategy(&linear_def(), "do the thing")
            .await;
        assert!(
            strategy.is_some(),
            "provider + concrete guardrail => Some(strategy)"
        );
    }

    // --- per-step model surfacing (R8 observability) ---

    /// A manifest mirroring `linear_def()` but with the `gather` step pinned to a
    /// model and `write` left on the agent default — exercising both the
    /// "has a model" and "no model" projection branches.
    fn manifest_with_pinned_model() -> WorkflowManifest {
        let mut m = WorkflowManifest::from_def(&linear_def());
        m.steps[0].model = Some("opus".into());
        m
    }

    #[test]
    fn manifest_step_pins_projects_only_pinned_steps() {
        let rows = manifest_step_pins(&manifest_with_pinned_model())
            .expect("at least one step pins something");
        assert_eq!(rows.len(), 1, "only the pinned step appears");
        assert_eq!(rows[0].step, "gather");
        assert_eq!(rows[0].model.as_deref(), Some("opus"));
        assert!(rows[0].effort.is_none() && rows[0].phase.is_none() && !rows[0].schema);
        // A wholly pin-less template projects to None (field omitted on wire).
        assert!(manifest_step_pins(&WorkflowManifest::from_def(&linear_def())).is_none());
    }

    /// Every pin `StepPins` can carry has a column on the projected row. This
    /// is the census that was missing when `effort` shipped: the projection
    /// read `s.model` directly, so a second executable pin could be stamped,
    /// consumed by the dispatcher, and reported by no face at all — and no
    /// test could red on it, because no test knew the field vocabulary.
    /// `StepPins::all_fields()` is derived by exhaustive destructuring, so a
    /// new pin lands here as a NAMED failure until the row (and this match)
    /// learns it.
    #[test]
    fn every_step_pin_has_a_projection_column() {
        let mut m = WorkflowManifest::from_def(&linear_def());
        m.steps[0].model = Some("opus".into());
        m.steps[0].effort = Some("max".into());
        m.steps[0].phase = Some("Scan".into());
        m.steps[0].schema = Some(serde_json::json!({"type": "object"}));
        let rows = manifest_step_pins(&m).expect("pinned");
        let row = &rows[0];
        for field in crate::workflow::StepPins::all_fields() {
            let projected = match field {
                "model" => row.model.is_some(),
                "effort" => row.effort.is_some(),
                "phase" => row.phase.is_some(),
                "schema" => row.schema,
                other => panic!(
                    "StepPins grew a pin `{other}` with no projection column — \
                     add it to WorkflowStepPin (and step_row) so it has a face"
                ),
            };
            assert!(
                projected,
                "pin `{field}` set on the manifest but not projected"
            );
        }
    }

    #[test]
    fn an_effort_the_runtime_cannot_apply_is_reported_as_not_applied() {
        // `run` validates only the lean def, so a template on disk can carry an
        // effort the think-level vocabulary does not know. The dispatcher then
        // applies NOTHING (`workflow_effort_think_level` → None) while this
        // projection echoed the word back verbatim — three faces telling the
        // model a step is pinned to `turbo` when nothing is pinned. A wrong
        // label reads as fact where a missing one reads as "no value".
        let mut m = WorkflowManifest::from_def(&linear_def());
        m.steps[0].effort = Some("turbo".into());
        m.steps[1].effort = Some("high".into());
        let rows = manifest_step_pins(&m).expect("pinned");
        let by_step = |id: &str| {
            rows.iter()
                .find(|r| r.step == id)
                .and_then(|r| r.effort.clone())
                .expect("effort reported")
        };
        assert_eq!(by_step("write"), "high", "a recognised effort reads as-is");
        assert_eq!(
            by_step("gather"),
            "turbo (unrecognised — not applied)",
            "an effort the dispatcher discards must say so"
        );

        // `status`'s row is the third face of the same value and answers the
        // same way — it reads the stamp the compiler wrote.
        let mut task = phase_task("Scan", CoordTaskStatus::Pending);
        task.metadata = crate::agents::swarm::tasks::merge_metadata_patch(
            &task.metadata,
            serde_json::json!({ crate::workflow::WORKFLOW_EFFORT_KEY: "turbo" }),
        );
        assert_eq!(
            step_row(&task, false).effort.as_deref(),
            Some("turbo (unrecognised — not applied)")
        );
    }

    #[test]
    fn manifest_step_pins_skips_blank_model() {
        // A whitespace-only model string is not a real override — it must not
        // surface as a pinned model (mirrors the run-time override parser, which
        // trims and treats empty as "no override").
        let mut m = WorkflowManifest::from_def(&linear_def());
        m.steps[0].model = Some("   ".into());
        assert!(manifest_step_pins(&m).is_none(), "blank model is no model");
    }

    #[tokio::test]
    async fn describe_surfaces_pinned_per_step_models() {
        // `describe` returns the lean WorkflowDef (no model field), so the
        // executable per-step model would be invisible without the `models`
        // projection. Save a model-pinned template and assert describe surfaces it.
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let described = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            workflow::store::save(&manifest_with_pinned_model()).expect("save pinned template");
            let described = t
                .call(WorkflowArgs::Describe {
                    name: "pipeline".into(),
                })
                .await
                .expect("describe");
            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            described
        };

        let pins = described
            .pins
            .as_ref()
            .expect("describe surfaces pinned steps");
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].step, "gather");
        assert_eq!(pins[0].model.as_deref(), Some("opus"));
        // The lean definition still comes back (executable core) alongside it.
        assert!(described.definition.is_some());
    }

    #[tokio::test]
    async fn status_row_carries_per_step_model_from_metadata() {
        // After a run, each step's backing task carries WORKFLOW_MODEL_KEY for
        // the pinned model; `status` must echo exactly that, so an operator sees
        // which model a step runs on without re-reading the template.
        let store = setup_store().await;
        let t = tool(store, None);
        let mut pins = std::collections::HashMap::new();
        pins.insert(
            "gather".to_string(),
            crate::workflow::StepPins {
                model: Some("opus".into()),
                ..Default::default()
            },
        );
        let mat = workflow::materialize(
            &linear_def(),
            &RunInputs::from_input("x"),
            "team-9",
            t.coord_store.as_ref(),
            None,
            Some(&pins),
            None,
            None,
        )
        .await
        .expect("materialise with a per-step model");

        let out = t
            .call(WorkflowArgs::Status {
                name: "pipeline".into(),
                team_id: "team-9".into(),
                run_id: Some(mat.run_id),
                include_output: false,
            })
            .await
            .expect("status");
        let steps = out.steps.as_ref().expect("status populates steps");
        assert_eq!(steps[0].step, "gather");
        assert_eq!(
            steps[0].model.as_deref(),
            Some("opus"),
            "pinned step reports its model"
        );
        assert_eq!(steps[1].step, "write");
        assert!(steps[1].model.is_none(), "unpinned step has no model");
    }

    #[tokio::test]
    async fn status_include_output_returns_bounded_completed_results() {
        // The collection face: a fan-out's results are unreadable without it —
        // `error` only populates on Failed, and team_status dumps the whole
        // team unbounded. Off by default (a poll is a poll).
        let store = setup_store().await;
        let t = tool(store, None);
        let (run_id, ids) = materialize_run(&t, &linear_def(), "team-out").await;
        t.coord_store
            .update_task(
                &ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    result: Some("the finding ".repeat(200)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // The still-pending step carries a retry NOTICE in `result` — which is
        // what `schedule/failure.rs` actually writes when it re-queues a failed
        // attempt. Without this the fixture proved nothing: the guard was green
        // because the pending row's `result` happened to be NULL, not because
        // the code ever looked at the status.
        t.coord_store
            .update_task(
                &ids[1],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Pending),
                    result: Some("retry 1/3 in 8s after: Timed out after 900 seconds".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let plain = t
            .call(WorkflowArgs::Status {
                name: "pipeline".into(),
                team_id: "team-out".into(),
                run_id: Some(run_id.clone()),
                include_output: false,
            })
            .await
            .expect("status");
        let rows = plain.steps.as_ref().unwrap();
        assert!(
            rows.iter().all(|r| r.output.is_none()),
            "off by default — a poll stays cheap"
        );

        let with_out = t
            .call(WorkflowArgs::Status {
                name: "pipeline".into(),
                team_id: "team-out".into(),
                run_id: Some(run_id),
                include_output: true,
            })
            .await
            .expect("status with output");
        let rows = with_out.steps.as_ref().unwrap();
        let done = rows.iter().find(|r| r.status == "completed").unwrap();
        let out = done.output.as_ref().expect("completed step echoes output");
        assert!(
            out.chars().count() <= MAX_STEP_OUTPUT_CHARS + 1,
            "bounded (ellipsis-terminated): {} chars",
            out.chars().count()
        );
        // The still-pending step has no output even when asked — even though
        // it HAS a `result`. A retry notice is not a deliverable, and handing
        // it back under `output` (documented to the model as "the step's
        // recorded output") folds a dispatcher diagnostic into the synthesis.
        let pending = rows
            .iter()
            .find(|r| r.status == "pending")
            .expect("the retry-scheduled step is still pending");
        assert!(
            pending.output.is_none(),
            "a retry notice is not this step's output: {:?}",
            pending.output
        );
        assert!(rows
            .iter()
            .filter(|r| r.status != "completed")
            .all(|r| r.output.is_none()));
    }

    #[test]
    fn only_result_bearing_statuses_echo_output() {
        // Which statuses may speak through `output`, asserted per status rather
        // than through a fixture that happens to leave `result` NULL. Every
        // status here carries the SAME non-empty result string, so the only
        // thing that can separate the arms is the status itself.
        let row = |status| {
            let mut task = phase_task("Scan", status);
            task.result = Some("a string that is not necessarily a deliverable".into());
            step_row(&task, true)
        };
        for produces in [CoordTaskStatus::Completed, CoordTaskStatus::WaitingReview] {
            assert!(
                row(produces).output.is_some(),
                "{produces:?} holds a real deliverable"
            );
        }
        for silent in [
            CoordTaskStatus::Pending,
            CoordTaskStatus::Blocked,
            CoordTaskStatus::InProgress,
            CoordTaskStatus::Paused,
            CoordTaskStatus::Failed,
            CoordTaskStatus::Cancelled,
            CoordTaskStatus::Unsatisfiable,
            CoordTaskStatus::Skipped,
        ] {
            assert!(
                row(silent).output.is_none(),
                "{silent:?} writes `result` too, and it is not an output"
            );
        }
    }

    #[tokio::test]
    async fn status_groups_steps_by_phase_when_pinned() {
        // A phased template reports `phases: Scan 1/1 ...` in the message and a
        // `phase` column per row — the `.workflow.js` live-view shape. A
        // phase-less run's message stays byte-identical (no suffix).
        let store = setup_store().await;
        let t = tool(store, None);
        let mut pins = std::collections::HashMap::new();
        pins.insert(
            "gather".to_string(),
            crate::workflow::StepPins {
                phase: Some("Scan".into()),
                ..Default::default()
            },
        );
        pins.insert(
            "write".to_string(),
            crate::workflow::StepPins {
                phase: Some("Write".into()),
                ..Default::default()
            },
        );
        let mat = workflow::materialize(
            &linear_def(),
            &RunInputs::from_input("x"),
            "team-ph",
            t.coord_store.as_ref(),
            None,
            Some(&pins),
            None,
            None,
        )
        .await
        .unwrap();
        t.coord_store
            .update_task(
                &mat.task_ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    result: Some("done".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let out = t
            .call(WorkflowArgs::Status {
                name: "pipeline".into(),
                team_id: "team-ph".into(),
                run_id: Some(mat.run_id),
                include_output: false,
            })
            .await
            .expect("status");
        assert!(
            out.message.contains("phases: Scan 1/1 ✓ · Write 0/1 ▶"),
            "grouped phase summary: {}",
            out.message
        );
        let rows = out.steps.as_ref().unwrap();
        assert_eq!(rows[0].phase.as_deref(), Some("Scan"));
        assert_eq!(rows[1].phase.as_deref(), Some("Write"));
    }

    /// A bare task carrying only a phase stamp and a status — the two inputs
    /// `summarize_phases` reads. Shared so the marker tests cannot drift apart
    /// on how a task is built.
    fn phase_task(phase: &str, status: CoordTaskStatus) -> CoordTask {
        CoordTask {
            id: uuid::Uuid::new_v4().to_string(),
            team_id: Some("t".into()),
            subject: "s".into(),
            description: String::new(),
            status,
            owner: None,
            priority: crate::agents::swarm::tasks::Priority::Normal,
            result: None,
            metadata: serde_json::json!({
                crate::workflow::WORKFLOW_PHASE_KEY: phase,
            }),
            dependencies: vec![],
            created_at: 0,
            started_at: None,
            completed_at: None,
            locked_by: None,
            locked_at: None,
        }
    }

    #[test]
    fn summarize_phases_marks_failures_most_alarming() {
        // The failure marker wins over the done marker within a phase: a
        // settled-but-failed step must not let the phase report success, and
        // cancelled/unsatisfiable are "stopped", not "succeeded".
        let tasks = vec![
            phase_task("Scan", CoordTaskStatus::Completed),
            phase_task("Scan", CoordTaskStatus::Failed),
            phase_task("Fix", CoordTaskStatus::Completed),
        ];
        let line = summarize_phases(&tasks).expect("phased");
        assert!(line.contains("Scan 1/2 ✗"), "{line}");
        assert!(line.contains("Fix 1/1 ✓"), "{line}");
        // No phases → None (message byte-identical to the legacy one).
        assert!(summarize_phases(&[phase_task("", CoordTaskStatus::Completed)]).is_none());
    }

    #[test]
    fn a_skipped_step_settles_its_phase_instead_of_pinning_it_at_running() {
        // `Skipped` is settled, not failed, and produced nothing. Inferring
        // "finished" from `done == total` renders it as `0/1 ▶` — a phase that
        // has stopped, displayed as still running, forever. The marker reads
        // `CoordTaskStatus::is_settled`, whose doc names it the honest
        // completion predicate for a workflow run.
        let one = |status| vec![phase_task("Scan", status)];
        assert_eq!(
            summarize_phases(&one(CoordTaskStatus::Skipped)).as_deref(),
            Some("Scan 0/1 ✓ (1 skipped)"),
            "settled, nothing failed, and the skip is spelled out"
        );
        // Still-running statuses keep the running marker.
        for live in [
            CoordTaskStatus::Pending,
            CoordTaskStatus::Blocked,
            CoordTaskStatus::InProgress,
            CoordTaskStatus::WaitingReview,
            CoordTaskStatus::Paused,
        ] {
            let line = summarize_phases(&one(live)).expect("phased");
            assert!(line.ends_with('▶'), "{live:?} is not settled: {line}");
        }
        // Every settled-and-bad status is the alarming marker.
        for bad in [
            CoordTaskStatus::Failed,
            CoordTaskStatus::Cancelled,
            CoordTaskStatus::Unsatisfiable,
        ] {
            let line = summarize_phases(&one(bad)).expect("phased");
            assert!(line.ends_with('✗'), "{bad:?} must not read as done: {line}");
        }
    }

    #[test]
    fn a_phase_still_running_is_not_marked_stopped_by_one_failure() {
        // `✗` in this table means "stopped badly". A phase with three steps
        // still executing has not stopped, so it must not wear the stopped
        // marker — but the failure must not vanish either, which is why the
        // count is spelled out beside the running marker.
        let mut tasks = vec![phase_task("Analyze", CoordTaskStatus::Failed)];
        for _ in 0..3 {
            tasks.push(phase_task("Analyze", CoordTaskStatus::InProgress));
        }
        assert_eq!(
            summarize_phases(&tasks).as_deref(),
            Some("Analyze 0/4 ▶ (1 failed)"),
            "still running, and the failure is still visible"
        );

        // Once the last step settles, the phase HAS stopped badly — and the
        // count is not repeated, because `✗` already says it.
        let settled = vec![
            phase_task("Analyze", CoordTaskStatus::Failed),
            phase_task("Analyze", CoordTaskStatus::Completed),
        ];
        assert_eq!(summarize_phases(&settled).as_deref(), Some("Analyze 1/2 ✗"));
    }

    /// `linear_def()` with named-var placeholders in both step prompts.
    fn var_def() -> WorkflowDef {
        let mut d = linear_def();
        d.name = "varflow".into();
        d.steps[0].prompt = "research {{topic}} in {{region}}".into();
        d.steps[1].prompt = "write a report about {{topic}}".into();
        d
    }

    #[tokio::test]
    async fn run_substitutes_named_args_into_the_materialised_prompts() {
        let store = setup_store().await;
        let t = tool(store, None);
        let mut args = std::collections::HashMap::new();
        args.insert("topic".to_string(), "sea ice".to_string());
        args.insert("region".to_string(), "the Arctic".to_string());
        let mat = workflow::materialize(
            &var_def(),
            &crate::workflow::RunInputs {
                input: String::new(),
                args,
            },
            "team-vars",
            t.coord_store.as_ref(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("materialise");
        let first = t
            .coord_store
            .get_task(&mat.task_ids[0])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            first.description, "research sea ice in the Arctic",
            "both placeholders substituted, in the task the agent actually reads"
        );
    }

    #[tokio::test]
    async fn run_refuses_a_launch_missing_a_referenced_var() {
        // Fail closed: an unsupplied `{{region}}` would reach the agent as the
        // literal text `{{region}}`, i.e. a question rendered as an
        // instruction. The refusal names the missing vars.
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);
        let mut args = std::collections::HashMap::new();
        args.insert("topic".to_string(), "sea ice".to_string());

        let (refused, accepted) = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            workflow::store::save(&WorkflowManifest::from_def(&var_def())).expect("save");
            let refused = t
                .call(WorkflowArgs::Run {
                    name: "varflow".into(),
                    team_id: "team-vars".into(),
                    input: String::new(),
                    args: args.clone(),
                })
                .await;
            args.insert("region".to_string(), "the Arctic".to_string());
            let accepted = t
                .call(WorkflowArgs::Run {
                    name: "varflow".into(),
                    team_id: "team-vars".into(),
                    input: String::new(),
                    args,
                })
                .await;
            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            (refused, accepted)
        };
        let err = refused
            .expect_err("a missing var must refuse the run")
            .to_string();
        assert!(err.contains("region"), "names the missing var: {err}");
        assert!(
            !err.contains("topic"),
            "does not name a supplied one: {err}"
        );
        assert!(accepted.is_ok(), "complete args launch: {accepted:?}");
    }

    #[tokio::test]
    async fn describe_reports_the_vars_a_run_will_require() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let (varry, plain) = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            workflow::store::save(&WorkflowManifest::from_def(&var_def())).expect("save");
            workflow::store::save(&WorkflowManifest::from_def(&linear_def())).expect("save");
            let varry = t
                .call(WorkflowArgs::Describe {
                    name: "varflow".into(),
                })
                .await
                .expect("describe");
            let plain = t
                .call(WorkflowArgs::Describe {
                    name: "pipeline".into(),
                })
                .await
                .expect("describe");
            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            (varry, plain)
        };
        assert_eq!(
            varry.vars.as_deref(),
            Some(["region".to_string(), "topic".to_string()].as_slice()),
            "derived from the prompts, sorted"
        );
        assert!(varry.message.contains("requires args: region, topic"));
        // A template with no named vars is byte-identical to before: no field,
        // no suffix on the message.
        assert!(plain.vars.is_none(), "no vars => field omitted");
        assert_eq!(plain.message, "workflow 'pipeline' has 2 step(s)");
    }

    #[tokio::test]
    async fn runs_lists_every_run_newest_first() {
        let store = setup_store().await;
        let t = tool(store, None);
        let (older, older_ids) = materialize_run(&t, &linear_def(), "team-runs").await;
        let (newer, _) = materialize_run(&t, &linear_def(), "team-runs").await;
        // Settle the older run so the two rows differ on more than identity.
        for id in &older_ids {
            t.coord_store
                .update_task(
                    id,
                    crate::agents::swarm::tasks::CoordTaskUpdate {
                        status: Some(CoordTaskStatus::Completed),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }

        let out = t
            .call(WorkflowArgs::Runs {
                name: "pipeline".into(),
                team_id: "team-runs".into(),
            })
            .await
            .expect("runs");
        let rows = out.runs.as_ref().expect("one row per run");
        assert_eq!(rows.len(), 2, "both runs listed: {rows:?}");
        let ids: Vec<&str> = rows.iter().map(|r| r.run_id.as_str()).collect();
        assert!(ids.contains(&older.as_str()) && ids.contains(&newer.as_str()));
        let old_row = rows.iter().find(|r| r.run_id == older).unwrap();
        let new_row = rows.iter().find(|r| r.run_id == newer).unwrap();
        assert_eq!(old_row.steps, 2);
        assert!(old_row.settled, "every step completed: {old_row:?}");
        assert!(!new_row.settled, "untouched run is still going");
        assert!(old_row.summary.contains("2 completed"), "{old_row:?}");
        assert!(out.message.contains("1 still running"), "{}", out.message);
    }

    /// End to end through the tool: a tolerant step keeps running after its
    /// upstream fails, and the two reporting faces must not miscount it.
    /// `status` tallies whatever the store derives (so the tolerant step reads
    /// `pending`, not `unsatisfiable`), and `rerun_failed` — whose target set
    /// is `Failed | Unsatisfiable` — must re-arm only the step that actually
    /// failed, never the dependent that is on its way to running.
    #[tokio::test]
    async fn a_tolerant_step_survives_its_failed_upstream_end_to_end() {
        let store = setup_store().await;
        let t = tool(store, None);
        let mut def = linear_def();
        // gather → write, plus a tolerant synthesis that also waits on gather.
        let mut synth = step("synth", "writer", &["gather"]);
        synth.tolerate_failed_deps = true;
        def.steps.push(synth);
        let (run_id, ids) = materialize_run(&t, &def, "team-tolerant").await;

        t.coord_store
            .update_task(
                &ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Failed),
                    result: Some("boom".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let status = t
            .call(WorkflowArgs::Status {
                name: "pipeline".into(),
                team_id: "team-tolerant".into(),
                run_id: Some(run_id.clone()),
                include_output: false,
            })
            .await
            .expect("status");
        let rows = status.steps.as_ref().expect("rows");
        let of = |step_id: &str| {
            rows.iter()
                .find(|r| r.step == step_id)
                .unwrap_or_else(|| panic!("row for {step_id}"))
                .status
                .clone()
        };
        assert_eq!(of("gather"), "failed");
        assert_eq!(
            of("write"),
            "unsatisfiable",
            "an ordinary dependent is still structurally dead"
        );
        assert_eq!(
            of("synth"),
            "pending",
            "the tolerant dependent is ready to run: {rows:?}"
        );
        assert!(
            status.message.contains("1 unsatisfiable") && status.message.contains("1 pending"),
            "the tally counts what the rows say: {}",
            status.message
        );

        let rerun = t
            .call(WorkflowArgs::RerunFailed {
                name: "pipeline".into(),
                team_id: "team-tolerant".into(),
                run_id: Some(run_id),
            })
            .await
            .expect("rerun_failed");
        let rearmed = rerun.task_ids.as_ref().expect("ids");
        assert!(
            rearmed.contains(&ids[0]) && rearmed.contains(&ids[1]),
            "the failed step and its dead dependent are re-armed: {rearmed:?}"
        );
        assert!(
            !rearmed.contains(&ids[2]),
            "the tolerant step was never dead, so it is not re-armed: {rearmed:?}"
        );
    }
    #[tokio::test]
    async fn rerun_failed_rearms_only_the_failed_and_unsatisfiable_steps() {
        // Three steps: one Completed (keeps its result), one Failed, one left
        // Unsatisfiable by it. Both of the latter go back to pending with a
        // cleared result; the completed one is untouched.
        let store = setup_store().await;
        let t = tool(store, None);
        let mut def = linear_def();
        def.steps.push(crate::workflow::WorkflowStepDef {
            id: "polish".into(),
            agent: "writer".into(),
            prompt: "polish it".into(),
            depends_on: vec!["write".into()],
            kind: crate::workflow::WorkflowStepKind::Agent,
            choices: vec![],
            review: false,
            require_grounding: false,
            tolerate_failed_deps: false,
            timeout_seconds: None,
            max_retries: None,
        });
        let (run_id, ids) = materialize_run(&t, &def, "team-rerun").await;
        let set = |id: &str, status, result: &str| {
            let store = t.coord_store.clone();
            let id = id.to_string();
            let result = result.to_string();
            async move {
                store
                    .update_task(
                        &id,
                        crate::agents::swarm::tasks::CoordTaskUpdate {
                            status: Some(status),
                            result: Some(result),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
            }
        };
        set(&ids[0], CoordTaskStatus::Completed, "the finding").await;
        set(&ids[1], CoordTaskStatus::Failed, "boom").await;
        // `polish` depends on the failed `write`, so it reads Unsatisfiable.
        let polish = t.coord_store.get_task(&ids[2]).await.unwrap().unwrap();
        assert_eq!(polish.status, CoordTaskStatus::Unsatisfiable);

        let out = t
            .call(WorkflowArgs::RerunFailed {
                name: "pipeline".into(),
                team_id: "team-rerun".into(),
                run_id: Some(run_id.clone()),
            })
            .await
            .expect("rerun_failed");
        let rearmed = out.task_ids.as_ref().expect("ids");
        assert_eq!(
            rearmed.len(),
            2,
            "exactly the failed + unsatisfiable: {rearmed:?}"
        );
        assert!(rearmed.contains(&ids[1]) && rearmed.contains(&ids[2]));
        // `polish` was selected even though re-arming `write` (visited first)
        // had already turned it from unsatisfiable into merely blocked. The set
        // is decided from ONE snapshot; re-reading it per step would make the
        // answer depend on the order the DAG happens to be walked.
        let dependent = t.coord_store.get_task(&ids[2]).await.unwrap().unwrap();
        assert_eq!(dependent.status, CoordTaskStatus::Blocked);

        let done = t.coord_store.get_task(&ids[0]).await.unwrap().unwrap();
        assert_eq!(done.status, CoordTaskStatus::Completed);
        assert_eq!(
            done.result.as_deref(),
            Some("the finding"),
            "a completed step keeps its result and is not re-run"
        );
        let failed = t.coord_store.get_task(&ids[1]).await.unwrap().unwrap();
        assert_eq!(failed.status, CoordTaskStatus::Pending);
        assert_eq!(failed.result.as_deref().unwrap_or_default(), "");
        assert!(
            crate::agents::swarm::tasks::retry::read_retry_budget_reset_at(&failed.metadata)
                .is_some(),
            "the retry ladder is re-armed, or the step dies on its first new failure"
        );

        // Nothing failed any more → an honest message, not an error.
        let again = t
            .call(WorkflowArgs::RerunFailed {
                name: "pipeline".into(),
                team_id: "team-rerun".into(),
                run_id: Some(run_id),
            })
            .await
            .expect("a run with nothing to rerun is not an error");
        assert!(again.task_ids.as_ref().is_some_and(Vec::is_empty));
        assert!(
            again.message.contains("nothing to rerun"),
            "{}",
            again.message
        );
    }

    #[tokio::test]
    async fn list_carries_selection_fields_and_names_problems() {
        // `list` answers "which should I run?" in one call: description,
        // whenToUse and step count per row, and a corrupt file is NAMED in
        // `problems` + the message instead of silently vanishing.
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let out = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            let mut m = crate::workflow::WorkflowManifest::from_def(&linear_def());
            m.when_to_use = "for research reports".into();
            workflow::store::save(&m).expect("save");
            std::fs::create_dir_all(tmp.path().join("workflows")).unwrap();
            std::fs::write(tmp.path().join("workflows").join("bad.json"), "{ nope").unwrap();
            let out = t.call(WorkflowArgs::List {}).await.expect("list");
            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            out
        };

        let rows = out.workflows.as_ref().unwrap();
        let good = rows.iter().find(|r| r.name == "pipeline").unwrap();
        assert_eq!(good.when_to_use, "for research reports");
        assert_eq!(good.steps, 2);
        assert_eq!(good.description, "research then write");
        assert!(
            rows.iter().any(|r| r.name == "bad"),
            "corrupt row still addressable"
        );
        let problems = out.problems.as_ref().expect("problems named");
        assert!(problems[0].contains("bad.json"), "{problems:?}");
        assert!(out.message.contains("unreadable"), "{}", out.message);
    }

    /// import → save → run → status, in one process.
    ///
    /// Each half of this chain had its own passing tests while the middle was
    /// severed: the importer recovered `phase()` markers, `export` re-rendered
    /// them, the store persisted them — and no runtime face read one, so a
    /// phased `.workflow.js` reported a flat step list forever. A test that
    /// stops at "the manifest has phases" cannot see that; only running the
    /// whole chain in one process can.
    #[tokio::test]
    async fn a_phased_workflow_js_keeps_its_phases_all_the_way_to_status() {
        let src = r#"
export const meta = {
  name: 'audit',
  description: 'scan then fix',
}

phase('Scan')
const found = await agent('scan the repo', { label: 'scan' })

phase('Fix')
await agent('fix what scan found', { label: 'fix' })
"#;
        let store = setup_store().await;
        let t = tool(store, None);
        let tmp = TempDir::new().unwrap();

        let run_out = {
            let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("ALEPH_HOME");
            // SAFETY: guarded single mutator; restored below.
            unsafe {
                std::env::set_var("ALEPH_HOME", tmp.path());
            }
            // save=true FIRST, so the rich manifest is on disk; the retarget
            // below then goes through `save`'s preservation path. Parse-only
            // import + save would drop the phases — which is exactly what the
            // import message now warns about.
            let imported = t
                .call(WorkflowArgs::Import {
                    source: src.to_string(),
                    save: true,
                })
                .await
                .expect("import");
            // The importer names an agent per step; retarget them onto one
            // member so the run is materialisable, then save through the tool.
            let mut def = imported.definition.expect("import yields a definition");
            for step in &mut def.steps {
                step.agent = "worker".into();
            }
            let saved = t
                .call(WorkflowArgs::Save { definition: def })
                .await
                .expect("save");
            for extra in ["phases", "phase", "label"] {
                assert!(
                    saved.message.contains(extra),
                    "save names every extra it carried across (missing `{extra}`): {}",
                    saved.message
                );
            }
            let run_out = t
                .call(WorkflowArgs::Run {
                    name: "audit".into(),
                    team_id: "team-e2e".into(),
                    input: String::new(),
                    args: std::collections::HashMap::new(),
                })
                .await
                .expect("run");
            // SAFETY: same guarded invariant; restore prior value.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("ALEPH_HOME", v),
                    None => std::env::remove_var("ALEPH_HOME"),
                }
            }
            run_out
        };

        let pins = run_out
            .pins
            .as_ref()
            .expect("run echoes the pins it applied");
        assert!(
            pins.iter().any(|p| p.phase.as_deref() == Some("Scan")),
            "the imported phase survived to the launched run: {pins:?}"
        );

        let status = t
            .call(WorkflowArgs::Status {
                name: "audit".into(),
                team_id: "team-e2e".into(),
                run_id: run_out.run_id.clone(),
                include_output: false,
            })
            .await
            .expect("status");
        assert!(
            status.message.contains("phases: Scan 0/1 ▶"),
            "status groups by the imported phases: {}",
            status.message
        );
        let rows = status.steps.as_ref().unwrap();
        assert!(rows.iter().any(|r| r.phase.as_deref() == Some("Fix")));
    }

    /// `save` must not read "the stored file is there but will not parse" as
    /// "there is nothing to preserve". `store::load` errors on BOTH a missing
    /// file and an unparseable one; collapsing them with `.ok()` routed a
    /// corrupt-but-rich template through `from_def`, which deletes every
    /// `model` / `effort` / `schema` / `phase` / `whenToUse` it still held —
    /// with a success message byte-identical to a first-ever save (criterion 8
    /// on a destructive overwrite path). Same three-way discipline
    /// `loop_graph_manage`'s workflow arm already applies to this very store.
    #[tokio::test]
    async fn save_refuses_to_overwrite_an_unreadable_stored_manifest() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }
        // A stored template that carries a real per-step pin and one typo'd
        // key: `deny_unknown_fields` refuses it, so the pin is unreachable but
        // NOT gone.
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).expect("workflows dir");
        let corrupt = r#"{"name":"pipeline","description":"d","steps":[{"id":"gather","agent":"researcher","prompt":"p","model":"opus","maxRetires":3}]}"#;
        std::fs::write(dir.join("pipeline.json"), corrupt).expect("seed corrupt template");

        let refused = t
            .call(WorkflowArgs::Save {
                definition: linear_def(),
            })
            .await;
        // A name with nothing on disk is unaffected — absent still means
        // "author it fresh".
        let mut fresh = linear_def();
        fresh.name = "fresh".into();
        let saved_fresh = t.call(WorkflowArgs::Save { definition: fresh }).await;
        let on_disk = std::fs::read_to_string(dir.join("pipeline.json")).expect("file still there");
        // SAFETY: same guarded invariant; restore prior value.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }

        let err = refused
            .expect_err("saving over a file that will not parse must refuse, not overwrite")
            .to_string();
        assert!(
            err.contains("maxRetires") || err.contains("unknown field"),
            "the refusal carries the parse error so the file can be repaired: {err}"
        );
        assert!(
            err.contains("delete") && err.contains("import"),
            "the refusal names the two ways out: {err}"
        );
        assert_eq!(
            on_disk, corrupt,
            "the unreadable file is left byte-identical"
        );
        saved_fresh.expect("an absent name still saves");
    }

    /// A listing face answers "none"; it does not error. `runs` inherited
    /// `run_groups`' "no runs found" error, which made "this workflow has never
    /// run on this team" indistinguishable from a real failure — while the
    /// faces that genuinely need a run (`status` / `cancel` / …) still do.
    #[tokio::test]
    async fn runs_with_no_runs_lists_nothing_instead_of_erroring() {
        let store = setup_store().await;
        let t = tool(store, None);

        let out = t
            .call(WorkflowArgs::Runs {
                name: "pipeline".into(),
                team_id: "team-empty".into(),
            })
            .await
            .expect("a listing face answers 'none' honestly");
        assert!(
            out.runs.as_ref().expect("runs field present").is_empty(),
            "empty list, not an absent field: {out:?}"
        );
        assert!(
            out.message.contains("no runs of 'pipeline'") && out.message.contains("team-empty"),
            "{}",
            out.message
        );

        // The run-needing faces are unchanged: they cannot answer without one.
        let status = t
            .call(WorkflowArgs::Status {
                name: "pipeline".into(),
                team_id: "team-empty".into(),
                run_id: None,
                include_output: false,
            })
            .await;
        assert!(status.is_err(), "status needs a run to report on");
    }

    /// The rendered file discloses partial fan-in in a `//` comment; the tool's
    /// MESSAGE is the other face of that same fact, and the caller who exports
    /// through the tool reads the message, not the file (criterion 9 — one
    /// verb, several faces, one derivation).
    #[tokio::test]
    async fn export_message_discloses_partial_fan_in() {
        let tmp = TempDir::new().unwrap();
        let store = setup_store().await;
        let t = tool(store, None);

        let _lock = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: guarded single mutator; restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", tmp.path());
        }
        // a and b are independent; c waits for a ONLY — the body's
        // `parallel([a, b]) / c` skeleton cannot say that.
        let mut partial = linear_def();
        partial.name = "partial".into();
        partial.steps = vec![
            step("a", "researcher", &[]),
            step("b", "researcher", &[]),
            step("c", "writer", &["a"]),
        ];
        let saved_partial = workflow::store::save(&WorkflowManifest::from_def(&partial));
        let exported = t
            .call(WorkflowArgs::Export {
                name: "partial".into(),
                write_file: false,
            })
            .await;
        // A complete-bipartite (here: linear) DAG says nothing extra.
        let saved_linear = workflow::store::save(&WorkflowManifest::from_def(&linear_def()));
        let clean = t
            .call(WorkflowArgs::Export {
                name: "pipeline".into(),
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
        saved_partial.expect("seed partial");
        saved_linear.expect("seed linear");

        let msg = exported.expect("export").message;
        assert!(
            msg.contains("partial fan-in") && msg.contains("c depends on: a"),
            "the export message names the lossy steps: {msg}"
        );
        assert!(
            msg.contains("@aleph-workflow"),
            "and says which header keeps the re-import lossless: {msg}"
        );
        let clean_msg = clean.expect("export").message;
        assert!(
            !clean_msg.contains("partial fan-in"),
            "a losslessly-expressible DAG gets no note: {clean_msg}"
        );
    }
}
