//! Scratchpad Tool — Agent working memory management
//!
//! Allows the AI to manage agent scratchpad files stored at
//! `~/.aleph/workspaces/<agent_id>/scratchpad.md`.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::builtin_tools::scratchpad_registry;
use crate::clarification::ClarificationResult;
use crate::error::Result;
use crate::gateway::i18n::{t_ui, Msg};
use crate::memory::scratchpad::{PlanItem, PlanItemStatus, ScratchpadManager, ScratchpadSnapshot};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// What action to perform on the scratchpad
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScratchpadAction {
    /// Initialize a new scratchpad (or read existing)
    Initialize,
    /// Read current scratchpad content
    Read,
    /// Update the objective
    SetObjective,
    /// Set plan items (replaces existing plan)
    SetPlan,
    /// Mark a plan item as the in-progress current step (by 0-based index)
    StartItem,
    /// Mark a plan item as complete (by 0-based index)
    CompleteItem,
    /// Append a note to the Notes section
    AppendNote,
    /// Show the current plan to the human and wait for their verdict
    RequestApproval,
    /// Clear and reset the scratchpad
    Clear,
}

impl std::fmt::Display for ScratchpadAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initialize => write!(f, "initialize"),
            Self::Read => write!(f, "read"),
            Self::SetObjective => write!(f, "set_objective"),
            Self::SetPlan => write!(f, "set_plan"),
            Self::StartItem => write!(f, "start_item"),
            Self::CompleteItem => write!(f, "complete_item"),
            Self::AppendNote => write!(f, "append_note"),
            Self::RequestApproval => write!(f, "request_approval"),
            Self::Clear => write!(f, "clear"),
        }
    }
}

/// Wire status, re-exported from the protocol crate so the tool result, the
/// `run_complete` summary and the Panel all speak one shape.
pub use aleph_protocol::plan::PlanItemStatus as PlanItemStatusDto;

/// Schema-only mirror of [`PlanItemStatusDto`].
///
/// `schemars` cannot derive `JsonSchema` for a type owned by another crate,
/// and the protocol crate has no business depending on `schemars` for a tool
/// argument shape. This exists **only** to describe the `status` field of a
/// `set_plan` argument to the model; the wire/output side uses the protocol
/// type directly. Kept honest by `status_dto_and_arg_schema_agree`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatusArg {
    Pending,
    InProgress,
    Completed,
}

impl From<PlanItemStatus> for PlanItemStatusDto {
    fn from(s: PlanItemStatus) -> Self {
        match s {
            PlanItemStatus::Pending => Self::Pending,
            PlanItemStatus::InProgress => Self::InProgress,
            PlanItemStatus::Done => Self::Completed,
        }
    }
}

impl From<PlanItemStatusArg> for PlanItemStatus {
    fn from(s: PlanItemStatusArg) -> Self {
        match s {
            PlanItemStatusArg::Pending => Self::Pending,
            PlanItemStatusArg::InProgress => Self::InProgress,
            PlanItemStatusArg::Completed => Self::Done,
        }
    }
}

/// One entry of a `set_plan` call.
///
/// Two accepted shapes, so the model can write the whole checklist —
/// statuses included — in a single call (codex `update_plan` /
/// kimi `SetTodoList` / hermes `todo` parity):
///
/// * `"Design the API"` — bare text. Status is **inherited** from the item of
///   the same text in the current plan when one exists, else `pending`. That
///   inheritance is what makes "add one more step" idempotent: the old
///   text-only signature forced every item back to `[ ]`, wiping the run's
///   progress and then getting the stop vetoed for work already done.
/// * `{"text": "Design the API", "status": "completed"}` — explicit status,
///   always honored verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PlanItemArg {
    /// Bare step text; status inherited from the current plan, else pending.
    Text(String),
    /// Step text with an explicit lifecycle status.
    Detailed {
        /// Step text.
        text: String,
        /// `pending` / `in_progress` / `completed`. Omitted behaves like the
        /// bare-text form.
        #[serde(default)]
        status: Option<PlanItemStatusArg>,
    },
}

impl PlanItemArg {
    fn text(&self) -> &str {
        match self {
            Self::Text(t) => t,
            Self::Detailed { text, .. } => text,
        }
    }

    fn explicit_status(&self) -> Option<PlanItemStatus> {
        match self {
            Self::Text(_) => None,
            Self::Detailed { status, .. } => status.map(Into::into),
        }
    }
}

/// Resolve incoming `set_plan` args against the plan already on disk.
///
/// Explicit statuses win; a bare-text item inherits the status of the item
/// with the same (trimmed) text in `current`, else starts `pending`.
///
/// **Each existing item may be claimed at most once.** Repeated step texts are
/// ordinary in a long run ("Run tests" appearing three times), and a plain
/// `find` handed every one of them the *first* match's status. Re-sending
/// `["Run tests" [x], "Fix bug" [ ], "Run tests" [ ]]` as bare text therefore
/// promoted the third — never executed — step to `[x]`. That direction does not
/// self-correct: it satisfies `is_objective_complete`, so `ScratchpadGoalVerifier`
/// stops guarding and the run reports done with work outstanding. (The reverse
/// failure — a re-worded step falling back to `pending` — is self-correcting:
/// the model sees the demoted glyph in the very next echo.) Claiming positionally
/// makes N same-text items map one-to-one in order.
fn resolve_plan_items(args: &[PlanItemArg], current: &[PlanItem]) -> Vec<PlanItem> {
    let mut claimed = vec![false; current.len()];
    args.iter()
        .map(|arg| {
            let text = arg.text().trim().to_string();
            let status = arg.explicit_status().unwrap_or_else(|| {
                current
                    .iter()
                    .enumerate()
                    .find(|(i, existing)| !claimed[*i] && existing.text == text)
                    .map_or(PlanItemStatus::Pending, |(i, existing)| {
                        claimed[i] = true;
                        existing.status
                    })
            });
            PlanItem { text, status }
        })
        .collect()
}

pub use aleph_protocol::plan::PlanItem as PlanItemDto;

/// Structured snapshot of the scratchpad plan, attached to `ScratchpadOutput`
/// so the Panel can render a live Todo widget (rides the existing
/// `tool_call_completed` event; no new protocol variant — R4/R10) **and**
/// carried on `RunSummary` so the Panel can reconcile a checklist whose live
/// frames the lossy trace mirror dropped.
pub use aleph_protocol::plan::PlanSnapshot as PlanSnapshotDto;

/// Project the on-disk scratchpad into the wire shape.
///
/// `From<Local> for Foreign` is legal here (the parameter type is local) and
/// keeps the conversion next to the thing being converted.
pub fn plan_snapshot_dto(s: &ScratchpadSnapshot) -> PlanSnapshotDto {
    PlanSnapshotDto {
        objective: s.objective.clone(),
        items: s
            .items
            .iter()
            .map(|i| PlanItemDto {
                text: i.text.clone(),
                status: i.status.into(),
            })
            .collect(),
        complete: s.is_objective_complete(),
    }
}

/// Resolve the execution list a session is currently writing to.
///
/// **The single "session → plan" resolution point.** Three surfaces need this
/// answer and they must not each derive it: the prompt layer
/// (`harness_bridge::context_blocks::active_execution_plan`), the stop guard
/// (`ScratchpadGoalVerifier`, which additionally needs the raw snapshot for its
/// veto text), and `chat.history`, which hands the durable list to a client
/// that just attached. Registry → manager → parse, once.
///
/// `None` when the session never bound a scratchpad, the file is gone, or it
/// cannot be read. Fail-soft on I/O for the same reason the verifier is: a
/// transient read error must never wedge prompt assembly or a history fetch.
pub async fn session_plan(session_key: &str) -> Option<ScratchpadSnapshot> {
    let project_id = scratchpad_registry::active(session_key)?;
    // The real session key, not a literal: `ScratchpadManager` stamps it into
    // the file's `_Session:` line on any write, and a reader that passes a
    // stand-in is one refactor away from becoming a writer that stamps it.
    let manager = ScratchpadManager::new(&project_id, session_key);
    if !manager.exists() {
        return None;
    }
    manager.snapshot().await.ok()
}

