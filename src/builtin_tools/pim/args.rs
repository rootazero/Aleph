//! PIM tool argument and output types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Arguments for the PIM tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PimArgs {
    /// The PIM action to perform.
    ///
    /// Supported via `desktop/*` today:
    /// Calendar: "`calendar_list`", "`calendar_get`", "`calendar_create`",
    ///           "`calendar_update`", "`calendar_delete`", "`calendar_calendars`"
    /// Reminders: "`reminders_list`", "`reminders_get`", "`reminders_create`",
    ///            "`reminders_complete`", "`reminders_delete`", "`reminders_lists`"
    /// Notes: "`notes_list`", "`notes_get`", "`notes_create`", "`notes_update`",
    ///        "`notes_delete`", "`notes_folders`"
    /// Contacts: "`contacts_search`", "`contacts_get`", "`contacts_groups`"
    ///
    /// Legacy compatibility actions "`contacts_create`", "`contacts_update`", and
    /// "`contacts_delete`" are still parsed but currently return an unsupported
    /// message because they are not part of the current `PimCapability` trait.
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

    /// Start of date range for `calendar_list` (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<chrono::DateTime<chrono::Utc>>,

    /// End of date range for `calendar_list` (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<chrono::DateTime<chrono::Utc>>,

    /// Event start time (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<chrono::DateTime<chrono::Utc>>,

    /// Event end time (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<chrono::DateTime<chrono::Utc>>,

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
    pub due_date: Option<chrono::DateTime<chrono::Utc>>,

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

    /// Search query for contacts or mail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    /// Maximum number of results to return (used by `mail_search`; default 20).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,

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
