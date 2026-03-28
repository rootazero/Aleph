//! PIM (Personal Information Management) tool — Calendar, Reminders, Notes, Contacts.
//!
//! Provides unified access to macOS PIM data through the Desktop Bridge.
//! Requires the Aleph Desktop Bridge to be connected. When the bridge is absent,
//! all operations return a friendly message instead of an error.

mod args;
mod request;

pub use args::{PimArgs, PimOutput};
pub use request::build_pim_request;

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde_json;

use aleph_desktop::pim_types::{NewCalendarEvent, NewReminder};
use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::desktop::DesktopBridgeClient;
use crate::error::Result;
use crate::tools::AlephTool;

/// PIM tool — unified access to macOS Calendar, Reminders, Notes, and Contacts.
#[derive(Clone)]
pub struct PimTool {
    client: DesktopBridgeClient,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
    platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl PimTool {
    pub fn new() -> Self {
        Self {
            client: DesktopBridgeClient::new(),
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

    /// Try to dispatch a PIM action via `DesktopPlatform.pim()`.
    ///
    /// Returns `Some(PimOutput)` if the action was handled, or `None` to fall
    /// through to the legacy bridge path.
    async fn call_via_platform(&self, args: &PimArgs) -> Option<PimOutput> {
        let platform = self.platform.as_ref()?;
        let pim = platform.pim()?;

        let result: std::result::Result<serde_json::Value, aleph_desktop::DesktopError> =
            match args.action.as_str() {
                // ── Notes ───────────────────────────────────────
                "notes_list" => pim
                    .notes_list(args.folder.as_deref())
                    .await
                    .map(|v| serde_json::to_value(v).unwrap_or_default()),
                "notes_get" => {
                    let id = args.id.as_deref()?;
                    pim.notes_read(id)
                        .await
                        .map(|v| serde_json::to_value(v).unwrap_or_default())
                }
                "notes_create" => {
                    let title = args.title.as_deref()?;
                    let folder = args.folder.as_deref().unwrap_or("Notes");
                    let body = args.body.as_deref().unwrap_or("");
                    pim.notes_create(folder, title, body)
                        .await
                        .map(|id| serde_json::json!({ "id": id }))
                }
                "notes_update" => {
                    let id = args.id.as_deref()?;
                    pim.notes_update(id, args.title.as_deref(), args.body.as_deref())
                        .await
                        .map(|()| serde_json::json!({ "updated": true }))
                }
                "notes_delete" => {
                    let id = args.id.as_deref()?;
                    pim.notes_delete(id)
                        .await
                        .map(|()| serde_json::json!({ "deleted": true }))
                }
                "notes_folders" => pim
                    .notes_folders()
                    .await
                    .map(|v| serde_json::to_value(v).unwrap_or_default()),

                // ── Calendar ────────────────────────────────────
                "calendar_list" => {
                    let from = args.from.as_deref()?;
                    let to = args.to.as_deref()?;
                    pim.calendar_list_events(from, to, args.calendar_id.as_deref())
                        .await
                        .map(|v| serde_json::to_value(v).unwrap_or_default())
                }
                "calendar_get" => {
                    let id = args.id.as_deref()?;
                    pim.calendar_get_event(id)
                        .await
                        .map(|v| serde_json::to_value(v).unwrap_or_default())
                }
                "calendar_create" => {
                    let title = args.title.as_deref()?;
                    let start = args.start.as_deref()?;
                    let end = args.end.as_deref()?;
                    let event = NewCalendarEvent {
                        title: title.to_string(),
                        calendar_id: args.calendar_id.clone().unwrap_or_default(),
                        start: start.to_string(),
                        end: end.to_string(),
                        all_day: args.all_day.unwrap_or(false),
                        location: args.location.clone(),
                        notes: args.notes.clone(),
                    };
                    pim.calendar_create_event(event)
                        .await
                        .map(|id| serde_json::json!({ "id": id }))
                }
                "calendar_update" => {
                    let id = args.id.as_deref()?;
                    let event = NewCalendarEvent {
                        title: args.title.clone().unwrap_or_default(),
                        calendar_id: args.calendar_id.clone().unwrap_or_default(),
                        start: args.start.clone().unwrap_or_default(),
                        end: args.end.clone().unwrap_or_default(),
                        all_day: args.all_day.unwrap_or(false),
                        location: args.location.clone(),
                        notes: args.notes.clone(),
                    };
                    pim.calendar_update_event(id, event)
                        .await
                        .map(|()| serde_json::json!({ "updated": true }))
                }
                "calendar_delete" => {
                    let id = args.id.as_deref()?;
                    pim.calendar_delete_event(id)
                        .await
                        .map(|()| serde_json::json!({ "deleted": true }))
                }
                "calendar_calendars" => pim
                    .calendar_calendars()
                    .await
                    .map(|v| serde_json::to_value(v).unwrap_or_default()),

                // ── Reminders ───────────────────────────────────
                "reminders_list" => {
                    let include_completed = args.include_completed.unwrap_or(false);
                    pim.reminders_list(args.list_id.as_deref(), include_completed)
                        .await
                        .map(|v| serde_json::to_value(v).unwrap_or_default())
                }
                "reminders_get" => {
                    let id = args.id.as_deref()?;
                    pim.reminders_get(id)
                        .await
                        .map(|v| serde_json::to_value(v).unwrap_or_default())
                }
                "reminders_create" => {
                    let title = args.title.as_deref()?;
                    let reminder = NewReminder {
                        title: title.to_string(),
                        list_id: args.list_id.clone().unwrap_or_default(),
                        due_date: args.due_date.clone(),
                        priority: args.priority.unwrap_or(0).clamp(0, 9) as u8,
                        notes: args.notes.clone(),
                    };
                    pim.reminders_create(reminder)
                        .await
                        .map(|id| serde_json::json!({ "id": id }))
                }
                "reminders_complete" => {
                    let id = args.id.as_deref()?;
                    pim.reminders_complete(id)
                        .await
                        .map(|()| serde_json::json!({ "completed": true }))
                }
                "reminders_delete" => {
                    let id = args.id.as_deref()?;
                    pim.reminders_delete(id)
                        .await
                        .map(|()| serde_json::json!({ "deleted": true }))
                }
                "reminders_lists" => pim
                    .reminders_lists()
                    .await
                    .map(|v| serde_json::to_value(v).unwrap_or_default()),

                // ── Contacts ────────────────────────────────────
                "contacts_search" => {
                    let query = args.query.as_deref()?;
                    pim.contacts_search(query)
                        .await
                        .map(|v| serde_json::to_value(v).unwrap_or_default())
                }
                "contacts_get" => {
                    let id = args.id.as_deref()?;
                    pim.contacts_get(id)
                        .await
                        .map(|v| serde_json::to_value(v).unwrap_or_default())
                }
                "contacts_groups" => pim
                    .contacts_groups()
                    .await
                    .map(|v| serde_json::to_value(v).unwrap_or_default()),

                // contacts_create, contacts_update, contacts_delete are not in
                // PimCapability trait — fall through to the legacy bridge.
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

        // Prefer DesktopPlatform.pim() over legacy bridge
        if let Some(output) = self.call_via_platform(&args).await {
            return Ok(output);
        }

        // Gracefully handle the case where the Desktop Bridge is not connected.
        if !self.client.is_available() {
            return Ok(PimOutput {
                success: false,
                data: None,
                message: Some(
                    "Desktop bridge not connected. PIM operations require the desktop bridge \
                     for Calendar, Reminders, Notes, and Contacts access. Ensure the desktop \
                     platform capability is available."
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

    // -- Calendar tests ------------------------------------------------------

    #[test]
    fn test_build_calendar_list() {
        let mut args = make_args("calendar_list");
        args.from = Some("2026-02-27T00:00:00Z".into());
        args.to = Some("2026-02-28T00:00:00Z".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimCalendarList { .. }));
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
        assert!(matches!(req, crate::desktop::DesktopRequest::PimCalendarGet { .. }));
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
        assert!(matches!(req, crate::desktop::DesktopRequest::PimCalendarCreate { .. }));
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
        assert!(matches!(req, crate::desktop::DesktopRequest::PimCalendarUpdate { .. }));
    }

    #[test]
    fn test_build_calendar_delete() {
        let mut args = make_args("calendar_delete");
        args.id = Some("evt-123".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimCalendarDelete { .. }));
    }

    #[test]
    fn test_build_calendar_calendars() {
        let args = make_args("calendar_calendars");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimCalendarCalendars));
    }

    // -- Reminders tests -----------------------------------------------------

    #[test]
    fn test_build_reminders_list() {
        let args = make_args("reminders_list");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimRemindersList { .. }));
    }

    #[test]
    fn test_build_reminders_get() {
        let mut args = make_args("reminders_get");
        args.id = Some("rem-456".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimRemindersGet { .. }));
    }

    #[test]
    fn test_build_reminders_create() {
        let mut args = make_args("reminders_create");
        args.title = Some("Buy milk".into());
        args.priority = Some(1);
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimRemindersCreate { .. }));
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
        assert!(matches!(req, crate::desktop::DesktopRequest::PimRemindersComplete { .. }));
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
        assert!(matches!(req, crate::desktop::DesktopRequest::PimRemindersDelete { .. }));
    }

