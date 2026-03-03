//! PIM (Personal Information Management) tool — Calendar, Reminders, Notes, Contacts.
//!
//! Provides unified access to macOS PIM data through the Desktop Bridge.
//! Requires the Aleph Desktop Bridge to be connected. When the bridge is absent,
//! all operations return a friendly message instead of an error.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::desktop::{DesktopBridgeClient, DesktopRequest};
use crate::error::Result;
use crate::tools::AlephTool;

/// Arguments for the PIM tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PimArgs {
    /// The PIM action to perform.
    ///
    /// Calendar: "calendar_list", "calendar_get", "calendar_create", "calendar_update",
    ///           "calendar_delete", "calendar_calendars"
    /// Reminders: "reminders_list", "reminders_get", "reminders_create",
    ///            "reminders_complete", "reminders_delete", "reminders_lists"
    /// Notes: "notes_list", "notes_get", "notes_create", "notes_update",
    ///        "notes_delete", "notes_folders"
    /// Contacts: "contacts_search", "contacts_get", "contacts_create",
    ///           "contacts_update", "contacts_delete", "contacts_groups"
    pub action: String,

    /// Item ID (for get, update, delete, complete actions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Title for events, reminders, or notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Notes/description text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,

    /// Start of date range for calendar_list (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,

    /// End of date range for calendar_list (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,

    /// Event start time (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,

    /// Event end time (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,

    /// Calendar ID to filter or assign events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_id: Option<String>,

    /// Event location.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,

    /// Whether an event is all-day.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all_day: Option<bool>,

    /// Reminder list ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,

    /// Reminder due date (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,

    /// Reminder priority (0=none, 1=high, 5=medium, 9=low).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,

    /// Whether a reminder is completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<bool>,

    /// Whether to include completed reminders in list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_completed: Option<bool>,

    /// Note body text (HTML supported).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,

    /// Notes folder name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,

    /// Search query for contacts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    /// Contact given (first) name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub given_name: Option<String>,

    /// Contact family (last) name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family_name: Option<String>,

    /// Contact organization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,

    /// Contact phone numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_numbers: Option<Vec<String>>,

    /// Contact email addresses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emails: Option<Vec<String>>,
}

