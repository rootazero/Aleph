//! PIM (Personal Information Management) tool — Calendar, Reminders, Notes,
//! Contacts via the platform-native `desktop/*` capability layer.

mod args;

pub use args::{PimArgs, PimOutput};

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde_json;
use tracing;

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::error::Result;
use crate::tools::AlephTool;
use aleph_desktop::pim_types::{NewCalendarEvent, NewReminder};

/// PIM tool — unified access to macOS Calendar, Reminders, Notes, and Contacts.
#[derive(Clone)]
pub struct PimTool {
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
    platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl PimTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            approval_policy: None,
            platform: None,
        }
    }

    /// Attach a `DesktopPlatform` so PIM operations are dispatched natively
    /// via `platform.pim()` instead of the legacy IPC bridge.
    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }

    /// Attach an approval policy to gate sensitive (write) actions.
    ///
    /// When a policy is set, mutating actions (create, update, delete, complete)
    /// are checked before execution. Read-only actions (list, get, search,
    /// calendars, lists, folders, groups) are always allowed.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }

    /// Returns `true` for PIM actions that modify data.
    fn is_write_action(action: &str) -> bool {
        matches!(
            action,
            "calendar_create"
                | "calendar_update"
                | "calendar_delete"
                | "reminders_create"
                | "reminders_complete"
                | "reminders_delete"
                | "notes_create"
                | "notes_update"
                | "notes_delete"
                | "contacts_create"
                | "contacts_update"
                | "contacts_delete"
        )
    }

    /// Human-readable description of a PIM action for approval prompts.
    fn describe_action(action: &str) -> String {
        match action {
            "calendar_create" => "Create a calendar event".to_string(),
            "calendar_update" => "Update a calendar event".to_string(),
            "calendar_delete" => "Delete a calendar event".to_string(),
            "reminders_create" => "Create a reminder".to_string(),
            "reminders_complete" => "Mark a reminder as completed".to_string(),
            "reminders_delete" => "Delete a reminder".to_string(),
            "notes_create" => "Create a note".to_string(),
            "notes_update" => "Update a note".to_string(),
            "notes_delete" => "Delete a note".to_string(),
            "contacts_create" => "Create a contact".to_string(),
            "contacts_update" => "Update a contact".to_string(),
            "contacts_delete" => "Delete a contact".to_string(),
            other => format!("PIM action: {other}"),
        }
    }

    fn approval_target(args: &PimArgs) -> String {
        match args.action.as_str() {
            "notes_create" => serde_json::json!({
                "action": "notes_create",
                "title": args.title,
                "body": args.body,
                "folder": args.folder,
            })
            .to_string(),
            "notes_update" => serde_json::json!({
                "action": "notes_update",
                "id": args.id,
                "title": args.title,
                "body": args.body,
            })
            .to_string(),
            "notes_delete" => serde_json::json!({
                "action": "notes_delete",
                "id": args.id,
            })
            .to_string(),
            "calendar_create" => serde_json::json!({
                "action": "calendar_create",
                "title": args.title,
                "calendar_id": args.calendar_id,
                "start": args.start,
                "end": args.end,
                "all_day": args.all_day,
                "location": args.location,
            })
            .to_string(),
            "calendar_update" => serde_json::json!({
                "action": "calendar_update",
                "id": args.id,
                "title": args.title,
                "calendar_id": args.calendar_id,
                "start": args.start,
                "end": args.end,
            })
            .to_string(),
            "calendar_delete" => serde_json::json!({
                "action": "calendar_delete",
                "id": args.id,
            })
            .to_string(),
            "reminders_create" => serde_json::json!({
                "action": "reminders_create",
                "title": args.title,
                "list_id": args.list_id,
                "due_date": args.due_date,
                "priority": args.priority,
            })
            .to_string(),
            "reminders_complete" => serde_json::json!({
                "action": "reminders_complete",
                "id": args.id,
            })
            .to_string(),
            "reminders_delete" => serde_json::json!({
                "action": "reminders_delete",
                "id": args.id,
            })
            .to_string(),
            other => Self::describe_action(other),
        }
    }

    /// Try to dispatch a PIM action via `DesktopPlatform.pim()`.
    ///
    /// Returns `Some(PimOutput)` if the action was handled, or `None` if the
    /// current platform layer does not implement that action.
    async fn call_via_platform(&self, args: &PimArgs) -> Option<PimOutput> {
        let platform = self.platform.as_ref()?;
        let pim = platform.pim()?;

        // Distinguish a missing required argument from an unimplemented action.
        // Both previously fell through as `None` and surfaced the misleading
        // "action is not implemented" message. `require!` returns an explicit
        // arg error (a failed `PimOutput`, consistent with how native errors are
        // reported below), so `None` now means only "unknown/unsupported action".
        macro_rules! require {
            ($opt:expr, $name:literal) => {
                match $opt {
                    Some(v) => v,
                    None => {
                        return Some(PimOutput {
                            success: false,
                            data: None,
                            message: Some(format!(
                                "PIM action '{}' requires the '{}' argument",
                                args.action, $name
                            )),
                        })
                    }
                }
            };
        }

        let result: std::result::Result<serde_json::Value, aleph_desktop::DesktopError> =
            match args.action.as_str() {
                // ── Notes ───────────────────────────────────────
                "notes_list" => pim.notes_list(args.folder.as_deref()).await.map(|v| {
                    serde_json::to_value(v).unwrap_or_else(|e| {
                        tracing::warn!(?e, "pim: serialization failed");
                        serde_json::Value::Null
                    })
                }),
                "notes_get" => {
                    let id = require!(args.id.as_deref(), "id");
                    pim.notes_read(id).await.map(|v| {
                        serde_json::to_value(v).unwrap_or_else(|e| {
                            tracing::warn!(?e, "pim: serialization failed");
                            serde_json::Value::Null
                        })
                    })
                }
                "notes_create" => {
                    let title = require!(args.title.as_deref(), "title");
                    let folder = args.folder.as_deref().unwrap_or("Notes");
                    let body = args.body.as_deref().unwrap_or("");
                    pim.notes_create(folder, title, body)
                        .await
                        .map(|id| serde_json::json!({ "id": id }))
                }
                "notes_update" => {
                    let id = require!(args.id.as_deref(), "id");
                    pim.notes_update(id, args.title.as_deref(), args.body.as_deref())
                        .await
                        .map(|()| serde_json::json!({ "updated": true }))
                }
                "notes_delete" => {
                    let id = require!(args.id.as_deref(), "id");
                    pim.notes_delete(id)
                        .await
                        .map(|()| serde_json::json!({ "deleted": true }))
                }
                "notes_folders" => pim.notes_folders().await.map(|v| {
                    serde_json::to_value(v).unwrap_or_else(|e| {
                        tracing::warn!(?e, "pim: serialization failed");
                        serde_json::Value::Null
                    })
                }),

                // ── Calendar ────────────────────────────────────
                "calendar_list" => {
                    let from = require!(args.from, "from");
                    let to = require!(args.to, "to");
                    pim.calendar_list_events(from, to, args.calendar_id.as_deref())
                        .await
                        .map(|v| {
                            serde_json::to_value(v).unwrap_or_else(|e| {
                                tracing::warn!(?e, "pim: serialization failed");
                                serde_json::Value::Null
                            })
                        })
                }
                "calendar_get" => {
                    let id = require!(args.id.as_deref(), "id");
                    pim.calendar_get_event(id).await.map(|v| {
                        serde_json::to_value(v).unwrap_or_else(|e| {
                            tracing::warn!(?e, "pim: serialization failed");
                            serde_json::Value::Null
                        })
                    })
                }
                "calendar_create" => {
                    let title = require!(args.title.as_deref(), "title");
                    let start = require!(args.start, "start");
                    let end = require!(args.end, "end");
                    let event = NewCalendarEvent {
                        title: title.to_string(),
                        calendar_id: args.calendar_id.clone().unwrap_or_default(),
                        start,
                        end,
                        all_day: args.all_day.unwrap_or(false),
                        location: args.location.clone(),
                        notes: args.notes.clone(),
                    };
                    pim.calendar_create_event(event)
                        .await
                        .map(|id| serde_json::json!({ "id": id }))
                }
                "calendar_update" => {
                    let id = require!(args.id.as_deref(), "id");
                    // Partial update must not blank omitted fields. The Swift
                    // handler assigns title/start/end UNCONDITIONALLY (only
                    // location/notes are `if let`-guarded), so an omitted title
                    // used to blank the event and an omitted start/end used to
                    // move it to now() — silent data loss on any partial edit.
                    // Read the current event and fill the gaps before writing.
                    let needs_current =
                        args.title.is_none() || args.start.is_none() || args.end.is_none();
                    // Fetch the current event only when a field is omitted;
                    // surface a fetch error as this action's error (this fn
                    // returns Option, so a bare `?` on the Result is not valid —
                    // thread it through the Result the arm already produces).
                    let current = if needs_current {
                        match pim.calendar_get_event(id).await {
                            Ok(c) => Some(c),
                            Err(e) => {
                                return Some(PimOutput {
                                    success: false,
                                    data: None,
                                    message: Some(e.to_string()),
                                })
                            }
                        }
                    } else {
                        None
                    };
                    let event = NewCalendarEvent {
                        title: args
                            .title
                            .clone()
                            .or_else(|| current.as_ref().map(|c| c.title.clone()))
                            .unwrap_or_default(),
                        calendar_id: args
                            .calendar_id
                            .clone()
                            .or_else(|| current.as_ref().map(|c| c.calendar_id.clone()))
                            .unwrap_or_default(),
                        start: args
                            .start
                            .or_else(|| current.as_ref().map(|c| c.start))
                            .unwrap_or_else(chrono::Utc::now),
                        end: args
                            .end
                            .or_else(|| current.as_ref().map(|c| c.end))
                            .unwrap_or_else(chrono::Utc::now),
                        all_day: args
                            .all_day
                            .or_else(|| current.as_ref().map(|c| c.all_day))
                            .unwrap_or(false),
                        // location/notes are already partial-safe on the wire
                        // (skip_serializing_if) + Swift (`if let`): omitting them
                        // keeps the stored value, so pass args through as-is.
                        location: args.location.clone(),
                        notes: args.notes.clone(),
                    };
                    pim.calendar_update_event(id, event)
                        .await
                        .map(|()| serde_json::json!({ "updated": true }))
                }
                "calendar_delete" => {
                    let id = require!(args.id.as_deref(), "id");
                    pim.calendar_delete_event(id)
                        .await
                        .map(|()| serde_json::json!({ "deleted": true }))
                }
                "calendar_calendars" => pim.calendar_calendars().await.map(|v| {
                    serde_json::to_value(v).unwrap_or_else(|e| {
                        tracing::warn!(?e, "pim: serialization failed");
                        serde_json::Value::Null
                    })
                }),

                // ── Reminders ───────────────────────────────────
                "reminders_list" => {
                    let include_completed = args.include_completed.unwrap_or(false);
                    pim.reminders_list(args.list_id.as_deref(), include_completed)
                        .await
                        .map(|v| {
                            serde_json::to_value(v).unwrap_or_else(|e| {
                                tracing::warn!(?e, "pim: serialization failed");
                                serde_json::Value::Null
                            })
                        })
                }
                "reminders_get" => {
                    let id = require!(args.id.as_deref(), "id");
                    pim.reminders_get(id).await.map(|v| {
                        serde_json::to_value(v).unwrap_or_else(|e| {
                            tracing::warn!(?e, "pim: serialization failed");
                            serde_json::Value::Null
                        })
                    })
                }
                "reminders_create" => {
                    let title = require!(args.title.as_deref(), "title");
                    let reminder = NewReminder {
                        title: title.to_string(),
                        list_id: args.list_id.clone().unwrap_or_default(),
                        due_date: args.due_date,
                        priority: args.priority.unwrap_or(0).clamp(0, 9) as u8,
                        notes: args.notes.clone(),
                    };
                    pim.reminders_create(reminder)
                        .await
                        .map(|id| serde_json::json!({ "id": id }))
                }
                "reminders_complete" => {
                    let id = require!(args.id.as_deref(), "id");
                    pim.reminders_complete(id)
                        .await
                        .map(|()| serde_json::json!({ "completed": true }))
                }
                "reminders_delete" => {
                    let id = require!(args.id.as_deref(), "id");
                    pim.reminders_delete(id)
                        .await
                        .map(|()| serde_json::json!({ "deleted": true }))
                }
                "reminders_lists" => pim.reminders_lists().await.map(|v| {
                    serde_json::to_value(v).unwrap_or_else(|e| {
                        tracing::warn!(?e, "pim: serialization failed");
                        serde_json::Value::Null
                    })
                }),

                // ── Contacts ────────────────────────────────────
                "contacts_search" => {
                    let query = require!(args.query.as_deref(), "query");
                    pim.contacts_search(query).await.map(|v| {
                        serde_json::to_value(v).unwrap_or_else(|e| {
                            tracing::warn!(?e, "pim: serialization failed");
                            serde_json::Value::Null
                        })
                    })
                }
                "contacts_get" => {
                    let id = require!(args.id.as_deref(), "id");
                    pim.contacts_get(id).await.map(|v| {
                        serde_json::to_value(v).unwrap_or_else(|e| {
                            tracing::warn!(?e, "pim: serialization failed");
                            serde_json::Value::Null
                        })
                    })
                }
                "contacts_groups" => pim.contacts_groups().await.map(|v| {
                    serde_json::to_value(v).unwrap_or_else(|e| {
                        tracing::warn!(?e, "pim: serialization failed");
                        serde_json::Value::Null
                    })
                }),

                // ── Mail ────────────────────────────────────────
                "mail_search" => {
                    let query = require!(args.query.as_deref(), "query");
                    // Clamp at the I/O boundary (P7): an unbounded limit could
                    // force the native bridge to enumerate a huge result set.
                    let limit = args.limit.unwrap_or(20).clamp(1, 200);
                    pim.mail_search(query, args.folder.as_deref(), limit)
                        .await
                        .map(|v| {
                            serde_json::to_value(v).unwrap_or_else(|e| {
                                tracing::warn!(?e, "pim: serialization failed");
                                serde_json::Value::Null
                            })
                        })
                }
                "mail_get" => {
                    let id = require!(args.id.as_deref(), "id");
                    pim.mail_get(id).await.map(|v| {
                        serde_json::to_value(v).unwrap_or_else(|e| {
                            tracing::warn!(?e, "pim: serialization failed");
                            serde_json::Value::Null
                        })
                    })
                }
                "mail_folders" => pim.mail_folders().await.map(|v| {
                    serde_json::to_value(v).unwrap_or_else(|e| {
                        tracing::warn!(?e, "pim: serialization failed");
                        serde_json::Value::Null
                    })
                }),

                // contacts_create, contacts_update, contacts_delete are not in
                // PimCapability today and remain unsupported on the platform path.
                _ => return None,
            };

        Some(match result {
            Ok(data) => PimOutput {
                success: true,
                data: Some(data),
                message: None,
            },
            Err(e) => PimOutput {
                success: false,
                data: None,
                message: Some(e.to_string()),
            },
        })
    }

    /// Check the approval policy for a sensitive (write) action.
    ///
    /// Returns `None` if the action is allowed (or no policy is configured),
    /// or `Some(PimOutput)` if the action is denied or requires user
    /// confirmation.
    async fn check_approval(&self, args: &PimArgs) -> Option<PimOutput> {
        let action = args.action.as_str();
        if !Self::is_write_action(action) {
            return None;
        }

        let policy = self.approval_policy.as_ref()?;

        let target = Self::approval_target(args);
        let display_target = Self::describe_action(action);
        let (agent_id, context) = crate::approval::audit_identity("pim", action, &display_target);
        let request = ActionRequest {
            action_type: ActionType::PimWrite,
            target,
            display_target,
            agent_id,
            context,
            timestamp: chrono::Utc::now(),
        };

        let decision = policy.check(&request).await;

        match decision {
            ApprovalDecision::Allow => {
                policy.record(&request, &decision).await;
                None
            }
            ApprovalDecision::Deny { ref reason } => {
                policy.record(&request, &decision).await;
                Some(PimOutput {
                    success: false,
                    data: None,
                    message: Some(format!("Action denied by approval policy: {reason}")),
                })
            }
            ApprovalDecision::Ask { ref prompt } => {
                // Don't record yet -- record() should be called after user responds
                Some(PimOutput {
                    success: false,
                    data: Some(serde_json::json!({
                        "approval_required": true,
                        "prompt": prompt,
                    })),
                    message: Some(format!("Approval required: {prompt}")),
                })
            }
        }
    }

    fn no_capability_output(&self) -> PimOutput {
        PimOutput {
            success: false,
            data: None,
            message: Some("PIM capability is not configured for this server build.".to_string()),
        }
    }

    fn unsupported_action_output(&self, args: &PimArgs) -> PimOutput {
        let message = if self
            .platform
            .as_ref()
            .and_then(|platform| platform.pim())
            .is_none()
        {
            "PIM capability is not available on this platform/build.".to_string()
        } else {
            format!(
                "PIM action '{}' is not implemented by the current `desktop/*` platform capability layer.",
                args.action
            )
        };

        PimOutput {
            success: false,
            data: None,
            message: Some(message),
        }
    }
}