/// [`session_plan`] projected onto the wire shape, for renderers.
///
/// Returns `None` for an inert list (no objective *and* no items) so a caller
/// that hydrates a widget from this never blanks one it did not produce —
/// the same rule the tool's own read-shaped actions follow.
pub async fn session_plan_snapshot(session_key: &str) -> Option<PlanSnapshotDto> {
    let snap = session_plan(session_key).await?;
    if snap.objective.is_none() && snap.items.is_empty() {
        return None;
    }
    Some(plan_snapshot_dto(&snap))
}

/// Arguments for the scratchpad tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScratchpadArgs {
    /// Project identifier (AI-assigned name). Optional — when omitted, the
    /// current chat session derives a default scratchpad, so single-chat
    /// todos work without naming a project. Pass an explicit id for a durable
    /// cross-session project.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Action to perform
    pub action: ScratchpadAction,
    /// Value for Initialize (objective), `SetObjective`, `AppendNote`.
    /// On `SetPlan` it optionally sets the objective in the same call — do
    /// that on the first `set_plan`, because a plan with no objective is not
    /// surfaced to you next turn and does not hold the session open.
    pub value: Option<String>,
    /// Plan items for `SetPlan` — replaces the whole list. Each entry is
    /// either a step string (status inherited from the current plan, else
    /// pending) or `{"text": "...", "status": "pending|in_progress|completed"}`.
    pub items: Option<Vec<PlanItemArg>>,
    /// Item index for `StartItem` / `CompleteItem` (0-based)
    pub item_index: Option<usize>,
}

/// Output from the scratchpad tool
#[derive(Debug, Clone, Serialize)]
pub struct ScratchpadOutput {
    /// Whether the operation succeeded
    pub success: bool,
    /// Human-readable result message
    pub message: String,
    /// The raw scratchpad markdown — `read` / `initialize` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// The updated checklist, echoed after a **mutating** action.
    ///
    /// Its own field rather than sharing `content` with the raw markdown
    /// above. Two different things in one field forced every consumer to
    /// recover the difference from somewhere else, and the progress sink did
    /// it with a hand-kept list of action names (`PROGRESS_ACTIONS`) — a
    /// whitelist that only describes the actions that existed the day it was
    /// written, so a ninth action would silently stop reaching the user's
    /// channel with nothing failing. Now the shape answers it: this field is
    /// present exactly when there is progress to surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    /// Structured plan snapshot for the Panel Todo widget (mutating actions only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<PlanSnapshotDto>,
    /// `request_approval` only: what the human decided —
    /// `approved` / `revise` / `rejected` / `timeout` / `cancelled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// `request_approval` only: what they said when they did not simply
    /// approve. This is the revision to act on, not a courtesy note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

/// Verdicts offered on a plan-approval card, as `(wire value, label key)`.
///
/// Free text is a fourth outcome the menu deliberately does not list: anything
/// that is not one of these three IS the revision, which is how pi's
/// "Refine the plan" reads without making the human pick twice.
///
/// The **value** is an English key and stays one: `verdict_of` reads it and the
/// model is told it received it, so a display setting must not be able to
/// change what the verdict *is*. Only the label a person reads is translated.
const APPROVAL_CHOICES: [(&str, Msg<'static>); 3] = [
    (DECISION_APPROVED, Msg::PlanApproveLabel),
    (DECISION_REVISE, Msg::PlanReviseLabel),
    ("rejected", Msg::PlanRejectLabel),
];

/// The one verdict that ENDS plan mode.
///
/// A `const` rather than three `"approved"` literals because it is now the
/// trigger for a real state change (the plan → build handoff), not just a word
/// in a message: the menu that offers it, the branch that reads it, and the
/// release that acts on it have to be the same string by construction.
const DECISION_APPROVED: &str = "approved";

/// Decision reported when the human answered with something not on the menu.
const DECISION_REVISE: &str = "revise";

/// Map a resolved clarification onto `(decision, feedback)`.
///
/// Anything the human typed that is not one of the three listed verdicts IS the
/// revision. Making them pick "Revise" and *then* type it would be two
/// interactions for one thought — and the free-text answer is already the more
/// informative of the two (pi's "Refine the plan" without the extra step).
fn verdict_of(result: &ClarificationResult) -> (String, Option<String>) {
    use crate::clarification::ClarificationResultType;
    match result.result_type {
        ClarificationResultType::Answered => match result.answers.first() {
            Some(answer) if answer.is_custom() => (
                DECISION_REVISE.to_string(),
                Some(answer.value.clone()).filter(|v| !v.trim().is_empty()),
            ),
            Some(answer) => (answer.value.clone(), None),
            // `Answered` with no answers cannot be built by
            // `ClarificationResult::answered`, but reporting a verdict we did
            // not receive is the one outcome worth refusing outright.
            None => ("cancelled".to_string(), None),
        },
        ClarificationResultType::Timeout => ("timeout".to_string(), None),
        ClarificationResultType::Cancelled => ("cancelled".to_string(), None),
    }
}

/// The model-facing sentence for a verdict.
///
/// The timeout wording is the load-bearing one: silence is the outcome most
/// easily misread as consent, and this is an advisory gate — nothing downstream
/// stops the model — so the only thing standing between "nobody answered" and
/// "nobody objected" is what this string says.
fn approval_message(decision: &str, feedback: Option<&str>, handoff: Option<&str>) -> String {
    match decision {
        // `handoff` is `Some` only when this call actually lifted a read-only
        // planning gate. On a conversation that was never planning the
        // sentence stays exactly what it has always been — approval remains
        // the advisory checkpoint it was designed as.
        DECISION_APPROVED => handoff.map_or_else(
            || "Plan approved — start working the list.".to_string(),
            |h| format!("Plan approved — start working the list. {h}"),
        ),
        DECISION_REVISE => feedback.map_or_else(
            || "Plan needs revision (no detail given) — ask what to change.".to_string(),
            |f| format!("Plan needs revision: {f}"),
        ),
        "rejected" => "Plan rejected — do not execute it.".to_string(),
        "timeout" => "Nobody answered in time. The plan is UNREVIEWED — do not treat silence as \
                      approval; do the reversible part, or stop and report."
            .to_string(),
        _ => "The approval request ended without a verdict; the plan is unreviewed.".to_string(),
    }
}

/// Render the plan for a human to read before approving it.
///
/// Reads the **persisted** snapshot rather than taking prose from the caller:
/// the whole value of a plan gate over a plain `ask_user` is that what the
/// human approves is what is actually on disk and in the model's
/// `<execution_plan>` — a paraphrase the model retypes into a question is a
/// second representation of the plan, free to flatter it.
fn render_plan_for_approval(snapshot: &ScratchpadSnapshot, note: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(objective) = snapshot.objective.as_deref() {
        let lead = t_ui(Msg::PlanObjectiveLead);
        out.push_str(&format!("**{lead}:** {objective}\n\n"));
    }
    for (i, item) in snapshot.items.iter().enumerate() {
        let mark = match item.status {
            PlanItemStatus::Done => "x",
            PlanItemStatus::InProgress => "~",
            PlanItemStatus::Pending => " ",
        };
        out.push_str(&format!("{}. [{mark}] {}\n", i + 1, item.text));
    }
    if let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) {
        out.push_str(&format!("\n{note}\n"));
    }
    out.push('\n');
    out.push_str(&t_ui(Msg::PlanApprovePrompt));
    out
}

/// Tool that allows the AI to manage project scratchpads
#[derive(Clone, Default)]
pub struct ScratchpadTool {
    /// Live session-key handle (shared with the execution engine, which
    /// writes the active session's key before every tool call). Used to
    /// bind the touched `project_id` to the session in
    /// [`scratchpad_registry`] so the goal-loop hook can find this
    /// execution list at stop time. `None` → registry binding is skipped
    /// (scratchpad still works; the hook simply stays dormant).
    session_key: Option<Arc<RwLock<String>>>,
    /// Handles for [`ScratchpadAction::RequestApproval`]. `None` → the action
    /// reports that no human gate is wired rather than pretending to ask,
    /// which is the same shape as the headless refusal one layer down.
    clarification: Option<crate::clarification::ClarificationDeps>,
    /// Session store, for the ONE write this tool makes outside its own
    /// markdown: clearing the `exec_tier` override when a human approves the
    /// plan and the conversation stops planning. `None` → the handoff still
    /// lifts the gate for the current turn and says that it could not be
    /// persisted (see [`Self::release_plan_gate`]).
    session_store: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
}

impl ScratchpadTool {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            session_key: None,
            clarification: None,
            session_store: None,
        }
    }

    /// Attach the session store used by the plan → build handoff.
    ///
    /// Wired from the same source as `session_set_mode`'s store (gateway
    /// context, else the session manager) — the two write the same carrier
    /// (`identity_meta.custom`), so they must not end up reading different
    /// backends.
    #[must_use]
    pub fn with_session_store(
        mut self,
        store: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
    ) -> Self {
        self.session_store = store;
        self
    }

    /// Attach the human-gate handles used by `request_approval`.
    ///
    /// Same pair `ask_user` holds, and deliberately the same
    /// [`crate::clarification::ask`] path: a plan gate that grew its own
    /// delivery ladder would be a second answer to "how do we reach the human",
    /// and the second answer is always the one that misses the next transport.
    #[must_use]
    pub fn with_clarification(
        mut self,
        clarification: Arc<crate::clarification::ClarificationManager>,
        channels: Arc<crate::gateway::channel_registry::ChannelRegistry>,
    ) -> Self {
        self.clarification = Some(crate::clarification::ClarificationDeps::new(
            clarification,
            channels,
        ));
        self
    }

    /// Attach the shared live session-key handle. Pass the same handle the
    /// execution engine writes (see `execution_engine::execute`).
    #[must_use]
    pub fn with_session_key_handle(mut self, handle: Option<Arc<RwLock<String>>>) -> Self {
        self.session_key = handle;
        self
    }

    /// Current live session key, or empty string when no handle is wired.
    /// Prefers the per-run `TURN_CONTEXT` task-local — the shared handle is
    /// process-global and rewritten at every run start, so a concurrent run
    /// of another agent can overwrite it mid-turn and the registry would
    /// bind the project to the wrong session.
    async fn current_session_key(&self) -> String {
        if let Some(sk) = crate::tools::turn_context::current_session_key() {
            return sk;
        }
        match &self.session_key {
            Some(h) => h.read().await.clone(),
            None => String::new(),
        }
    }
}

