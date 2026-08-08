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
use crate::error::Result;
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
    /// Scratchpad content (returned for Read/Initialize)
    pub content: Option<String>,
    /// Structured plan snapshot for the Panel Todo widget (mutating actions only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<PlanSnapshotDto>,
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
}

impl ScratchpadTool {
    #[must_use]
    pub const fn new() -> Self {
        Self { session_key: None }
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

/// Read the scratchpad snapshot once and produce BOTH the model-facing
/// progress echo text and the Panel-facing structured DTO, so the two never
/// drift. Fail-soft: returns (None, None) on any read error rather than
/// failing the op.
///
/// When the action just finished the objective (every box `[x]`), the echo
/// becomes a wrap-up completion summary instead of the in-progress checklist —
/// closing the goal-loop with hermes-agent `mark_done` parity. The summary is
/// structural (the model's own checkboxes), so the model stays sovereign over
/// completion (R7); the progress sink mirrors it to the user channel (R5).
/// Panel-facing DTO only, with no model-facing echo — for read-shaped actions
/// whose `content` is already the raw markdown. Fail-soft to `None`; an empty
/// plan yields `None` so a read never blanks a widget it did not produce.
async fn plan_snapshot(manager: &ScratchpadManager) -> Option<PlanSnapshotDto> {
    let snap = manager.snapshot().await.ok()?;
    if snap.objective.is_none() && snap.items.is_empty() {
        return None;
    }
    Some(plan_snapshot_dto(&snap))
}

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
         Arm both at once with action='set_plan', value='<the objective>', \
         items=[...] — a plan with no objective is not re-surfaced to you next \
         turn and does not hold the session open. \
         set_plan replaces the whole list: send every step each time. A step \
         resent with the SAME text keeps the status it already had, so inserting \
         or reordering steps mid-run does not reset your progress. Re-wording a \
         step makes it a new step (it starts pending), so when you change a \
         step's wording send {text, status} to carry its status over. Exactly \
         one step may be in_progress. \
         action='start_item' / 'complete_item' (0-based item_index) move a \
         single step and echo the updated list back to you. The scratchpad \
         persists across sessions. While an objective is set and plan items \
         remain unfinished, the goal-loop keeps this session running so you work \
         through them step by step — call action='clear' once the objective is \
         fully achieved. The project_id is optional — omit it to use the current \
         chat's scratchpad.";

    type Args = ScratchpadArgs;
    type Output = ScratchpadOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Resolve the effective project id: explicit, else derive from the
        // live chat session so single-chat todos need no project name.
        let session_key = self.current_session_key().await;
        let project_id = match args.project_id.clone() {
            Some(p) if !p.trim().is_empty() => p,
            _ => derive_default_project_id(&session_key),
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

        // Registry binding (unchanged semantics, now keyed on resolved id).
        if !session_key.is_empty() {
            match args.action {
                ScratchpadAction::Read => {}
                ScratchpadAction::Clear => scratchpad_registry::clear(&session_key),
                _ => scratchpad_registry::set_active(&session_key, &project_id),
            }
        }

        let manager = ScratchpadManager::new(&project_id, "tool");

        match args.action {
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
                    // Read-shaped actions carry the snapshot too, so the Panel
                    // Todo widget rehydrates on reconnect / a fresh session
                    // attaching to an existing project — it used to stay hidden
                    // until the next mutating call.
                    snapshot: plan_snapshot(&manager).await,
                })
            }

            ScratchpadAction::Read => {
                if !manager.exists() {
                    return Ok(ScratchpadOutput {
                        success: true,
                        message: "No scratchpad exists for this project".to_string(),
                        content: None,
                        snapshot: None,
                    });
                }
                let content = manager.read().await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: "Scratchpad content loaded".to_string(),
                    content: Some(content),
                    snapshot: plan_snapshot(&manager).await,
                })
            }

            ScratchpadAction::SetObjective => {
                let value = args.value.unwrap_or_default();
                manager.set_objective(&value).await?;
                let (content, snapshot) = progress_parts(&manager).await;
                Ok(ScratchpadOutput {
                    success: true,
                    message: format!("Objective updated: {value}"),
                    content,
                    snapshot,
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
                let (content, snapshot) = progress_parts(&manager).await;
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
                    content,
                    snapshot,
                })
            }

            ScratchpadAction::StartItem => {
                let index = args.item_index.unwrap_or(0);
                manager.start_item(index).await?;
                let (content, snapshot) = progress_parts(&manager).await;
                Ok(ScratchpadOutput {
                    success: true,
                    message: format!("Item {index} marked in progress (current step)"),
                    content,
                    snapshot,
                })
            }

            ScratchpadAction::CompleteItem => {
                let index = args.item_index.unwrap_or(0);
                manager.complete_item(index).await?;
                let (content, snapshot) = progress_parts(&manager).await;
                Ok(ScratchpadOutput {
                    success: true,
                    message: format!("Item {index} marked as complete"),
                    content,
                    snapshot,
                })
            }

            ScratchpadAction::AppendNote => {
                let note = args.value.unwrap_or_default();
                manager.append_note(&note).await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: "Note appended".to_string(),
                    content: None,
                    snapshot: None,
                })
            }

            ScratchpadAction::Clear => {
                manager.clear().await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: "Scratchpad cleared".to_string(),
                    content: None,
                    snapshot: None,
                })
            }
        }
    }
}

/// Derive a filesystem-safe default scratchpad project id from the live
/// session key, for single-chat ad-hoc todos where the model omits
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
