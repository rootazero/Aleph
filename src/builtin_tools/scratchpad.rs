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
use crate::memory::scratchpad::{PlanItemStatus, ScratchpadManager, ScratchpadSnapshot};
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

/// Serde-friendly mirror of `PlanItemStatus` (which derives no serde).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatusDto {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanItemDto {
    pub text: String,
    pub status: PlanItemStatusDto,
}

/// Structured snapshot of the scratchpad plan, attached to `ScratchpadOutput`
/// so the Panel can render a live Todo widget (rides the existing
/// `tool_call_completed` event; no new protocol variant — R4/R10).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanSnapshotDto {
    pub objective: Option<String>,
    pub items: Vec<PlanItemDto>,
    pub complete: bool,
}

impl From<&ScratchpadSnapshot> for PlanSnapshotDto {
    fn from(s: &ScratchpadSnapshot) -> Self {
        Self {
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
    /// Value for Initialize (objective), `SetObjective`, `AppendNote`
    pub value: Option<String>,
    /// Plan items for `SetPlan`
    pub items: Option<Vec<String>>,
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
async fn progress_parts(manager: &ScratchpadManager) -> (Option<String>, Option<PlanSnapshotDto>) {
    match manager.snapshot().await {
        Ok(s) => {
            let text = if s.is_objective_complete() {
                s.render_completion()
            } else {
                s.render_progress()
            };
            (Some(text), Some(PlanSnapshotDto::from(&s)))
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
         step at a time. Mark the step you are about to work with \
         action='start_item' (it becomes the single in-progress step), and \
         action='complete_item' when it is done; both echo the updated list back \
         to you so you always see current progress. The scratchpad persists \
         across sessions. While an objective is set and plan items remain \
         unfinished, the goal-loop keeps this session running so you work through \
         them step by step — call action='clear' once the objective is fully \
         achieved. The project_id is optional — omit it to use the current chat's scratchpad.";

    type Args = ScratchpadArgs;
    type Output = ScratchpadOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "scratchpad(project_id='blog-redesign', action='initialize', value='Redesign the blog layout with modern CSS')"
                .to_string(),
            "scratchpad(project_id='blog-redesign', action='set_plan', items=['Design mockup', 'Implement header', 'Add responsive styles'])"
                .to_string(),
            "scratchpad(project_id='blog-redesign', action='start_item', item_index=0)"
                .to_string(),
            "scratchpad(project_id='blog-redesign', action='complete_item', item_index=0)"
                .to_string(),
            "scratchpad(project_id='blog-redesign', action='append_note', value='User prefers dark theme')"
                .to_string(),
        ])
    }

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
                if manager.exists() {
                    let content = manager.read().await?;
                    Ok(ScratchpadOutput {
                        success: true,
                        message: "Scratchpad already exists, returning current content".to_string(),
                        content: Some(content),
                        snapshot: None,
                    })
                } else {
                    manager.initialize(args.value.as_deref()).await?;
                    let content = manager.read().await?;
                    Ok(ScratchpadOutput {
                        success: true,
                        message: "Scratchpad initialized".to_string(),
                        content: Some(content),
                        snapshot: None,
                    })
                }
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
                    snapshot: None,
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
                let items = args.items.unwrap_or_default();
                let items_ref: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
                manager.set_plan(&items_ref).await?;
                let (content, snapshot) = progress_parts(&manager).await;
                Ok(ScratchpadOutput {
                    success: true,
                    message: format!("Plan set with {} items", items.len()),
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
        let dto = PlanSnapshotDto::from(&snap);
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
        assert!(PlanSnapshotDto::from(&snap).complete);
    }

    #[test]
    fn test_tool_name_and_description() {
        assert_eq!(ScratchpadTool::NAME, "scratchpad");
        assert!(ScratchpadTool::DESCRIPTION.contains("scratchpad"));
    }

    #[test]
    fn test_tool_examples() {
        let tool = ScratchpadTool::new();
        let examples = tool.examples();
        assert!(examples.is_some());
        assert_eq!(examples.unwrap().len(), 5);
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
        assert!(def.llm_context.is_some());
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
}