impl Default for PimTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AlephTool for PimTool {
    const NAME: &'static str = "pim";
    const DESCRIPTION: &'static str = r#"Access Notes, Calendar, Reminders, and Contacts via platform-native `desktop/*` capabilities.

Calendar:
- calendar_list: List events in date range. Required: from, to (ISO 8601). Optional: calendar_id
- calendar_get: Get event details. Required: id
- calendar_create: Create event. Required: title, start, end. Optional: calendar_id, location, notes, all_day
- calendar_update: Update event. Required: id. Optional: title, start, end, location, notes
- calendar_delete: Delete event. Required: id
- calendar_calendars: List available calendars

Reminders:
- reminders_list: List reminders. Optional: list_id, include_completed
- reminders_get: Get reminder details. Required: id
- reminders_create: Create reminder. Required: title. Optional: list_id, due_date, priority, notes
- reminders_complete: Mark a reminder as completed. Required: id
- reminders_delete: Delete reminder. Required: id
- reminders_lists: List available reminder lists

Notes:
- notes_list: List notes. Optional: folder
- notes_get: Get note details. Required: id
- notes_create: Create note. Required: title. Optional: body, folder
- notes_update: Update note. Required: id. Optional: title, body
- notes_delete: Delete note. Required: id
- notes_folders: List available folders

Contacts:
- contacts_search: Search contacts. Required: query
- contacts_get: Get contact details. Required: id
- contacts_groups: List contact groups

Mail:
- mail_search: Search mail messages. Required: query. Optional: folder, limit (default 20)
- mail_get: Get a mail message (full body + attachments). Required: id
- mail_folders: List available mail folders"#;

