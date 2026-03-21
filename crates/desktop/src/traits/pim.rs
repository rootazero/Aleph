//! Personal Information Management capability.

use async_trait::async_trait;

use crate::pim_types::*;
use crate::Result;

/// Access to the user's personal information: Notes, Calendar, Reminders, Contacts.
///
/// Each method family maps to a native PIM store (Apple Notes, Apple Calendar,
/// Apple Reminders, Apple Contacts on macOS; equivalent stores on other platforms).
#[async_trait]
pub trait PimCapability: Send + Sync {
    // ── Notes ───────────────────────────────────────────────────

    /// List notes, optionally filtered by folder name.
    async fn notes_list(&self, folder: Option<&str>) -> Result<Vec<NoteInfo>>;

    /// Read the full content of a note by ID.
    async fn notes_read(&self, note_id: &str) -> Result<NoteContent>;

    /// Create a new note in the given folder, returning the new note ID.
    async fn notes_create(&self, folder: &str, title: &str, body: &str) -> Result<String>;

    /// Update an existing note's title and/or body.
    async fn notes_update(
        &self,
        note_id: &str,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<()>;

    /// Delete a note by ID.
    async fn notes_delete(&self, note_id: &str) -> Result<()>;

    /// List available note folders/notebooks.
    async fn notes_folders(&self) -> Result<Vec<String>>;

    // ── Calendar ────────────────────────────────────────────────

    /// List events within a date range.
    ///
    /// `from` and `to` are ISO 8601 datetime strings.
    /// `calendar_id` optionally filters to a specific calendar.
    async fn calendar_list_events(
        &self,
        from: &str,
        to: &str,
        calendar_id: Option<&str>,
    ) -> Result<Vec<CalendarEvent>>;

    /// Get a single calendar event by ID.
    async fn calendar_get_event(&self, event_id: &str) -> Result<CalendarEvent>;

    /// Create a new calendar event, returning the new event ID.
    async fn calendar_create_event(&self, event: NewCalendarEvent) -> Result<String>;

    /// Update an existing calendar event.
    async fn calendar_update_event(&self, event_id: &str, event: NewCalendarEvent) -> Result<()>;

    /// Delete a calendar event by ID.
    async fn calendar_delete_event(&self, event_id: &str) -> Result<()>;

    /// List available calendars.
    async fn calendar_calendars(&self) -> Result<Vec<CalendarInfo>>;

    // ── Reminders ───────────────────────────────────────────────

    /// List reminders, optionally filtered by list ID and completion status.
    async fn reminders_list(
        &self,
        list_id: Option<&str>,
        include_completed: bool,
    ) -> Result<Vec<Reminder>>;

    /// Get a single reminder by ID.
    async fn reminders_get(&self, reminder_id: &str) -> Result<Reminder>;

    /// Create a new reminder, returning the new reminder ID.
    async fn reminders_create(&self, reminder: NewReminder) -> Result<String>;

    /// Mark a reminder as completed.
    async fn reminders_complete(&self, reminder_id: &str) -> Result<()>;

    /// Delete a reminder by ID.
    async fn reminders_delete(&self, reminder_id: &str) -> Result<()>;

    /// List available reminder lists.
    async fn reminders_lists(&self) -> Result<Vec<ReminderList>>;

    // ── Contacts ────────────────────────────────────────────────

    /// Search contacts by query string (name, email, phone).
    async fn contacts_search(&self, query: &str) -> Result<Vec<Contact>>;

    /// Get detailed contact information by ID.
    async fn contacts_get(&self, contact_id: &str) -> Result<ContactDetail>;

    /// List contact groups.
    async fn contacts_groups(&self) -> Result<Vec<ContactGroup>>;
}