    #[test]
    fn test_build_reminders_lists() {
        let args = make_args("reminders_lists");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimRemindersLists));
    }

    // -- Notes tests ---------------------------------------------------------

    #[test]
    fn test_build_notes_list() {
        let args = make_args("notes_list");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimNotesList { .. }));
    }

    #[test]
    fn test_build_notes_get() {
        let mut args = make_args("notes_get");
        args.id = Some("note-789".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimNotesGet { .. }));
    }

    #[test]
    fn test_build_notes_create() {
        let mut args = make_args("notes_create");
        args.title = Some("My note".into());
        args.body = Some("Some content".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimNotesCreate { .. }));
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
        assert!(matches!(req, crate::desktop::DesktopRequest::PimNotesUpdate { .. }));
    }

    #[test]
    fn test_build_notes_delete() {
        let mut args = make_args("notes_delete");
        args.id = Some("note-789".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimNotesDelete { .. }));
    }

    #[test]
    fn test_build_notes_folders() {
        let args = make_args("notes_folders");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimNotesFolders));
    }

    // -- Contacts tests ------------------------------------------------------

    #[test]
    fn test_build_contacts_search() {
        let mut args = make_args("contacts_search");
        args.query = Some("John".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimContactsSearch { .. }));
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
        assert!(matches!(req, crate::desktop::DesktopRequest::PimContactsGet { .. }));
    }

    #[test]
    fn test_build_contacts_create() {
        let mut args = make_args("contacts_create");
        args.given_name = Some("Jane".into());
        args.family_name = Some("Doe".into());
        args.emails = Some(vec!["jane@example.com".into()]);
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimContactsCreate { .. }));
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
        assert!(matches!(req, crate::desktop::DesktopRequest::PimContactsUpdate { .. }));
    }

    #[test]
    fn test_build_contacts_delete() {
        let mut args = make_args("contacts_delete");
        args.id = Some("ct-abc".into());
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimContactsDelete { .. }));
    }

    #[test]
    fn test_build_contacts_groups() {
        let args = make_args("contacts_groups");
        let req = build_pim_request(&args).unwrap();
        assert!(matches!(req, crate::desktop::DesktopRequest::PimContactsGroups));
    }

    // -- Error tests ---------------------------------------------------------

    #[test]
    fn test_build_unknown_action() {
        let args = make_args("invalid_action");
        let err = build_pim_request(&args).unwrap_err();
        assert!(err.contains("invalid_action"), "error should mention the unknown action");
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
    }

    // -- Approval policy tests -----------------------------------------------

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