    type Args = PimArgs;
    type Output = PimOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"pim(action="calendar_list", from="2026-02-27T00:00:00Z", to="2026-02-28T00:00:00Z")"#.to_string(),
            r#"pim(action="calendar_create", title="Team standup", start="2026-02-27T09:00:00Z", end="2026-02-27T09:30:00Z")"#.to_string(),
            r#"pim(action="reminders_create", title="Buy groceries", due_date="2026-02-28T18:00:00Z", priority=1)"#.to_string(),
            r#"pim(action="notes_create", title="Meeting notes", body="Discussed Q1 roadmap...")"#.to_string(),
            r#"pim(action="contacts_search", query="John")"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Check approval for write actions before attempting PIM execution.
        if let Some(out) = self.check_approval(&args).await {
            return Ok(out);
        }

        // Prefer DesktopPlatform.pim() for all supported PIM actions.
        if let Some(output) = self.call_via_platform(&args).await {
            return Ok(output);
        }

        if self.platform.is_none() {
            return Ok(self.no_capability_output());
        }

        Ok(self.unsupported_action_output(&args))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{ActionRequest, ApprovalDecision, ApprovalPolicy, ConfigApprovalPolicy};
    use crate::sync_primitives::Arc;
    use async_trait::async_trait;

    fn make_args(action: &str) -> PimArgs {
        PimArgs {
            action: action.into(),
            id: None,
            title: None,
            notes: None,
            from: None,
            to: None,
            start: None,
            end: None,
            calendar_id: None,
            location: None,
            all_day: None,
            list_id: None,
            due_date: None,
            priority: None,
            include_completed: None,
            body: None,
            folder: None,
            query: None,
            limit: None,
        }
    }