/// Output from PIM operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PimOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// PIM tool — unified access to macOS Calendar, Reminders, Notes, and Contacts.
#[derive(Clone)]
pub struct PimTool {
    client: DesktopBridgeClient,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl PimTool {
    pub fn new() -> Self {
        Self {
            client: DesktopBridgeClient::new(),
            approval_policy: None,
        }
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

    /// Check the approval policy for a sensitive (write) action.
    ///
    /// Returns `None` if the action is allowed (or no policy is configured),
    /// or `Some(PimOutput)` if the action is denied or requires user
    /// confirmation.
    async fn check_approval(&self, action: &str) -> Option<PimOutput> {
        if !Self::is_write_action(action) {
            return None;
        }

        let policy = self.approval_policy.as_ref()?;

        // TODO: Add PIM-specific ActionTypes (e.g., PimCalendarWrite, PimContactsWrite).
        // For now, reuse DesktopClick as a placeholder to integrate with the existing
        // approval infrastructure.
        let request = ActionRequest {
            action_type: ActionType::DesktopClick,
            target: Self::describe_action(action),
            agent_id: String::new(),
            context: format!("PIM write action: {action}"),
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
                // Don't record yet — record() should be called after user responds
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
}

impl Default for PimTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AlephTool for PimTool {
    const NAME: &'static str = "pim";
    const DESCRIPTION: &'static str = r#"Access macOS Calendar, Reminders, Notes, and Contacts via the Desktop Bridge.

Requires the Aleph Desktop Bridge (starts automatically with the server).

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
- reminders_complete: Mark reminder done/undone. Required: id, completed
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
- contacts_create: Create contact. Required: given_name. Optional: family_name, organization, notes, phone_numbers, emails
- contacts_update: Update contact. Required: id. Optional: given_name, family_name, organization, notes, phone_numbers, emails
- contacts_delete: Delete contact. Required: id
- contacts_groups: List contact groups"#;

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
        // Check approval for write actions BEFORE touching the bridge.
        if let Some(out) = self.check_approval(&args.action).await {
            return Ok(out);
        }

        // Gracefully handle the case where the Desktop Bridge is not connected.
        if !self.client.is_available() {
            return Ok(PimOutput {
                success: false,
                data: None,
                message: Some(
                    "Desktop bridge not connected. PIM operations require the Aleph Desktop \
                     Bridge for Calendar, Reminders, Notes, and Contacts access. It starts \
                     automatically with aleph, or can be run standalone via aleph-tauri."
                        .to_string(),
                ),
            });
        }

        let request = match build_pim_request(&args) {
            Ok(r) => r,
            Err(msg) => {
                return Ok(PimOutput {
                    success: false,
                    data: None,
                    message: Some(msg),
                });
            }
        };

        match self.client.send(request).await {
            Ok(result) => Ok(PimOutput {
                success: true,
                data: Some(result),
                message: None,
            }),
            Err(e) => Ok(PimOutput {
                success: false,
                data: None,
                message: Some(e.to_string()),
            }),
        }
    }
}

/// Build a `DesktopRequest` from PIM tool args, returning an error message string if invalid.
pub fn build_pim_request(args: &PimArgs) -> std::result::Result<DesktopRequest, String> {
    let req = match args.action.as_str() {
        // ── Calendar ────────────────────────────────────────────────────
        "calendar_list" => {
            let from = args.from.clone().ok_or_else(|| "calendar_list requires 'from' (ISO 8601 date)".to_string())?;
            let to = args.to.clone().ok_or_else(|| "calendar_list requires 'to' (ISO 8601 date)".to_string())?;
            DesktopRequest::PimCalendarList {
                from,
                to,
                calendar_id: args.calendar_id.clone(),
            }
        }
        "calendar_get" => {
            let id = args.id.clone().ok_or_else(|| "calendar_get requires 'id'".to_string())?;
            DesktopRequest::PimCalendarGet { id }
        }
        "calendar_create" => {
            let title = args.title.clone().ok_or_else(|| "calendar_create requires 'title'".to_string())?;
            let start = args.start.clone().ok_or_else(|| "calendar_create requires 'start' (ISO 8601)".to_string())?;
            let end = args.end.clone().ok_or_else(|| "calendar_create requires 'end' (ISO 8601)".to_string())?;
            DesktopRequest::PimCalendarCreate {
                title,
                start,
                end,
                calendar_id: args.calendar_id.clone(),
                location: args.location.clone(),
                notes: args.notes.clone(),
                all_day: args.all_day,
            }
        }
        "calendar_update" => {
            let id = args.id.clone().ok_or_else(|| "calendar_update requires 'id'".to_string())?;
            DesktopRequest::PimCalendarUpdate {
                id,
                title: args.title.clone(),
                start: args.start.clone(),
                end: args.end.clone(),
                location: args.location.clone(),
                notes: args.notes.clone(),
                all_day: args.all_day,
                calendar_id: args.calendar_id.clone(),
            }
        }
        "calendar_delete" => {
            let id = args.id.clone().ok_or_else(|| "calendar_delete requires 'id'".to_string())?;
            DesktopRequest::PimCalendarDelete { id }
        }
        "calendar_calendars" => DesktopRequest::PimCalendarCalendars,

        // ── Reminders ───────────────────────────────────────────────────
        "reminders_list" => DesktopRequest::PimRemindersList {
            list_id: args.list_id.clone(),
            include_completed: args.include_completed,
        },
        "reminders_get" => {
            let id = args.id.clone().ok_or_else(|| "reminders_get requires 'id'".to_string())?;
            DesktopRequest::PimRemindersGet { id }
        }
        "reminders_create" => {
            let title = args.title.clone().ok_or_else(|| "reminders_create requires 'title'".to_string())?;
            DesktopRequest::PimRemindersCreate {
                title,
                list_id: args.list_id.clone(),
                due_date: args.due_date.clone(),
                priority: args.priority,
                notes: args.notes.clone(),
            }
        }
        "reminders_complete" => {
            let id = args.id.clone().ok_or_else(|| "reminders_complete requires 'id'".to_string())?;
            let completed = args.completed.ok_or_else(|| "reminders_complete requires 'completed' (true/false)".to_string())?;
            DesktopRequest::PimRemindersComplete { id, completed }
        }
        "reminders_delete" => {
            let id = args.id.clone().ok_or_else(|| "reminders_delete requires 'id'".to_string())?;
            DesktopRequest::PimRemindersDelete { id }
        }
        "reminders_lists" => DesktopRequest::PimRemindersLists,

        // ── Notes ───────────────────────────────────────────────────────
        "notes_list" => DesktopRequest::PimNotesList {
            folder: args.folder.clone(),
        },
        "notes_get" => {
            let id = args.id.clone().ok_or_else(|| "notes_get requires 'id'".to_string())?;
            DesktopRequest::PimNotesGet { id }
        }
        "notes_create" => {
            let title = args.title.clone().ok_or_else(|| "notes_create requires 'title'".to_string())?;
            DesktopRequest::PimNotesCreate {
                title,
                body: args.body.clone(),
                folder: args.folder.clone(),
            }
        }
        "notes_update" => {
            let id = args.id.clone().ok_or_else(|| "notes_update requires 'id'".to_string())?;
            DesktopRequest::PimNotesUpdate {
                id,
                title: args.title.clone(),
                body: args.body.clone(),
            }
        }
        "notes_delete" => {
            let id = args.id.clone().ok_or_else(|| "notes_delete requires 'id'".to_string())?;
            DesktopRequest::PimNotesDelete { id }
        }
        "notes_folders" => DesktopRequest::PimNotesFolders,

        // ── Contacts ────────────────────────────────────────────────────
        "contacts_search" => {
            let query = args.query.clone().ok_or_else(|| "contacts_search requires 'query'".to_string())?;
            DesktopRequest::PimContactsSearch { query }
        }
        "contacts_get" => {
            let id = args.id.clone().ok_or_else(|| "contacts_get requires 'id'".to_string())?;
            DesktopRequest::PimContactsGet { id }
        }
        "contacts_create" => {
            let given_name = args.given_name.clone().ok_or_else(|| "contacts_create requires 'given_name'".to_string())?;
            DesktopRequest::PimContactsCreate {
                given_name,
                family_name: args.family_name.clone(),
                organization: args.organization.clone(),
                notes: args.notes.clone(),
                phone_numbers: args.phone_numbers.clone(),
                emails: args.emails.clone(),
            }
        }
        "contacts_update" => {
            let id = args.id.clone().ok_or_else(|| "contacts_update requires 'id'".to_string())?;
            DesktopRequest::PimContactsUpdate {
                id,
                given_name: args.given_name.clone(),
                family_name: args.family_name.clone(),
                organization: args.organization.clone(),
                notes: args.notes.clone(),
                phone_numbers: args.phone_numbers.clone(),
                emails: args.emails.clone(),
            }
        }
        "contacts_delete" => {
            let id = args.id.clone().ok_or_else(|| "contacts_delete requires 'id'".to_string())?;
            DesktopRequest::PimContactsDelete { id }
        }
        "contacts_groups" => DesktopRequest::PimContactsGroups,

        other => {
            return Err(format!(
                "Unknown PIM action: '{}'. Valid actions: \
                 calendar_list, calendar_get, calendar_create, calendar_update, calendar_delete, calendar_calendars, \
                 reminders_list, reminders_get, reminders_create, reminders_complete, reminders_delete, reminders_lists, \
                 notes_list, notes_get, notes_create, notes_update, notes_delete, notes_folders, \
                 contacts_search, contacts_get, contacts_create, contacts_update, contacts_delete, contacts_groups",
                other
            ));
        }
    };
    Ok(req)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            completed: None,
            include_completed: None,
            body: None,
            folder: None,
            query: None,
            given_name: None,
            family_name: None,
            organization: None,
            phone_numbers: None,
            emails: None,
        }
    }

    // ── Calendar tests ──────────────────────────────────────────────────

    #[test]
    fn test_build_calendar_list() {
        let mut args = make_args("calendar_list");
        args.from = Some("2026-02-27T00:00:00Z".into());
        args.to = Some("2026-02-28T00:00:00Z".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimCalendarList { .. }));
    }

