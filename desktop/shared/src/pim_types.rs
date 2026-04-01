//! Shared types for Personal Information Management (PIM) capabilities.
//!
//! Covers Notes, Calendar, Reminders, and Contacts.

use serde::{Deserialize, Serialize};

// ── Notes ───────────────────────────────────────────────────────

/// Summary information about a note (without full body content).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteInfo {
    /// Unique identifier for the note.
    pub id: String,
    /// Note title.
    pub title: String,
    /// Folder or notebook the note belongs to.
    pub folder: String,
    /// ISO 8601 timestamp of last modification.
    pub modified_at: String,
    /// Short snippet or first few lines of the note body.
    pub snippet: String,
}

/// Full content of a note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteContent {
    /// Unique identifier for the note.
    pub id: String,
    /// Note title.
    pub title: String,
    /// Folder or notebook the note belongs to.
    pub folder: String,
    /// Full body content (plain text or HTML depending on source).
    pub body: String,
    /// ISO 8601 timestamp of creation.
    pub created_at: String,
    /// ISO 8601 timestamp of last modification.
    pub modified_at: String,
}

// ── Calendar ────────────────────────────────────────────────────

/// Information about a calendar account/source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarInfo {
    /// Unique identifier for the calendar.
    pub id: String,
    /// Display name of the calendar.
    pub title: String,
    /// Whether the calendar is read-only.
    pub read_only: bool,
    /// Calendar color as a hex string (e.g. "#FF5733"), if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// An existing calendar event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    /// Unique identifier for the event.
    pub id: String,
    /// Event title.
    pub title: String,
    /// Calendar this event belongs to.
    pub calendar_id: String,
    /// ISO 8601 start datetime.
    pub start: String,
    /// ISO 8601 end datetime.
    pub end: String,
    /// Whether this is an all-day event.
    pub all_day: bool,
    /// Event location, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Event notes/description, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Data required to create a new calendar event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCalendarEvent {
    /// Event title.
    pub title: String,
    /// Calendar to add the event to.
    pub calendar_id: String,
    /// ISO 8601 start datetime.
    pub start: String,
    /// ISO 8601 end datetime.
    pub end: String,
    /// Whether this is an all-day event.
    pub all_day: bool,
    /// Event location, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Event notes/description, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

// ── Reminders ───────────────────────────────────────────────────

/// A reminder list (grouping container).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReminderList {
    /// Unique identifier for the list.
    pub id: String,
    /// Display name.
    pub title: String,
    /// Number of incomplete reminders in this list.
    pub count: u32,
}

/// An existing reminder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    /// Unique identifier for the reminder.
    pub id: String,
    /// Reminder title.
    pub title: String,
    /// List this reminder belongs to.
    pub list_id: String,
    /// Whether the reminder is completed.
    pub completed: bool,
    /// ISO 8601 due date, if set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    /// Priority (0 = none, 1 = high, 5 = medium, 9 = low).
    pub priority: u8,
    /// Additional notes, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Data required to create a new reminder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewReminder {
    /// Reminder title.
    pub title: String,
    /// List to add the reminder to.
    pub list_id: String,
    /// ISO 8601 due date, if desired.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    /// Priority (0 = none, 1 = high, 5 = medium, 9 = low).
    pub priority: u8,
    /// Additional notes, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

// ── Contacts ────────────────────────────────────────────────────

/// A contact group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactGroup {
    /// Unique identifier for the group.
    pub id: String,
    /// Group name.
    pub name: String,
    /// Number of contacts in this group.
    pub count: u32,
}

/// Summary information about a contact (search result).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    /// Unique identifier for the contact.
    pub id: String,
    /// Full display name.
    pub name: String,
    /// Primary email address, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Primary phone number, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

/// Detailed contact information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactDetail {
    /// Unique identifier for the contact.
    pub id: String,
    /// Full display name.
    pub name: String,
    /// All email addresses.
    pub emails: Vec<LabeledValue>,
    /// All phone numbers.
    pub phones: Vec<LabeledValue>,
    /// All postal addresses.
    pub addresses: Vec<LabeledValue>,
    /// Organization / company name, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    /// Job title, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_title: Option<String>,
    /// Birthday as ISO 8601 date, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday: Option<String>,
    /// Additional notes, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// A value with a label (e.g. "work" phone, "home" email).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabeledValue {
    /// Label such as "home", "work", "mobile", "other".
    pub label: String,
    /// The value string.
    pub value: String,
}
