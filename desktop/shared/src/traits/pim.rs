//! Personal Information Management capability.

use async_trait::async_trait;

use crate::pim_types::{
    CalendarEvent, CalendarInfo, Contact, ContactDetail, ContactGroup, NewCalendarEvent,
    NewReminder, NoteContent, NoteInfo, Reminder, ReminderList,
};
use crate::Result;

/// Access to the user's personal information: Notes, Calendar, Reminders, Contacts.
///
/// Each method family maps to a native PIM store (Apple Notes, Apple Calendar,
/// Apple Reminders, Apple Contacts on macOS; equivalent stores on other platforms).
#[async_trait]
pub trait PimCapability: Send + Sync {
    // ── Notes ───────────────────────────────────────────────────
    //
    // The Apple-ecosystem domains (Notes/Calendar/Reminders/Contacts) default to
    // `NotImplemented`: platforms without an equivalent native store inherit the
    // stub rather than re-declaring it, and macOS overrides every method with a
    // real bridge-backed implementation. Mirrors the `MediaCapability` pattern.

    /// List notes, optionally filtered by folder name.
    async fn notes_list(&self, folder: Option<&str>) -> Result<Vec<NoteInfo>> {
        let _ = folder;
        Err(crate::DesktopError::NotImplemented(
            "notes are not available on this platform".into(),
        ))
    }

    /// Read the full content of a note by ID.
    async fn notes_read(&self, note_id: &str) -> Result<NoteContent> {
        let _ = note_id;
        Err(crate::DesktopError::NotImplemented(
            "notes are not available on this platform".into(),
        ))
    }

    /// Create a new note in the given folder, returning the new note ID.
    async fn notes_create(&self, folder: &str, title: &str, body: &str) -> Result<String> {
        let _ = (folder, title, body);
        Err(crate::DesktopError::NotImplemented(
            "notes are not available on this platform".into(),
        ))
    }

    /// Update an existing note's title and/or body.
    async fn notes_update(
        &self,
        note_id: &str,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<()> {
        let _ = (note_id, title, body);
        Err(crate::DesktopError::NotImplemented(
            "notes are not available on this platform".into(),
        ))
    }

    /// Delete a note by ID.
    async fn notes_delete(&self, note_id: &str) -> Result<()> {
        let _ = note_id;
        Err(crate::DesktopError::NotImplemented(
            "notes are not available on this platform".into(),
        ))
    }

    /// List available note folders/notebooks.
    async fn notes_folders(&self) -> Result<Vec<String>> {
        Err(crate::DesktopError::NotImplemented(
            "notes are not available on this platform".into(),
        ))
    }

    // ── Calendar ────────────────────────────────────────────────

    /// List events within a date range.
    ///
    /// `calendar_id` optionally filters to a specific calendar.
    async fn calendar_list_events(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        calendar_id: Option<&str>,
    ) -> Result<Vec<CalendarEvent>> {
        let _ = (from, to, calendar_id);
        Err(crate::DesktopError::NotImplemented(
            "calendar is not available on this platform".into(),
        ))
    }

    /// Get a single calendar event by ID.
    async fn calendar_get_event(&self, event_id: &str) -> Result<CalendarEvent> {
        let _ = event_id;
        Err(crate::DesktopError::NotImplemented(
            "calendar is not available on this platform".into(),
        ))
    }

    /// Create a new calendar event, returning the new event ID.
    async fn calendar_create_event(&self, event: NewCalendarEvent) -> Result<String> {
        let _ = event;
        Err(crate::DesktopError::NotImplemented(
            "calendar is not available on this platform".into(),
        ))
    }

    /// Update an existing calendar event.
    async fn calendar_update_event(&self, event_id: &str, event: NewCalendarEvent) -> Result<()> {
        let _ = (event_id, event);
        Err(crate::DesktopError::NotImplemented(
            "calendar is not available on this platform".into(),
        ))
    }

    /// Delete a calendar event by ID.
    async fn calendar_delete_event(&self, event_id: &str) -> Result<()> {
        let _ = event_id;
        Err(crate::DesktopError::NotImplemented(
            "calendar is not available on this platform".into(),
        ))
    }

    /// List available calendars.
    async fn calendar_calendars(&self) -> Result<Vec<CalendarInfo>> {
        Err(crate::DesktopError::NotImplemented(
            "calendar is not available on this platform".into(),
        ))
    }

    // ── Reminders ───────────────────────────────────────────────

    /// List reminders, optionally filtered by list ID and completion status.
    async fn reminders_list(
        &self,
        list_id: Option<&str>,
        include_completed: bool,
    ) -> Result<Vec<Reminder>> {
        let _ = (list_id, include_completed);
        Err(crate::DesktopError::NotImplemented(
            "reminders are not available on this platform".into(),
        ))
    }

    /// Get a single reminder by ID.
    async fn reminders_get(&self, reminder_id: &str) -> Result<Reminder> {
        let _ = reminder_id;
        Err(crate::DesktopError::NotImplemented(
            "reminders are not available on this platform".into(),
        ))
    }

    /// Create a new reminder, returning the new reminder ID.
    async fn reminders_create(&self, reminder: NewReminder) -> Result<String> {
        let _ = reminder;
        Err(crate::DesktopError::NotImplemented(
            "reminders are not available on this platform".into(),
        ))
    }

    /// Mark a reminder as completed.
    async fn reminders_complete(&self, reminder_id: &str) -> Result<()> {
        let _ = reminder_id;
        Err(crate::DesktopError::NotImplemented(
            "reminders are not available on this platform".into(),
        ))
    }

    /// Delete a reminder by ID.
    async fn reminders_delete(&self, reminder_id: &str) -> Result<()> {
        let _ = reminder_id;
        Err(crate::DesktopError::NotImplemented(
            "reminders are not available on this platform".into(),
        ))
    }

    /// List available reminder lists.
    async fn reminders_lists(&self) -> Result<Vec<ReminderList>> {
        Err(crate::DesktopError::NotImplemented(
            "reminders are not available on this platform".into(),
        ))
    }

    // ── Contacts ────────────────────────────────────────────────

    /// Search contacts by query string (name, email, phone).
    async fn contacts_search(&self, query: &str) -> Result<Vec<Contact>> {
        let _ = query;
        Err(crate::DesktopError::NotImplemented(
            "contacts are not available on this platform".into(),
        ))
    }

    /// Get detailed contact information by ID.
    async fn contacts_get(&self, contact_id: &str) -> Result<ContactDetail> {
        let _ = contact_id;
        Err(crate::DesktopError::NotImplemented(
            "contacts are not available on this platform".into(),
        ))
    }

    /// List contact groups.
    async fn contacts_groups(&self) -> Result<Vec<ContactGroup>> {
        Err(crate::DesktopError::NotImplemented(
            "contacts are not available on this platform".into(),
        ))
    }

    // ── Mail ──────────────────────────────────────────────────────

    /// Search mail messages by query string.
    async fn mail_search(
        &self,
        query: &str,
        folder: Option<&str>,
        limit: u32,
    ) -> Result<Vec<crate::pim_types::MailMessage>>;

    /// Get a single mail message by ID.
    async fn mail_get(&self, message_id: &str) -> Result<crate::pim_types::MailMessageDetail>;

    /// List available mail folders.
    async fn mail_folders(&self) -> Result<Vec<crate::pim_types::MailFolder>>;
}
