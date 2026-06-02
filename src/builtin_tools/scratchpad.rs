//! Scratchpad Tool — Project working memory management
//!
//! Allows the AI to manage project scratchpad files stored at
//! `~/.aleph/projects/<project_id>/scratchpad.md`.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::builtin_tools::scratchpad_registry;
use crate::error::Result;
use crate::memory::scratchpad::ScratchpadManager;
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

/// Arguments for the scratchpad tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScratchpadArgs {
    /// Project identifier (AI-assigned name for the project)
    pub project_id: String,
    /// Action to perform
    pub action: ScratchpadAction,
    /// Value for Initialize (objective), SetObjective, AppendNote
    pub value: Option<String>,
    /// Plan items for SetPlan
    pub items: Option<Vec<String>>,
    /// Item index for StartItem / CompleteItem (0-based)
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
    pub fn new() -> Self {
        Self { session_key: None }
    }

    /// Attach the shared live session-key handle. Pass the same handle the
    /// execution engine writes (see `execution_engine::execute`).
    pub fn with_session_key_handle(mut self, handle: Option<Arc<RwLock<String>>>) -> Self {
        self.session_key = handle;
        self
    }

    /// Current live session key, or empty string when no handle is wired.
    async fn current_session_key(&self) -> String {
        match &self.session_key {
            Some(h) => h.read().await.clone(),
            None => String::new(),
        }
    }
}

/// Echo the updated execution list back to the model after a mutating
/// action — Claude Code `TodoWrite` parity (the tool always returns the
/// current list), giving the loop continuous visibility through the tool-
/// result channel without touching the harness prompt builder (R10-safe).
/// Fail-soft: returns `None` on any read error rather than failing the op.
///
/// When the action just finished the objective (every box `[x]`), the echo
/// becomes a 收尾 completion summary instead of the in-progress checklist —
/// closing the goal-loop with hermes-agent `mark_done` parity. The summary is
/// structural (the model's own checkboxes), so the model stays sovereign over
/// completion (R7); the progress sink mirrors it to the user channel (R5).
async fn progress_echo(manager: &ScratchpadManager) -> Option<String> {
    manager.snapshot().await.ok().map(|s| {
        if s.is_objective_complete() {
            s.render_completion()
        } else {
            s.render_progress()
        }
    })
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
         achieved.";

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
        info!(
            project_id = %args.project_id,
            action = %args.action,
            "Scratchpad operation requested"
        );

        // Validate project_id to prevent path traversal
        if args.project_id.contains("..")
            || args.project_id.contains('/')
            || args.project_id.contains('\\')
            || args.project_id.contains('\0')
            || args.project_id.starts_with('.')
        {
            return Err(crate::error::AlephError::tool(
                "Invalid project_id: must not contain path separators, '..', null bytes, or start with '.'".to_string(),
            ));
        }

        // Bind this session to the project it is actively working, so the
        // goal-loop hook (ScratchpadGoalVerifier) can find this execution
        // list at stop time. Read-only access never re-points the binding;
        // Clear unbinds. No-op when no session handle is wired.
        let session_key = self.current_session_key().await;
        if !session_key.is_empty() {
            match args.action {
                ScratchpadAction::Read => {}
                ScratchpadAction::Clear => scratchpad_registry::clear(&session_key),
                _ => scratchpad_registry::set_active(&session_key, &args.project_id),
            }
        }

        let manager = ScratchpadManager::new(&args.project_id, "tool");

        match args.action {
            ScratchpadAction::Initialize => {
                if manager.exists() {
                    let content = manager.read().await?;
                    Ok(ScratchpadOutput {
                        success: true,
                        message: "Scratchpad already exists, returning current content".to_string(),
                        content: Some(content),
                    })
                } else {
                    manager.initialize(args.value.as_deref()).await?;
                    let content = manager.read().await?;
                    Ok(ScratchpadOutput {
                        success: true,
                        message: "Scratchpad initialized".to_string(),
                        content: Some(content),
                    })
                }
            }

            ScratchpadAction::Read => {
                if !manager.exists() {
                    return Ok(ScratchpadOutput {
                        success: true,
                        message: "No scratchpad exists for this project".to_string(),
                        content: None,
                    });
                }
                let content = manager.read().await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: "Scratchpad content loaded".to_string(),
                    content: Some(content),
                })
            }

            ScratchpadAction::SetObjective => {
                let value = args.value.unwrap_or_default();
                manager.set_objective(&value).await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: format!("Objective updated: {}", value),
                    content: progress_echo(&manager).await,
                })
            }

            ScratchpadAction::SetPlan => {
                let items = args.items.unwrap_or_default();
                let items_ref: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
                manager.set_plan(&items_ref).await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: format!("Plan set with {} items", items.len()),
                    content: progress_echo(&manager).await,
                })
            }

            ScratchpadAction::StartItem => {
                let index = args.item_index.unwrap_or(0);
                manager.start_item(index).await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: format!("Item {} marked in progress (current step)", index),
                    content: progress_echo(&manager).await,
                })
            }

            ScratchpadAction::CompleteItem => {
                let index = args.item_index.unwrap_or(0);
                manager.complete_item(index).await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: format!("Item {} marked as complete", index),
                    content: progress_echo(&manager).await,
                })
            }

            ScratchpadAction::AppendNote => {
                let note = args.value.unwrap_or_default();
                manager.append_note(&note).await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: "Note appended".to_string(),
                    content: None,
                })
            }

            ScratchpadAction::Clear => {
                manager.clear().await?;
                Ok(ScratchpadOutput {
                    success: true,
                    message: "Scratchpad cleared".to_string(),
                    content: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(args.project_id, "my-project");
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
}