    // -- Write action classification tests -----------------------------------

    #[test]
    fn test_is_write_action() {
        assert!(PimTool::is_write_action("calendar_create"));
        assert!(PimTool::is_write_action("calendar_update"));
        assert!(PimTool::is_write_action("calendar_delete"));
        assert!(PimTool::is_write_action("reminders_create"));
        assert!(PimTool::is_write_action("reminders_complete"));
        assert!(PimTool::is_write_action("reminders_delete"));
        assert!(PimTool::is_write_action("notes_create"));
        assert!(PimTool::is_write_action("notes_update"));
        assert!(PimTool::is_write_action("notes_delete"));
        assert!(PimTool::is_write_action("contacts_create"));
        assert!(PimTool::is_write_action("contacts_update"));
        assert!(PimTool::is_write_action("contacts_delete"));
    }

    #[test]
    fn test_is_not_write_action() {
        assert!(!PimTool::is_write_action("calendar_list"));
        assert!(!PimTool::is_write_action("calendar_get"));
        assert!(!PimTool::is_write_action("calendar_calendars"));
        assert!(!PimTool::is_write_action("reminders_list"));
        assert!(!PimTool::is_write_action("reminders_get"));
        assert!(!PimTool::is_write_action("reminders_lists"));
        assert!(!PimTool::is_write_action("notes_list"));
        assert!(!PimTool::is_write_action("notes_get"));
        assert!(!PimTool::is_write_action("notes_folders"));
        assert!(!PimTool::is_write_action("contacts_search"));
        assert!(!PimTool::is_write_action("contacts_get"));
        assert!(!PimTool::is_write_action("contacts_groups"));
        assert!(!PimTool::is_write_action("mail_search"));
        assert!(!PimTool::is_write_action("mail_get"));
        assert!(!PimTool::is_write_action("mail_folders"));
    }
    /// A mock policy that returns a fixed decision for all checks.
    struct MockPolicy {
        decision: ApprovalDecision,
    }