    #[test]
    fn test_build_calendar_list_missing_from() {
        let mut args = make_args("calendar_list");
        args.to = Some("2026-02-28T00:00:00Z".into());
        assert!(build_pim_request(&args).is_err());
    }

    #[test]
    fn test_build_calendar_list_missing_to() {
        let mut args = make_args("calendar_list");
        args.from = Some("2026-02-27T00:00:00Z".into());
        assert!(build_pim_request(&args).is_err());
    }

    #[test]
    fn test_build_calendar_get() {
        let mut args = make_args("calendar_get");
        args.id = Some("evt-123".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimCalendarGet { .. }));
    }

    #[test]
    fn test_build_calendar_get_missing_id() {
        let args = make_args("calendar_get");
        assert!(build_pim_request(&args).is_err());
    }

    #[test]
    fn test_build_calendar_create() {
        let mut args = make_args("calendar_create");
        args.title = Some("Meeting".into());
        args.start = Some("2026-02-27T09:00:00Z".into());
        args.end = Some("2026-02-27T10:00:00Z".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimCalendarCreate { .. }));
    }

    #[test]
    fn test_build_calendar_create_missing_title() {
        let mut args = make_args("calendar_create");
        args.start = Some("2026-02-27T09:00:00Z".into());
        args.end = Some("2026-02-27T10:00:00Z".into());
        assert!(build_pim_request(&args).is_err());
    }

    #[test]
    fn test_build_calendar_update() {
        let mut args = make_args("calendar_update");
        args.id = Some("evt-123".into());
        args.title = Some("Updated title".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimCalendarUpdate { .. }));
    }

    #[test]
    fn test_build_calendar_delete() {
        let mut args = make_args("calendar_delete");
        args.id = Some("evt-123".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimCalendarDelete { .. }));
    }

    #[test]
    fn test_build_calendar_calendars() {
        let args = make_args("calendar_calendars");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimCalendarCalendars));
    }

    // ── Reminders tests ─────────────────────────────────────────────────

    #[test]
    fn test_build_reminders_list() {
        let args = make_args("reminders_list");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimRemindersList { .. }));
    }

    #[test]
    fn test_build_reminders_get() {
        let mut args = make_args("reminders_get");
        args.id = Some("rem-456".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimRemindersGet { .. }));
    }

    #[test]
    fn test_build_reminders_create() {
        let mut args = make_args("reminders_create");
        args.title = Some("Buy milk".into());
        args.priority = Some(1);
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimRemindersCreate { .. }));
    }

    #[test]
    fn test_build_reminders_create_missing_title() {
        let args = make_args("reminders_create");
        assert!(build_pim_request(&args).is_err());
    }

    #[test]
    fn test_build_reminders_complete() {
        let mut args = make_args("reminders_complete");
        args.id = Some("rem-456".into());
        args.completed = Some(true);
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimRemindersComplete { .. }));
    }

    #[test]
    fn test_build_reminders_complete_missing_completed() {
        let mut args = make_args("reminders_complete");
        args.id = Some("rem-456".into());
        assert!(build_pim_request(&args).is_err());
    }

    #[test]
    fn test_build_reminders_delete() {
        let mut args = make_args("reminders_delete");
        args.id = Some("rem-456".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimRemindersDelete { .. }));
    }

    #[test]
    fn test_build_reminders_lists() {
        let args = make_args("reminders_lists");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimRemindersLists));
    }

    // ── Notes tests ─────────────────────────────────────────────────────

    #[test]
    fn test_build_notes_list() {
        let args = make_args("notes_list");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimNotesList { .. }));
    }

    #[test]
    fn test_build_notes_get() {
        let mut args = make_args("notes_get");
        args.id = Some("note-789".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimNotesGet { .. }));
    }

    #[test]
    fn test_build_notes_create() {
        let mut args = make_args("notes_create");
        args.title = Some("My note".into());
        args.body = Some("Some content".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimNotesCreate { .. }));
    }

    #[test]
    fn test_build_notes_create_missing_title() {
        let args = make_args("notes_create");
        assert!(build_pim_request(&args).is_err());
    }

    #[test]
    fn test_build_notes_update() {
        let mut args = make_args("notes_update");
        args.id = Some("note-789".into());
        args.body = Some("Updated content".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimNotesUpdate { .. }));
    }

    #[test]
    fn test_build_notes_delete() {
        let mut args = make_args("notes_delete");
        args.id = Some("note-789".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimNotesDelete { .. }));
    }

    #[test]
    fn test_build_notes_folders() {
        let args = make_args("notes_folders");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimNotesFolders));
    }

    // ── Contacts tests ──────────────────────────────────────────────────

    #[test]
    fn test_build_contacts_search() {
        let mut args = make_args("contacts_search");
        args.query = Some("John".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimContactsSearch { .. }));
    }

    #[test]
    fn test_build_contacts_search_missing_query() {
        let args = make_args("contacts_search");
        assert!(build_pim_request(&args).is_err());
    }

    #[test]
    fn test_build_contacts_get() {
        let mut args = make_args("contacts_get");
        args.id = Some("ct-abc".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimContactsGet { .. }));
    }

    #[test]
    fn test_build_contacts_create() {
        let mut args = make_args("contacts_create");
        args.given_name = Some("Jane".into());
        args.family_name = Some("Doe".into());
        args.emails = Some(vec!["jane@example.com".into()]);
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimContactsCreate { .. }));
    }

    #[test]
    fn test_build_contacts_create_missing_given_name() {
        let args = make_args("contacts_create");
        assert!(build_pim_request(&args).is_err());
    }

    #[test]
    fn test_build_contacts_update() {
        let mut args = make_args("contacts_update");
        args.id = Some("ct-abc".into());
        args.organization = Some("Acme Corp".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimContactsUpdate { .. }));
    }

    #[test]
    fn test_build_contacts_delete() {
        let mut args = make_args("contacts_delete");
        args.id = Some("ct-abc".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimContactsDelete { .. }));
    }

    #[test]
    fn test_build_contacts_groups() {
        let args = make_args("contacts_groups");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::PimContactsGroups));
    }

    // ── Error tests ─────────────────────────────────────────────────────

    #[test]
    fn test_build_unknown_action() {
        let args = make_args("invalid_action");
        let err = build_pim_request(&args).unwrap_err();
        assert!(err.contains("invalid_action"), "error should mention the unknown action");
    }

    // ── Write action classification tests ───────────────────────────────

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
    }

    // ── Approval policy tests ───────────────────────────────────────────

    use crate::approval::{ActionRequest, ApprovalDecision, ApprovalPolicy};

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
        args.start = Some("2026-02-27T09:00:00Z".into());
        args.end = Some("2026-02-27T10:00:00Z".into());
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output
            .message
            .as_deref()
            .unwrap()
            .contains("Action denied"));
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
        args.from = Some("2026-02-27T00:00:00Z".into());
        args.to = Some("2026-02-28T00:00:00Z".into());
        let output = AlephTool::call(&tool, args).await.unwrap();
        // Should NOT be "Action denied". It will fail on bridge not available,
        // which is expected (approval gate was not triggered).
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
        // Should fail on bridge not available, NOT on approval
        assert!(!output.success);
        let msg = output.message.as_deref().unwrap();
        assert!(
            !msg.contains("Action denied") && !msg.contains("Approval required"),
            "Without policy, should not hit approval gate, got: {msg}"
        );
    }
}