/// Panel-facing DTO only, with no model-facing echo — for the read-shaped
/// actions whose `content` is already the raw markdown. Fail-soft to `None`;
/// an empty plan yields `None` so a read never blanks a widget it did not
/// produce.
async fn plan_snapshot(manager: &ScratchpadManager) -> Option<PlanSnapshotDto> {
    let snap = manager.snapshot().await.ok()?;
    if snap.objective.is_none() && snap.items.is_empty() {
        return None;
    }
    Some(plan_snapshot_dto(&snap))
}

/// Read the scratchpad once and produce BOTH the model-facing progress echo
/// and the Panel-facing structured DTO, so the two never drift. Fail-soft:
/// `(None, None)` on any read error rather than failing the op.
///
/// When the action just finished the objective (every box `[x]`), the echo
/// becomes a wrap-up completion summary instead of the in-progress checklist —
/// closing the goal-loop with hermes-agent `mark_done` parity. The summary is
/// structural (the model's own checkboxes), so the model stays sovereign over
/// completion (R7); the progress sink mirrors it to the user channel (R5).
///
/// Unbounded by design — this is a tool result, one message the model asked
/// for, backstopped by the generic tool-output budget. The prompt-resident copy
/// is the one that has to be capped; see `render_progress_bounded`.
async fn progress_parts(manager: &ScratchpadManager) -> (Option<String>, Option<PlanSnapshotDto>) {
    match manager.snapshot().await {
        Ok(s) => {
            let text = if s.is_objective_complete() {
                s.render_completion()
            } else {
                s.render_progress()
            };
            (Some(text), Some(plan_snapshot_dto(&s)))
        }
        Err(_) => (None, None),
    }
}

#[async_trait]
impl AlephTool for ScratchpadTool {
    const NAME: &'static str = "scratchpad";
    const DESCRIPTION: &'static str =
        "Manage your working memory (scratchpad) for a multi-step task: set an \
         objective, lay out a plan as an execution list, then work the list one \
         step at a time. Use it for work with 3+ non-trivial steps; skip it for \
         a single-step answer. \
         set_plan replaces the whole list: send every step each time. A step \
         resent with the SAME text keeps the status it already had, so inserting \
         or reordering steps mid-run does not reset your progress. Re-wording a \
         step makes it a new step (it starts pending), so when you change a \
         step's wording send {text, status} to carry its status over. Exactly \
         one step may be in_progress. \
         action='start_item' / 'complete_item' (0-based item_index) move a \
         single step and echo the updated list back to you. \
         action='request_approval' shows the saved plan to the user and waits; \
         verdict in `decision`/`feedback`. Use before expensive or \
         irreversible work. The scratchpad persists across sessions. While an objective is set and plan items \
         remain unfinished, the goal-loop keeps this session running so you work \
         through them step by step — call action='clear' once the objective is \
         fully achieved.";

    type Args = ScratchpadArgs;
    type Output = ScratchpadOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Resolve the effective project id: explicit, else derive from the
        // live chat session so single-chat todos need no project name.
        let session_key = self.current_session_key().await;
        let (project_id, explicit) = match args.project_id.clone() {
            Some(p) if !p.trim().is_empty() => (p, true),
            _ => (derive_default_project_id(&session_key), false),
        };

        info!(
            project_id = %project_id,
            action = %args.action,
            "Scratchpad operation requested"
        );

        // Validate project_id to prevent path traversal (applies to explicit ids;
        // derived ids are pre-sanitized and always pass).
        if project_id.contains("..")
            || project_id.contains('/')
            || project_id.contains('\\')
            || project_id.contains('\0')
            || project_id.starts_with('.')
        {
            return Err(crate::error::AlephError::tool(
                "Invalid project_id: must not contain path separators, '..', null bytes, or start with '.'".to_string(),
            ));
        }

        // Multi-user namespacing (round-5 ⑤, product decision — previously
        // the answer was given by accident): an EXPLICIT project_id is
        // model-chosen and used to resolve against one flat, install-global
        // directory, so any principal could name — and read — another's
        // named scratchpad. Explicit ids are now namespaced by the asking
        // actor: the owner (and caller-less legacy paths) keep the flat path
        // byte-identically, every other principal gets `<id>__<actor>`, the
        // memory-partition suffix shape. Derived ids need no suffix: they
        // come from the session key, which already separates personal
        // sessions, and deliberately shares a room's — matching the room's
        // shared memory partition.
        let project_id = if explicit {
            namespace_explicit_project_id(&project_id)
        } else {
            project_id
        };

        // Registry binding (unchanged semantics, now keyed on resolved id).
        // BT-D-R4-12: previously this updated the registry BEFORE the file
        // operation ran. A failed write (disk full, permission denied,
        // concurrent removal) would leave the registry pointing at a
        // project whose scratchpad does not exist on disk, and the next
        // Read or Plan lookup would silently resolve to a missing file.
        // Capture the intended binding here and only commit it after the
        // action match returns Ok.
        let mut pending_registry_bind: Option<(String, String)> = None;
        if !session_key.is_empty() {
            match args.action {
                ScratchpadAction::Read => {}
                ScratchpadAction::Clear => scratchpad_registry::clear(&session_key),
                _ => pending_registry_bind = Some((session_key.clone(), project_id.clone())),
            }
        }

        // The `_Session:` line the manager stamps into the file is the plan's
        // only record of who owns it. Passing a literal made every plan on
        // disk claim the same fictional owner ("tool"), so a plan file could
        // not name the conversation it belongs to. The live session key is
        // already resolved above — hand it the real one.
        let manager = ScratchpadManager::new(&project_id, &session_key);