    #[async_trait]
    impl ApprovalPolicy for MockPolicy {
        async fn check(&self, _request: &ActionRequest) -> ApprovalDecision {
            self.decision.clone()
        }
        async fn record(&self, _request: &ActionRequest, _decision: &ApprovalDecision) {}
    }

    #[tokio::test]
    async fn test_pim_approval_deny_blocks_write() {
        let policy = Arc::new(MockPolicy {
            decision: ApprovalDecision::Deny {
                reason: "write blocked".to_string(),
            },
        });
        let tool = PimTool::new().with_approval_policy(policy);

        let mut args = make_args("calendar_create");
        args.title = Some("Test".into());
        args.start = Some("2026-02-27T09:00:00Z".parse().unwrap());
        args.end = Some("2026-02-27T10:00:00Z".parse().unwrap());
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output.message.as_deref().unwrap().contains("Action denied"));
    }

    #[tokio::test]
    async fn test_pim_approval_ask_returns_prompt() {
        let policy = Arc::new(MockPolicy {
            decision: ApprovalDecision::Ask {
                prompt: "Confirm calendar creation".to_string(),
            },
        });
        let tool = PimTool::new().with_approval_policy(policy);

        let mut args = make_args("notes_create");
        args.title = Some("Test note".into());
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output
            .message
            .as_deref()
            .unwrap()
            .contains("Approval required"));
        let data = output.data.unwrap();
        assert_eq!(data["approval_required"], true);
    }

    #[tokio::test]
    async fn test_pim_approval_allows_read() {
        // Read-only actions should never be blocked even with a deny-all policy.
        let policy = Arc::new(MockPolicy {
            decision: ApprovalDecision::Deny {
                reason: "everything denied".to_string(),
            },
        });
        let tool = PimTool::new().with_approval_policy(policy);

        let mut args = make_args("calendar_list");
        args.from = Some("2026-02-27T00:00:00Z".parse().unwrap());
        args.to = Some("2026-02-28T00:00:00Z".parse().unwrap());
        let output = AlephTool::call(&tool, args).await.unwrap();
        // Should NOT be "Action denied". It will fail because this plain test
        // instance does not wire a platform capability.
        assert!(!output.success);
        let msg = output.message.as_deref().unwrap();
        assert!(
            !msg.contains("Action denied"),
            "Read-only action should bypass approval gate, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_pim_no_policy_allows_all() {
        // Without a policy, write actions should proceed as before.
        let tool = PimTool::new();

        let mut args = make_args("contacts_delete");
        args.id = Some("ct-123".into());
        let output = AlephTool::call(&tool, args).await.unwrap();
        // Should fail on missing PIM capability, NOT on approval.
        assert!(!output.success);
        let msg = output.message.as_deref().unwrap();
        assert!(
            !msg.contains("Action denied") && !msg.contains("Approval required"),
            "Without policy, should not hit approval gate, got: {msg}"
        );
        assert!(msg.contains("not configured"));
    }

    #[tokio::test]
    async fn test_pim_reports_missing_platform_capability() {
        let tool = PimTool::new();

        let mut args = make_args("notes_list");
        args.folder = Some("Notes".into());
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output
            .message
            .as_deref()
            .unwrap()
            .contains("not configured"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_pim_reports_legacy_contact_write_as_unsupported() {
        let tool =
            PimTool::new().with_platform(Arc::new(aleph_desktop_macos::MacOSPlatform::new()));

        let mut args = make_args("contacts_delete");
        args.id = Some("ct-123".into());
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output
            .message
            .as_deref()
            .unwrap()
            .contains("not implemented"));
    }

    struct CapturePolicy {
        captured: std::sync::Mutex<Vec<ActionRequest>>,
    }

    #[async_trait]
    impl ApprovalPolicy for CapturePolicy {
        async fn check(&self, request: &ActionRequest) -> ApprovalDecision {
            self.captured.lock().unwrap().push(request.clone());
            ApprovalDecision::Allow
        }
        async fn record(&self, _request: &ActionRequest, _decision: &ApprovalDecision) {}
    }

    #[tokio::test]
    async fn notes_create_target_carries_title_body_folder_for_blocklist_matching() {
        let policy = Arc::new(CapturePolicy {
            captured: std::sync::Mutex::new(Vec::new()),
        });
        let tool = PimTool::new().with_approval_policy(policy.clone());

        let mut args = make_args("notes_create");
        args.title = Some("Patient SSN 123-45-6789".into());
        args.body = Some("sensitive medical details".into());
        args.folder = Some("Archive".into());

        let _ = AlephTool::call(&tool, args).await.unwrap();
        let captured = policy.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert!(
            req.target.contains("Patient SSN 123-45-6789"),
            "notes_create target must include title for blocklist matching, got: {}",
            req.target
        );
        assert!(
            req.target.contains("sensitive medical details"),
            "notes_create target must include body for blocklist matching, got: {}",
            req.target
        );
        assert!(
            req.target.contains("Archive"),
            "notes_create target must include folder for blocklist matching, got: {}",
            req.target
        );
    }

    #[tokio::test]
    async fn calendar_create_target_carries_title_and_calendar_id() {
        let policy = Arc::new(CapturePolicy {
            captured: std::sync::Mutex::new(Vec::new()),
        });
        let tool = PimTool::new().with_approval_policy(policy.clone());

        let mut args = make_args("calendar_create");
        args.title = Some("Board Meeting - Confidential".into());
        args.calendar_id = Some("work-2026".into());
        args.start = Some("2026-04-22T09:00:00Z".parse().unwrap());
        args.end = Some("2026-04-22T10:00:00Z".parse().unwrap());

        let _ = AlephTool::call(&tool, args).await.unwrap();
        let captured = policy.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert!(
            req.target.contains("Board Meeting - Confidential"),
            "calendar_create target must include title, got: {}",
            req.target
        );
        assert!(
            req.target.contains("work-2026"),
            "calendar_create target must include calendar_id, got: {}",
            req.target
        );
    }

    #[tokio::test]
    async fn reminders_delete_target_carries_id_for_blocklist_matching() {
        let policy = Arc::new(CapturePolicy {
            captured: std::sync::Mutex::new(Vec::new()),
        });
        let tool = PimTool::new().with_approval_policy(policy.clone());

        let mut args = make_args("reminders_delete");
        args.id = Some("rm-PROD-9001".into());

        let _ = AlephTool::call(&tool, args).await.unwrap();
        let captured = policy.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert!(
            req.target.contains("rm-PROD-9001"),
            "reminders_delete target must include id for blocklist matching, got: {}",
            req.target
        );
    }

    fn deny_policy_blocking(action_type: ActionType, secret: &str) -> Arc<ConfigApprovalPolicy> {
        use crate::approval::{ConfigApprovalPolicy, PolicyConfig, PolicyRule};
        use std::collections::HashMap;
        let pattern = format!("*{secret}*");
        Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults: HashMap::new(),
            allowlist: vec![],
            blocklist: vec![PolicyRule {
                action_type,
                pattern,
            }],
        }))
    }

    #[tokio::test]
    async fn pim_blocklist_on_actual_title_actually_blocks() {
        let policy = deny_policy_blocking(ActionType::PimWrite, "Patient SSN");
        let tool = PimTool::new().with_approval_policy(policy as Arc<dyn ApprovalPolicy>);

        let mut args = make_args("notes_create");
        args.title = Some("Patient SSN 999-88-7777".into());

        let out = AlephTool::call(&tool, args).await.unwrap();
        assert!(!out.success);
        let msg = out.message.as_deref().unwrap_or("");
        assert!(
            msg.contains("denied"),
            "expected denial when blocklist matches PIM title, got: {msg}"
        );
    }
}
