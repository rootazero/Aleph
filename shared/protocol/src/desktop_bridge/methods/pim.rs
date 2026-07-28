//! Personal Information Management — `pim.*` JSON-RPC method schemas.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── Deadlines ────────────────────────────────────────────────────────────────

/// Deadline for a PIM call.
///
/// The most generous namespace, and deliberately so: several of these drive
/// Notes and Mail through Apple Events, which launch the target app on first use
/// and then walk a library that can be large. A minute is what these calls have
/// always had (the client's old catch-all), and there is no evidence any of them
/// needs less — naming it here makes it a decision rather than a leftover.
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

pub const TIMEOUT_OVERRIDES_MS: &[(&str, u64)] = &[];

pub const METHOD_NOTES_LIST: &str = "pim.notes.list";
pub const METHOD_NOTES_GET: &str = "pim.notes.get";
pub const METHOD_NOTES_CREATE: &str = "pim.notes.create";
pub const METHOD_NOTES_UPDATE: &str = "pim.notes.update";
pub const METHOD_NOTES_DELETE: &str = "pim.notes.delete";
pub const METHOD_NOTES_FOLDERS: &str = "pim.notes.folders";

pub const METHOD_CALENDAR_EVENTS: &str = "pim.calendar.events";
pub const METHOD_CALENDAR_GET: &str = "pim.calendar.get";
pub const METHOD_CALENDAR_CREATE: &str = "pim.calendar.create";
pub const METHOD_CALENDAR_UPDATE: &str = "pim.calendar.update";
pub const METHOD_CALENDAR_DELETE: &str = "pim.calendar.delete";
pub const METHOD_CALENDAR_LISTS: &str = "pim.calendar.lists";

pub const METHOD_REMINDERS_LIST: &str = "pim.reminders.list";
pub const METHOD_REMINDERS_GET: &str = "pim.reminders.get";
pub const METHOD_REMINDERS_CREATE: &str = "pim.reminders.create";
pub const METHOD_REMINDERS_COMPLETE: &str = "pim.reminders.complete";
pub const METHOD_REMINDERS_DELETE: &str = "pim.reminders.delete";
pub const METHOD_REMINDERS_LISTS: &str = "pim.reminders.lists";

pub const METHOD_CONTACTS_SEARCH: &str = "pim.contacts.search";
pub const METHOD_CONTACTS_GET: &str = "pim.contacts.get";
pub const METHOD_CONTACTS_GROUPS: &str = "pim.contacts.groups";

pub const METHOD_MAIL_SEARCH: &str = "pim.mail.search";
pub const METHOD_MAIL_GET: &str = "pim.mail.get";
pub const METHOD_MAIL_FOLDERS: &str = "pim.mail.folders";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NoteInfo {
    pub id: String,
    pub title: String,
    pub folder: String,
    pub modified_at: chrono::DateTime<chrono::Utc>,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NoteContent {
    pub id: String,
    pub title: String,
    pub folder: String,
    pub body: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub modified_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalendarInfo {
    pub id: String,
    pub title: String,
    pub read_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub calendar_id: String,
    pub start: chrono::DateTime<chrono::Utc>,
    pub end: chrono::DateTime<chrono::Utc>,
    pub all_day: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Data required to create a new calendar event.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NewCalendarEvent {
    /// Event title.
    pub title: String,
    /// Calendar to add the event to.
    pub calendar_id: String,
    /// Start datetime.
    pub start: chrono::DateTime<chrono::Utc>,
    /// End datetime.
    pub end: chrono::DateTime<chrono::Utc>,
    /// Whether this is an all-day event.
    pub all_day: bool,
    /// Event location, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Event notes/description, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReminderList {
    pub id: String,
    pub title: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Reminder {
    pub id: String,
    pub title: String,
    pub list_id: String,
    pub completed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<chrono::DateTime<chrono::Utc>>,
    pub priority: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Data required to create a new reminder.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NewReminder {
    pub title: String,
    pub list_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<chrono::DateTime<chrono::Utc>>,
    pub priority: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Contact {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContactGroup {
    pub id: String,
    pub name: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LabeledValue {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContactDetail {
    pub id: String,
    pub name: String,
    pub emails: Vec<LabeledValue>,
    pub phones: Vec<LabeledValue>,
    pub addresses: Vec<LabeledValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday: Option<chrono::NaiveDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotesListParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotesListResult {
    pub notes: Vec<NoteInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotesGetParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotesCreateParams {
    pub folder: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotesCreateResult {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotesUpdateParams {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotesDeleteParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotesFoldersResult {
    pub folders: Vec<NotesFolderEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotesFolderEntry {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalendarEventsParams {
    pub from: chrono::DateTime<chrono::Utc>,
    pub to: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalendarEventsResult {
    pub events: Vec<CalendarEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalendarGetParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalendarCreateParams {
    pub title: String,
    pub start: chrono::DateTime<chrono::Utc>,
    pub end: chrono::DateTime<chrono::Utc>,
    pub calendar_id: String,
    pub all_day: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalendarCreateResult {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalendarUpdateParams {
    pub id: String,
    pub title: String,
    pub start: chrono::DateTime<chrono::Utc>,
    pub end: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalendarDeleteParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalendarListsResult {
    pub calendars: Vec<CalendarInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemindersListParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,
    pub include_completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemindersListResult {
    pub reminders: Vec<Reminder>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemindersGetParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemindersCreateParams {
    pub title: String,
    pub list_id: String,
    pub priority: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemindersCreateResult {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemindersCompleteParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemindersDeleteParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemindersListsResult {
    pub lists: Vec<ReminderList>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContactsSearchParams {
    pub query: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContactsSearchResult {
    pub contacts: Vec<Contact>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContactsGetParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContactsGroupsResult {
    pub groups: Vec<ContactGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailMessage {
    pub id: String,
    pub subject: String,
    pub sender: String,
    pub recipients: Vec<String>,
    pub date: chrono::DateTime<chrono::Utc>,
    pub body_preview: String,
    pub is_read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailAttachment {
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailMessageDetail {
    pub id: String,
    pub subject: String,
    pub sender: String,
    pub recipients: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub date: chrono::DateTime<chrono::Utc>,
    pub body: String,
    pub is_read: bool,
    pub attachments: Vec<MailAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailSearchParams {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailSearchResult {
    pub messages: Vec<MailMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailGetParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailGetResult {
    pub id: String,
    pub subject: String,
    pub sender: String,
    pub recipients: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub date: chrono::DateTime<chrono::Utc>,
    pub body: String,
    pub is_read: bool,
    pub attachments: Vec<MailAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailFolder {
    pub id: String,
    pub name: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailFoldersResult {
    pub folders: Vec<MailFolder>,
}