        // The action match below may return early via `?`. A guard type
        // would be more idiomatic, but a one-call `pending_registry_bind`
        // commit at the successful exit keeps the diff local and avoids
        // a new struct.
        let result = match args.action {
            ScratchpadAction::Initialize => {
                let existed = manager.exists();
                if !existed {
                    manager.initialize(args.value.as_deref()).await?;
                }
                let content = manager.read().await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: if existed {
                        "Scratchpad already exists, returning current content".to_string()
                    } else {
                        "Scratchpad initialized".to_string()
                    },
                    content: Some(content),
                    progress: None,
                    // Read-shaped actions carry the snapshot too, so the Panel
                    // Todo widget rehydrates on reconnect / a fresh session
                    // attaching to an existing project — it used to stay hidden
                    // until the next mutating call.
                    snapshot: plan_snapshot(&manager).await,
                    decision: None,
                    feedback: None,
                })
            }

            ScratchpadAction::Read => {
                if !manager.exists() {
                    return Ok(ScratchpadOutput {
                        success: true,
                        message: "No scratchpad exists for this project".to_string(),
                        content: None,
                        progress: None,
                        snapshot: None,
                        decision: None,
                        feedback: None,
                    });
                }
                let content = manager.read().await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: "Scratchpad content loaded".to_string(),
                    content: Some(content),
                    progress: None,
                    snapshot: plan_snapshot(&manager).await,
                    decision: None,
                    feedback: None,
                })
            }

            ScratchpadAction::SetObjective => {
                let value = args.value.unwrap_or_default();
                manager.set_objective(&value).await?;
                let (progress, snapshot) = progress_parts(&manager).await;
                Ok(ScratchpadOutput {
                    success: true,
                    message: format!("Objective updated: {value}"),
                    content: None,
                    progress,
                    snapshot,
                    decision: None,
                    feedback: None,
                })
            }

            ScratchpadAction::SetPlan => {
                let incoming = args.items.unwrap_or_default();
                let current = manager.snapshot().await.unwrap_or_default();
                let items = resolve_plan_items(&incoming, &current.items);
                let objective = args
                    .value
                    .as_deref()
                    .map(str::trim)
                    .filter(|v| !v.is_empty());
                let written = manager.set_plan(objective, &items).await?;
                let (progress, snapshot) = progress_parts(&manager).await;
                let inert = objective.is_none() && current.objective.is_none() && written > 0;
                let message = if inert {
                    format!(
                        "Plan set with {written} item(s), but no objective is set — so this plan \
                         is NOT re-surfaced to you next turn and does NOT hold the session open. \
                         Pass `value` (the objective) on set_plan, or call action='set_objective'."
                    )
                } else {
                    format!("Plan set with {written} item(s)")
                };
                Ok(ScratchpadOutput {
                    success: true,
                    message,
                    content: None,
                    progress,
                    snapshot,
                    decision: None,
                    feedback: None,
                })
            }

            ScratchpadAction::StartItem => {
                let index = require_item_index(args.item_index, "start_item")?;
                manager.start_item(index).await?;
                let (progress, snapshot) = progress_parts(&manager).await;
                Ok(ScratchpadOutput {
                    success: true,
                    message: format!("Item {index} marked in progress (current step)"),
                    content: None,
                    progress,
                    snapshot,
                    decision: None,
                    feedback: None,
                })
            }

            ScratchpadAction::CompleteItem => {
                let index = require_item_index(args.item_index, "complete_item")?;
                manager.complete_item(index).await?;
                let (progress, snapshot) = progress_parts(&manager).await;
                Ok(ScratchpadOutput {
                    success: true,
                    message: format!("Item {index} marked as complete"),
                    content: None,
                    progress,
                    snapshot,
                    decision: None,
                    feedback: None,
                })
            }

            ScratchpadAction::AppendNote => {
                let note = args.value.unwrap_or_default();
                manager.append_note(&note).await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: "Note appended".to_string(),
                    content: None,
                    progress: None,
                    snapshot: None,
                    decision: None,
                    feedback: None,
                })
            }

            ScratchpadAction::RequestApproval => {
                self.request_approval(&manager, args.value.as_deref()).await
            }

            ScratchpadAction::Clear => {
                manager.clear().await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: "Scratchpad cleared".to_string(),
                    content: None,
                    progress: None,
                    snapshot: None,
                    decision: None,
                    feedback: None,
                })
            }
        };
        // BT-D-R4-12: commit the deferred registry binding now that the
        // file operation completed successfully. If we got here, the
        // scratchpad file exists (or was just created) for `project_id`,
        // so the registry can safely point at it. Any error in the action
        // match above propagated via `?` before reaching this line, so the
        // binding is only ever written on a verified-Ok path.
        if let Some((session, project)) = pending_registry_bind {
            scratchpad_registry::set_active(&session, &project);
        }
        result
    }
}

impl ScratchpadTool {
    /// Show the persisted plan to the human and wait for their verdict.
    ///
    /// The human gate the exec tier cannot be: `[exec] tier` asks about *this
    /// call*, one tool invocation at a time, and by the time it fires the plan
    /// is already being executed. This asks about the plan **as a whole**,
    /// before the first step — the axis codex covers with plan mode and pi with
    /// its post-plan `select("Execute / Stay / Refine")`.
    ///
    /// On a conversation that is not planning it is advisory by construction:
    /// nothing in `src/harness/` learns about the verdict and no tool is
    /// blocked by it — the model asked, the model was answered, and the model
    /// decides what that means (R7/R10).
    ///
    /// On a conversation at [`ExecTier::Plan`] it is also the HANDOFF: an
    /// `approved` lifts the read-only planning gate for the rest of this turn
    /// and clears the session's tier override for every turn after it (see
    /// [`Self::release_plan_gate`]). That still needs no plan-state machine in
    /// the loop — the loop dispatches exactly what it always did, and the one
    /// enforcement chokepoint reads one `AtomicBool`. The doc here used to
    /// argue the opposite ("a gate that enforced itself would need a
    /// plan-state machine inside the loop — that is cognition"). What was
    /// actually true is narrower: the DECISION is cognition and stays with the
    /// human, while flipping a tier the resolver already computed is
    /// bookkeeping.
    ///
    /// # Why the model cannot let itself out
    ///
    /// Everything below the `ask` call refuses without a person:
    /// `request_approval` errors when no approval transport is wired, and
    /// [`crate::clarification::ask`] errors on an unattended run and on any
    /// turn with no channel to deliver to. There is no branch that reaches an
    /// `approved` verdict without one having been chosen off a menu.
    ///
    /// [`ExecTier::Plan`]: crate::config::types::policies::ExecTier::Plan
    async fn request_approval(
        &self,
        manager: &ScratchpadManager,
        note: Option<&str>,
    ) -> Result<ScratchpadOutput> {
        use crate::clarification::{
            ClarificationOption, ClarificationQuestion, ClarificationRequest,
        };

        let Some(ref deps) = self.clarification else {
            return Err(crate::error::AlephError::tool(
                "scratchpad: no human-approval gate is wired on this server — proceed and say \
                 that the plan was not reviewed",
            ));
        };

        let snapshot = manager.snapshot().await?;
        if snapshot.items.is_empty() {
            return Err(crate::error::AlephError::tool(
                "scratchpad: there is no plan to approve — call action='set_plan' first",
            ));
        }

        let options: Vec<ClarificationOption> = APPROVAL_CHOICES
            .iter()
            .map(|(value, label)| ClarificationOption::new(value, &t_ui(*label)))
            .collect();
        let request = ClarificationRequest::new(vec![ClarificationQuestion::select(
            "plan_approval",
            &render_plan_for_approval(&snapshot, note),
            options,
        )
        .with_header("Plan")])
        .map_err(crate::error::AlephError::tool)?;

        // `withheld_secret` is structurally empty here: the plan gate asks one
        // question and never marks it a secret, so there is nothing the
        // transport rule can hold back. Written out in full rather than with
        // `..` so that a new field on `AskOutcome` stops the compiler here and
        // this site gets a decision, instead of inheriting a silent default.
        let crate::clarification::AskOutcome {
            result,
            withheld_secret: _,
        } = crate::clarification::ask(deps, request)
            .await
            .map_err(|e| crate::error::AlephError::tool(format!("scratchpad: {e}")))?;

        let (decision, feedback) = verdict_of(&result);
        // The plan → build handoff. Runs BEFORE the message is built so the
        // model is told, in the same breath as the verdict, that it may now
        // act — and runs only for a human `approved` on a turn that actually
        // has a read-only gate to lift.
        let handoff = self.release_plan_gate(&decision).await;
        let message = approval_message(&decision, feedback.as_deref(), handoff.as_deref());

        Ok(ScratchpadOutput {
            success: true,
            message,
            content: None,
            progress: None,
            // The reviewed plan itself, so the Panel's todo widget shows
            // exactly what the verdict was about.
            snapshot: Some(plan_snapshot_dto(&snapshot)),
            decision: Some(decision),
            feedback,
        })
    }

