//! Data Transfer Objects for the LLM-facing `workflow` tool.
//!
//! These are the wire shapes — the args the model sends and the rows it
//! reads back. Kept separate from [`super::phase_tally`] (which is an
//! internal accumulator) and from the main [`crate::builtin_tools::workflow_tool`]
//! file (which holds the tool struct + dispatch), so a wire-shape change
//! touches one file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::workflow::WorkflowDef;

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
    pub(crate) fn msg(action: &str, message: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            message: message.into(),
            ..Default::default()
        }
    }
}