    /// Turn a human `approved` into the plan → build handoff.
    ///
    /// Returns the sentence to append to the verdict when this call actually
    /// ended plan mode, `None` otherwise — including on a conversation that
    /// was never planning, where approval keeps being the advisory checkpoint
    /// it has always been.
    ///
    /// ## Why this is not "the model releasing its own gate"
    ///
    /// The only caller is the arm below a resolved
    /// [`crate::clarification::ask`], which refuses outright on an unattended
    /// run and on any turn with no channel to reach a person
    /// (`ask.rs::HEADLESS_DENIAL`), and `request_approval` refuses earlier
    /// still when no approval transport is wired at all. So reaching this
    /// function means a human saw the persisted plan and picked "approve" off
    /// a menu. Nothing the model can say to itself gets here.
    ///
    /// ## Two writes, one decision
    ///
    /// The in-memory gate governs the REST OF THIS TURN (the next tool call
    /// runs at the restored tier — that is the whole point of a handoff), and
    /// the session write governs every later turn and every attached client.
    /// They are done here, together, because 判据 §0 has already collected the
    /// bill for terminal side effects that live on only one of an action's
    /// arms.
    ///
    /// Order is deliberate: the gate opens FIRST. A store that is down must
    /// not veto a decision the human already made; it downgrades the handoff
    /// to "this turn only", and the message says so rather than leaving the
    /// model to discover it next turn.
    async fn release_plan_gate(&self, decision: &str) -> Option<String> {
        if decision != DECISION_APPROVED {
            return None;
        }
        let gate = crate::tools::turn_context::current_plan_gate()?;
        // `release` is one-shot: a model that asks for approval twice in one
        // turn gets one handoff sentence and one session write.
        if !gate.release() {
            return None;
        }
        let restore = gate.restore_to();
        let persisted = self.clear_session_exec_tier().await;
        info!(
            restore_tier = restore.id(),
            persisted, "Plan approved — read-only planning gate released"
        );
        Some(if persisted {
            format!(
                "Planning is over: the read-only gate is lifted for this conversation \
                 (execution tier `{}`), effective immediately — your next tool call \
                 already runs under it. Work the checklist and keep it ticked off.",
                restore.id()
            )
        } else {
            format!(
                "Planning is over for THIS TURN: the read-only gate is lifted (execution \
                 tier `{}`) but the choice could not be written to the session, so a \
                 later turn may start planning again. Get as far as you can now, and \
                 tell the user they may need to leave plan mode from the composer.",
                restore.id()
            )
        })
    }

    /// Clear this session's `exec_tier` override, returning `true` on success.
    ///
    /// Clearing rather than pinning the restored tier: the gate's
    /// `restore_to` was itself derived with the session's `plan` taken out of
    /// the running, so "no override" resolves to the same value on the next
    /// turn — and keeps following `[policies] exec_tier` if the operator moves
    /// it later, which a pin would silently stop doing. `null` is the
    /// established clear-an-override convention on this carrier
    /// (`sessions.patch`'s `first_invalid_knob` documents it).
    ///
    /// The write also emits `session.updated`, which is how the Panel's tier
    /// pill learns that the conversation stopped planning: it re-reads the
    /// session list on that frame and adopts the dials it reports. No
    /// plan-specific client wiring — the knob-sync path that already exists
    /// carries it.
    async fn clear_session_exec_tier(&self) -> bool {
        use crate::config::types::policies::EXEC_TIER_SESSION_KEY;
        use crate::gateway::router::SessionKey as LegacySessionKey;
        use crate::gateway::session_store::types::SessionPatch;

        let Some(store) = self.session_store.as_ref() else {
            return false;
        };
        let key_str = self.current_session_key().await;
        let Some(key) = LegacySessionKey::from_key_string(&key_str) else {
            return false;
        };
        let patch = SessionPatch {
            metadata: Some(serde_json::json!({ EXEC_TIER_SESSION_KEY: serde_json::Value::Null })),
            ..Default::default()
        };
        match store.patch_session(&key, &patch).await {
            Ok(updated) => updated,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to clear session exec tier after plan approval");
                false
            }
        }
    }
}

/// `item_index` is required by the two index-addressed actions.
///
/// It used to default to `0`. An out-of-range index has been a real error
/// since §3.13 ④ — because "you completed item 7" for a 3-item plan tells the
/// model it recorded work it did not, and the run then burns `steer_max`
/// against a veto it cannot explain. A *missing* index reaches the identical
/// end state by a shorter road: it silently ticks whatever step 0 happens to
/// be, reports success, and the step the model meant stays open. Same
/// consequence, so the same answer.
fn require_item_index(index: Option<usize>, action: &str) -> Result<usize> {
    index.ok_or_else(|| {
        crate::error::AlephError::tool(format!(
            "action='{action}' needs `item_index` (0-based, from the current plan). \
             Call action='read' to see the list."
        ))
    })
}

/// Derive a filesystem-safe default scratchpad project id from the live
/// session key, for single-chat ad-hoc todos where the model omits
/// Namespace an explicit, model-chosen `project_id` by the asking principal
/// (round-5 ⑤). The owner and caller-less paths (single-user installs,
/// loopback before P1, tests) return the id unchanged — the flat legacy path
/// is byte-identical there. Any other principal gets `<id>__<actor>`, so two
/// members naming the same scratchpad land in different directories instead
/// of sharing one by accident. The actor slug is sanitized the same way
/// `derive_default_project_id` sanitizes session keys, so the namespaced id
/// still passes the path-traversal guard above.
fn namespace_explicit_project_id(project_id: &str) -> String {
    match crate::gateway::visibility::ambient_actor() {
        Some(actor)
            if actor != crate::gateway::security::store::OWNER_USER_ID && !actor.is_empty() =>
        {
            let slug: String = actor
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            format!("{project_id}__{slug}")
        }
        _ => project_id.to_string(),
    }
}

/// `project_id`. Keeps only `[A-Za-z0-9_-]`, prefixes `chat-` (so it never
/// starts with `.` and never collides with the path-traversal guard).
fn derive_default_project_id(session_key: &str) -> String {
    let slug: String = session_key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // collapse runs of '-' and trim edges for a clean slug
    let mut collapsed = String::with_capacity(slug.len());
    let mut prev_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash {
                collapsed.push('-');
            }
            prev_dash = true;
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }
    let trimmed = collapsed.trim_matches('-');
    if trimmed.is_empty() {
        "chat-default".to_string()
    } else {
        format!("chat-{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- plan approval (`action='request_approval'`) --------------------

    fn snapshot() -> ScratchpadSnapshot {
        ScratchpadSnapshot {
            objective: Some("Ship auth".into()),
            items: vec![
                PlanItem {
                    text: "Design".into(),
                    status: PlanItemStatus::Done,
                },
                PlanItem {
                    text: "Build".into(),
                    status: PlanItemStatus::InProgress,
                },
                PlanItem {
                    text: "Test".into(),
                    status: PlanItemStatus::Pending,
                },
            ],
        }
    }

    fn answer(value: &str, custom: bool) -> ClarificationResult {
        ClarificationResult::answered(vec![crate::clarification::ClarificationAnswer {
            question_id: "plan_approval".into(),
            selected_indices: if custom { vec![] } else { vec![0] },
            value: value.into(),
        }])
    }

    /// What the human reads is the plan as PERSISTED — objective, every step,
    /// and each step's real status. That is the whole reason this is an action
    /// on `scratchpad` rather than an `ask_user` the model types a plan into:
    /// a retyped plan is a second representation, free to flatter the first.
    #[test]
    fn the_card_shows_the_persisted_plan_with_its_real_statuses() {
        let rendered = render_plan_for_approval(&snapshot(), Some("  heads up  "));
        assert!(rendered.contains("**Objective:** Ship auth"), "{rendered}");
        assert!(rendered.contains("1. [x] Design"), "{rendered}");
        assert!(rendered.contains("2. [~] Build"), "{rendered}");
        assert!(rendered.contains("3. [ ] Test"), "{rendered}");
        assert!(rendered.contains("heads up"), "{rendered}");
        assert!(
            rendered.trim_end().ends_with("Approve this plan?"),
            "{rendered}"
        );
    }

    /// A blank note is not a blank line: an approval card is read at a glance,
    /// and vertical noise is what makes the plan hard to scan.
    #[test]
    fn a_blank_note_renders_identically_to_no_note() {
        assert_eq!(
            render_plan_for_approval(&snapshot(), Some("   ")),
            render_plan_for_approval(&snapshot(), None)
        );
        assert!(
            !render_plan_for_approval(&snapshot(), None).contains("\n\n\n"),
            "no run of blank lines"
        );
    }

    // ---- plan → build handoff (`release_plan_gate`) ----------------------

    /// Run `f` with a plan gate installed on the turn, as a planning run has.
    async fn with_gate<F, T>(
        restore: crate::config::types::policies::ExecTier,
        f: impl FnOnce(Arc<crate::tools::plan_gate::PlanGate>) -> F,
    ) -> T
    where
        F: std::future::Future<Output = T>,
    {
        let gate = Arc::new(crate::tools::plan_gate::PlanGate::new(restore));
        let ctx = crate::tools::turn_context::TurnContext {
            session_key: crate::routing::session_key::SessionKey::main("planner"),
            run_id: String::new(),
            channel_id: "test".to_string(),
            conversation_id: "conv".to_string(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: Some(Arc::clone(&gate)),
        };
        crate::tools::turn_context::TURN_CONTEXT
            .scope(ctx, f(gate))
            .await
    }

    /// The handoff fires on exactly one verdict. `revise` and `rejected` are
    /// the two answers a human gives when the work must NOT start, so a gate
    /// that lifted on either of them would be worse than no gate at all.
    #[tokio::test]
    async fn only_an_approval_lifts_the_planning_gate() {
        use crate::config::types::policies::ExecTier;

        for verdict in [DECISION_REVISE, "rejected", "timeout", "cancelled"] {
            with_gate(ExecTier::Auto, |gate| async move {
                let tool = ScratchpadTool::new();
                assert!(tool.release_plan_gate(verdict).await.is_none());
                assert!(
                    !gate.is_released(),
                    "`{verdict}` must leave the conversation planning"
                );
            })
            .await;
        }

        with_gate(ExecTier::Auto, |gate| async move {
            let tool = ScratchpadTool::new();
            let handoff = tool
                .release_plan_gate(DECISION_APPROVED)
                .await
                .expect("an approval hands off");
            assert!(gate.is_released());
            assert!(handoff.contains("auto"), "{handoff}");
            assert_eq!(gate.tier(), ExecTier::Auto);
        })
        .await;
    }

    /// Asking twice in one turn is one handoff. The sentence and the session
    /// write ride on the first release, and a second "approved" must not
    /// re-announce a transition that already happened.
    #[tokio::test]
    async fn the_handoff_happens_once_per_turn() {
        with_gate(
            crate::config::types::policies::ExecTier::Auto,
            |_gate| async move {
                let tool = ScratchpadTool::new();
                assert!(tool.release_plan_gate(DECISION_APPROVED).await.is_some());
                assert!(tool.release_plan_gate(DECISION_APPROVED).await.is_none());
            },
        )
        .await;
    }

    /// On a conversation that was never planning there is nothing to hand off,
    /// and approval stays the advisory checkpoint it was designed as — the
    /// message is byte-identical to what it has always been.
    #[tokio::test]
    async fn approval_without_a_plan_gate_is_the_advisory_checkpoint_it_always_was() {
        let tool = ScratchpadTool::new();
        assert!(tool.release_plan_gate(DECISION_APPROVED).await.is_none());
        assert_eq!(
            approval_message(DECISION_APPROVED, None, None),
            "Plan approved — start working the list."
        );
    }

    /// …and when there IS a handoff, the model is told in the same breath as
    /// the verdict. A transition it only finds out about by trying a tool is
    /// a transition it will not try.
    #[test]
    fn the_approval_message_carries_the_handoff_when_one_happened() {
        let msg = approval_message(DECISION_APPROVED, None, Some("Planning is over: go."));
        assert!(msg.contains("start working the list"), "{msg}");
        assert!(msg.contains("Planning is over"), "{msg}");
    }

    #[test]
    fn listed_verdicts_come_back_verbatim_with_no_feedback() {
        for (value, _) in APPROVAL_CHOICES {
            let (decision, feedback) = verdict_of(&answer(value, false));
            assert_eq!(decision, value);
            assert!(feedback.is_none(), "a picked verdict carries no revision");
        }
    }

    /// Free text is the fourth outcome the menu does not list: it IS the
    /// revision, so the human never has to pick "Revise" and then type it.
    #[test]
    fn free_text_becomes_a_revision_carrying_what_they_wrote() {
        let (decision, feedback) = verdict_of(&answer("do step 3 first", true));
        assert_eq!(decision, DECISION_REVISE);
        assert_eq!(feedback.as_deref(), Some("do step 3 first"));
        assert!(approval_message(&decision, feedback.as_deref(), None).contains("do step 3 first"));

        // Whitespace-only free text is a revision with nothing to act on, not
        // a revision whose detail is a blank string.
        let (decision, feedback) = verdict_of(&answer("   ", true));
        assert_eq!(decision, DECISION_REVISE);
        assert!(feedback.is_none());
        assert!(approval_message(&decision, None, None).contains("ask what to change"));
    }

    /// Silence is the outcome most easily misread as consent, and this gate is
    /// advisory — nothing downstream blocks on it — so the wording is the only
    /// thing between "nobody answered" and "nobody objected".
    #[test]
    fn a_timeout_says_unreviewed_not_approved() {
        let (decision, feedback) = verdict_of(&ClarificationResult::timeout());
        assert_eq!(decision, "timeout");
        assert!(feedback.is_none());
        let message = approval_message(&decision, None, None);
        assert!(message.contains("UNREVIEWED"), "{message}");
        assert!(
            message.contains("do not treat silence as approval"),
            "{message}"
        );
        assert!(!message.to_lowercase().contains("approved —"), "{message}");
    }

    #[test]
    fn a_cancelled_request_reports_no_verdict() {
        let (decision, _) = verdict_of(&ClarificationResult::cancelled());
        assert_eq!(decision, "cancelled");
        assert!(approval_message(&decision, None, None).contains("unreviewed"));
    }

    /// An `Answered` with no answers cannot be built by
    /// `ClarificationResult::answered`, but inventing a verdict is the one
    /// outcome worth refusing outright.
    #[test]
    fn an_answer_less_answered_result_is_not_read_as_approval() {
        let empty = ClarificationResult::answered(vec![]);
        assert_eq!(verdict_of(&empty).0, "cancelled");
    }

    /// Without the two handles there is no way to reach a human. Saying so is
    /// the same shape as the headless refusal one layer down — never a silent
    /// success, and never a pretend question.
    #[tokio::test]
    async fn request_approval_without_a_wired_gate_says_so() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let manager = ScratchpadManager::with_dir(tmp.path().to_path_buf(), "sess");
        let err = ScratchpadTool::new()
            .request_approval(&manager, None)
            .await
            .expect_err("no gate wired must be an error");
        assert!(err.to_string().contains("no human-approval gate"), "{err}");
    }

    /// Nothing to approve is a caller error the model can fix in one step, not
    /// an empty card for a human to stare at.
    #[tokio::test]
    async fn request_approval_refuses_when_there_is_no_plan() {
        // `with_dir`, not `new`: `new` resolves `ALEPH_HOME`, and a test that
        // sets a process-global env var is a test that fails only when the
        // suite runs in parallel.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let tool = ScratchpadTool::new().with_clarification(
            Arc::new(crate::clarification::ClarificationManager::new()),
            Arc::new(crate::gateway::channel_registry::ChannelRegistry::new()),
        );
        let manager = ScratchpadManager::with_dir(tmp.path().to_path_buf(), "sess");
        manager
            .initialize(Some("objective only"))
            .await
            .expect("initialize");
        let err = tool
            .request_approval(&manager, None)
            .await
            .expect_err("an empty plan must be refused");
        assert!(err.to_string().contains("no plan to approve"), "{err}");
    }

    #[test]
    fn plan_snapshot_dto_maps_three_states_and_completion() {
        use crate::memory::scratchpad::{PlanItem, PlanItemStatus, ScratchpadSnapshot};
        let snap = ScratchpadSnapshot {
            objective: Some("Ship auth".into()),
            items: vec![
                PlanItem {
                    text: "Design".into(),
                    status: PlanItemStatus::Done,
                },
                PlanItem {
                    text: "Build".into(),
                    status: PlanItemStatus::InProgress,
                },
                PlanItem {
                    text: "Test".into(),
                    status: PlanItemStatus::Pending,
                },
            ],
        };
        let dto = plan_snapshot_dto(&snap);
        assert_eq!(dto.objective.as_deref(), Some("Ship auth"));
        assert_eq!(dto.items.len(), 3);
        assert!(!dto.complete); // not all done
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["items"][0]["status"], "completed");
        assert_eq!(json["items"][1]["status"], "in_progress");
        assert_eq!(json["items"][2]["status"], "pending");
    }

    #[test]
    fn plan_snapshot_dto_complete_when_all_done() {
        use crate::memory::scratchpad::{PlanItem, PlanItemStatus, ScratchpadSnapshot};
        let snap = ScratchpadSnapshot {
            objective: Some("X".into()),
            items: vec![PlanItem {
                text: "a".into(),
                status: PlanItemStatus::Done,
            }],
        };
        assert!(plan_snapshot_dto(&snap).complete);
    }

    /// Round-5 ⑤: an explicit, model-chosen `project_id` is namespaced by the
    /// asking principal — two members naming the same scratchpad must not
    /// land in one shared directory by accident.
    #[tokio::test]
    async fn explicit_project_id_is_namespaced_per_non_owner_actor() {
        // No caller (single-user / legacy / tests): flat, byte-identical.
        assert_eq!(namespace_explicit_project_id("roadmap"), "roadmap");

        // The owner keeps the flat legacy path.
        let flat = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some(crate::gateway::security::store::OWNER_USER_ID.to_string()),
                async { namespace_explicit_project_id("roadmap") },
            )
            .await;
        assert_eq!(flat, "roadmap");

        // A member gets their own suffix — and two members differ.
        let bob = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                namespace_explicit_project_id("roadmap")
            })
            .await;
        let alice = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                namespace_explicit_project_id("roadmap")
            })
            .await;
        assert_eq!(bob, "roadmap__u-bob");
        assert_eq!(alice, "roadmap__u-alice");
        // The suffix must survive the ingress path-traversal guard.
        for id in [&bob, &alice] {
            assert!(
                !id.contains("..")
                    && !id.contains('/')
                    && !id.contains('\\')
                    && !id.starts_with('.'),
                "namespaced id must pass the ingress guard: {id}"
            );
        }
    }

    /// The `_Session:` line is the plan file's only record of who owns it, and
    /// every writer used to stamp the same literal — so no plan on disk could
    /// name the conversation it belonged to.
    #[tokio::test]
    async fn plan_file_records_the_owning_session_key() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tool = ScratchpadTool::new().with_session_key_handle(Some(Arc::new(RwLock::new(
            "agent:main:main:s7".to_string(),
        ))));
        let out = tool
            .call(ScratchpadArgs {
                project_id: Some("owner-probe".to_string()),
                action: ScratchpadAction::Initialize,
                value: Some("Ship auth".to_string()),
                items: None,
                item_index: None,
            })
            .await
            .unwrap();
        let content = out.content.expect("initialize returns content");
        assert!(
            content.contains("_Session: agent:main:main:s7_"),
            "plan file must name its owning session, got:\n{content}"
        );
        assert!(!content.contains("_Session: tool_"));
    }

    fn args(action: ScratchpadAction) -> ScratchpadArgs {
        ScratchpadArgs {
            project_id: Some("shape-probe".to_string()),
            action,
            value: None,
            items: None,
            item_index: None,
        }
    }

    /// The two index-addressed actions must not silently default to item 0.
    ///
    /// An out-of-range index has been a hard error since the round that found
    /// runs burning `steer_max` against a veto they could not explain — the
    /// model had been told it recorded work it never did. A *missing* index
    /// reached the same place faster: tick step 0, report success, leave the
    /// intended step open.
    #[tokio::test]
    async fn an_index_addressed_action_without_an_index_is_refused() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tool = ScratchpadTool::new();
        tool.call(ScratchpadArgs {
            value: Some("Ship".into()),
            items: Some(vec![
                PlanItemArg::Text("first".into()),
                PlanItemArg::Text("second".into()),
            ]),
            ..args(ScratchpadAction::SetPlan)
        })
        .await
        .unwrap();

        for action in [ScratchpadAction::StartItem, ScratchpadAction::CompleteItem] {
            let err = tool.call(args(action)).await.unwrap_err();
            assert!(
                err.to_string().contains("item_index"),
                "the refusal must name the missing argument: {err}"
            );
        }

        // And nothing was ticked behind the refusal.
        let snap = session_plan("").await;
        assert!(snap.is_none(), "no session bound, so no ambient plan");
        let manager = ScratchpadManager::new("shape-probe", "");
        let items = manager.snapshot().await.unwrap().items;
        assert!(
            items.iter().all(|i| i.status == PlanItemStatus::Pending),
            "a refused call must not move an item: {items:?}"
        );
    }

    /// `content` is the raw markdown, `progress` is the checklist echo. The
    /// progress sink decides what to push to the user's channel from the
    /// presence of the second one, so a mutating action that put its echo in
    /// `content` would either go unpushed or push a whole markdown file.
    #[tokio::test]
    async fn the_two_output_texts_live_in_their_own_fields() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tool = ScratchpadTool::new();

        let mutated = tool
            .call(ScratchpadArgs {
                value: Some("Ship auth".into()),
                items: Some(vec![PlanItemArg::Text("Design".into())]),
                ..args(ScratchpadAction::SetPlan)
            })
            .await
            .unwrap();
        assert!(
            mutated.content.is_none(),
            "a mutation returns no raw markdown"
        );
        let progress = mutated.progress.expect("a mutation echoes the checklist");
        assert!(progress.contains("Objective: Ship auth"));
        assert!(progress.contains("- [ ] Design"));

        let read = tool.call(args(ScratchpadAction::Read)).await.unwrap();
        assert!(
            read.progress.is_none(),
            "a read is a pull, not progress — pushing it would dump the file"
        );
        let markdown = read.content.expect("a read returns the document");
        assert!(markdown.starts_with("# Current Task"));
        // Both shapes still carry the structured snapshot the Panel reads.
        assert!(read.snapshot.is_some() && mutated.snapshot.is_some());
    }

    /// The seam `chat.history` serves: whatever the tool wrote is readable
    /// back out of the durable file by session key alone, with no live frame
    /// and no trace involved.
    #[tokio::test]
    async fn a_written_plan_is_resolvable_from_the_session_key_alone() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let session = "agent:resolver:main:s1";
        let tool = ScratchpadTool::new()
            .with_session_key_handle(Some(Arc::new(RwLock::new(session.to_string()))));

        assert!(
            session_plan_snapshot(session).await.is_none(),
            "nothing bound yet"
        );

        tool.call(ScratchpadArgs {
            project_id: None, // derive from the session, the single-chat case
            action: ScratchpadAction::SetPlan,
            value: Some("Ship auth".into()),
            items: Some(vec![
                PlanItemArg::Detailed {
                    text: "Design".into(),
                    status: Some(PlanItemStatusArg::Completed),
                },
                PlanItemArg::Text("Build".into()),
            ]),
            item_index: None,
        })
        .await
        .unwrap();

        let dto = session_plan_snapshot(session)
            .await
            .expect("the durable list is reachable by session key");
        assert_eq!(dto.objective.as_deref(), Some("Ship auth"));
        assert_eq!(dto.done_count(), 1);
        assert_eq!(dto.total(), 2);
        assert!(!dto.complete);
        crate::builtin_tools::scratchpad_registry::clear(session);
    }

    #[test]
    fn test_tool_name_and_description() {
        assert_eq!(ScratchpadTool::NAME, "scratchpad");
        assert!(ScratchpadTool::DESCRIPTION.contains("scratchpad"));
    }

    #[test]
    fn test_action_display() {
        assert_eq!(format!("{}", ScratchpadAction::Initialize), "initialize");
        assert_eq!(format!("{}", ScratchpadAction::Read), "read");
        assert_eq!(
            format!("{}", ScratchpadAction::SetObjective),
            "set_objective"
        );
        assert_eq!(format!("{}", ScratchpadAction::SetPlan), "set_plan");
        assert_eq!(format!("{}", ScratchpadAction::StartItem), "start_item");
        assert_eq!(
            format!("{}", ScratchpadAction::CompleteItem),
            "complete_item"
        );
        assert_eq!(format!("{}", ScratchpadAction::AppendNote), "append_note");
        assert_eq!(format!("{}", ScratchpadAction::Clear), "clear");
    }

    #[test]
    fn test_action_serialization() {
        assert_eq!(
            serde_json::to_string(&ScratchpadAction::Initialize).unwrap(),
            "\"initialize\""
        );
        assert_eq!(
            serde_json::to_string(&ScratchpadAction::SetPlan).unwrap(),
            "\"set_plan\""
        );
        assert_eq!(
            serde_json::to_string(&ScratchpadAction::CompleteItem).unwrap(),
            "\"complete_item\""
        );
    }

    #[test]
    fn test_args_deserialization() {
        let json = r#"{
            "project_id": "my-project",
            "action": "initialize",
            "value": "Build feature X"
        }"#;
        let args: ScratchpadArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.project_id.as_deref(), Some("my-project"));
        assert!(matches!(args.action, ScratchpadAction::Initialize));
        assert_eq!(args.value, Some("Build feature X".to_string()));
    }

    #[test]
    fn test_args_set_plan_deserialization() {
        let json = r#"{
            "project_id": "my-project",
            "action": "set_plan",
            "items": ["Step 1", "Step 2", "Step 3"]
        }"#;
        let args: ScratchpadArgs = serde_json::from_str(json).unwrap();
        assert!(matches!(args.action, ScratchpadAction::SetPlan));
        assert_eq!(args.items.unwrap().len(), 3);
    }

    #[test]
    fn status_dto_and_arg_schema_agree() {
        // `PlanItemStatusArg` exists only because schemars cannot derive for a
        // foreign type. If the two ever drift, the model would be shown an
        // enum the wire cannot express. Compare the serde forms, both ways.
        for (arg, dto) in [
            (PlanItemStatusArg::Pending, PlanItemStatusDto::Pending),
            (PlanItemStatusArg::InProgress, PlanItemStatusDto::InProgress),
            (PlanItemStatusArg::Completed, PlanItemStatusDto::Completed),
        ] {
            assert_eq!(
                serde_json::to_value(arg).unwrap(),
                serde_json::to_value(dto).unwrap()
            );
        }
        // And the schema the model reads lists exactly those three values —
        // no variant added to one side only.
        let json = serde_json::to_value(schemars::schema_for!(PlanItemStatusArg)).unwrap();
        assert_eq!(
            json["enum"],
            serde_json::json!(["pending", "in_progress", "completed"]),
            "schema drifted from the wire enum: {json}"
        );
    }

    #[test]
    fn set_plan_accepts_mixed_string_and_object_items() {
        let json = r#"{
            "action": "set_plan",
            "value": "Ship auth",
            "items": [
                {"text": "Design", "status": "completed"},
                {"text": "Build", "status": "in_progress"},
                "Test"
            ]
        }"#;
        let args: ScratchpadArgs = serde_json::from_str(json).unwrap();
        let items = args.items.unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].explicit_status(), Some(PlanItemStatus::Done));
        assert_eq!(items[1].explicit_status(), Some(PlanItemStatus::InProgress));
        assert_eq!(items[2].text(), "Test");
        assert_eq!(items[2].explicit_status(), None);
        assert_eq!(args.value.as_deref(), Some("Ship auth"));
    }

    #[test]
    fn bare_text_items_inherit_status_from_the_current_plan() {
        // The core regression: refining a plan (adding a step, re-ordering)
        // must not reset the run's progress.
        let current = vec![
            PlanItem {
                text: "Design".into(),
                status: PlanItemStatus::Done,
            },
            PlanItem {
                text: "Build".into(),
                status: PlanItemStatus::InProgress,
            },
        ];
        let incoming = vec![
            PlanItemArg::Text("Design".into()),
            PlanItemArg::Text("Build".into()),
            PlanItemArg::Text("Test".into()),
        ];
        let resolved = resolve_plan_items(&incoming, &current);
        assert_eq!(resolved[0].status, PlanItemStatus::Done);
        assert_eq!(resolved[1].status, PlanItemStatus::InProgress);
        assert_eq!(
            resolved[2].status,
            PlanItemStatus::Pending,
            "a genuinely new step starts pending"
        );
    }

    #[test]
    fn explicit_status_overrides_inheritance() {
        let current = vec![PlanItem {
            text: "Build".into(),
            status: PlanItemStatus::Done,
        }];
        let incoming = vec![PlanItemArg::Detailed {
            text: "Build".into(),
            status: Some(PlanItemStatusArg::Pending),
        }];
        // Re-opening a finished step is the model's call, not ours (R7).
        assert_eq!(
            resolve_plan_items(&incoming, &current)[0].status,
            PlanItemStatus::Pending
        );
    }

    #[test]
    fn item_text_is_trimmed_so_inheritance_matches() {
        let current = vec![PlanItem {
            text: "Build".into(),
            status: PlanItemStatus::Done,
        }];
        let incoming = vec![PlanItemArg::Text("  Build  ".into())];
        let resolved = resolve_plan_items(&incoming, &current);
        assert_eq!(resolved[0].text, "Build");
        assert_eq!(resolved[0].status, PlanItemStatus::Done);
    }

    #[test]
    fn test_args_complete_item_deserialization() {
        let json = r#"{
            "project_id": "my-project",
            "action": "complete_item",
            "item_index": 2
        }"#;
        let args: ScratchpadArgs = serde_json::from_str(json).unwrap();
        assert!(matches!(args.action, ScratchpadAction::CompleteItem));
        assert_eq!(args.item_index, Some(2));
    }

    #[test]
    fn test_tool_definition() {
        let tool = ScratchpadTool::new();
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "scratchpad");
    }

    #[test]
    fn derive_default_project_id_sanitizes_and_prefixes() {
        assert_eq!(
            derive_default_project_id("agent:abc/def 1"),
            "chat-agent-abc-def-1"
        );
        assert_eq!(derive_default_project_id(""), "chat-default");
        assert_eq!(derive_default_project_id("///"), "chat-default");
        // result must pass the same path-safety rules call() enforces
        let id = derive_default_project_id("..\\evil");
        assert!(
            !id.contains("..") && !id.contains('/') && !id.contains('\\') && !id.starts_with('.')
        );
    }

    /// N steps with the same text must inherit one-to-one, in order.
    ///
    /// A plain `find` gave every repetition the FIRST match's status, so
    /// re-sending `["Run tests" [x], "Fix bug" [ ], "Run tests" [ ]]` as bare
    /// text promoted the third — never executed — step to `[x]`. That direction
    /// does not self-correct: an all-done plan satisfies `is_objective_complete`,
    /// so `ScratchpadGoalVerifier` stops guarding and the run reports success
    /// with work outstanding. Repeated step texts are ordinary in a long run.
    #[test]
    fn duplicate_texts_inherit_positionally_not_all_from_the_first() {
        let current = vec![
            PlanItem {
                text: "Run tests".into(),
                status: PlanItemStatus::Done,
            },
            PlanItem {
                text: "Fix bug".into(),
                status: PlanItemStatus::Pending,
            },
            PlanItem {
                text: "Run tests".into(),
                status: PlanItemStatus::Pending,
            },
        ];
        let args = vec![
            PlanItemArg::Text("Run tests".into()),
            PlanItemArg::Text("Fix bug".into()),
            PlanItemArg::Text("Run tests".into()),
        ];

        let resolved = resolve_plan_items(&args, &current);
        assert_eq!(resolved[0].status, PlanItemStatus::Done);
        assert_eq!(resolved[1].status, PlanItemStatus::Pending);
        assert_eq!(
            resolved[2].status,
            PlanItemStatus::Pending,
            "a second 'Run tests' must not inherit the first one's [x] — that \
             marks a never-executed step done and silently satisfies the stop \
             guard"
        );
    }

    /// Re-wording is a new step (pending). The tool DESCRIPTION must say so —
    /// it used to promise "re-wording steps mid-run never resets your progress",
    /// actively steering the model into the one action that loses status.
    #[test]
    fn a_reworded_step_starts_pending_and_the_description_says_so() {
        let current = vec![PlanItem {
            text: "Write migration".into(),
            status: PlanItemStatus::InProgress,
        }];
        let args = vec![PlanItemArg::Text("Write migration (v2 schema)".into())];

        let resolved = resolve_plan_items(&args, &current);
        assert_eq!(resolved[0].status, PlanItemStatus::Pending);

        let desc = ScratchpadTool::DESCRIPTION;
        assert!(
            !desc.contains("re-wording steps mid-run never resets"),
            "the DESCRIPTION still promises behaviour the code does not have"
        );
        assert!(
            desc.contains("Re-wording a step makes it a new step"),
            "the DESCRIPTION must tell the model to send {{text, status}} when \
             it rewords a step"
        );
    }
}
