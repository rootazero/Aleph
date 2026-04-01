//! macOS PIM capability via SwiftBridge (aleph-bridge CLI).

use async_trait::async_trait;
use serde::Deserialize;

use aleph_desktop::bridge::SwiftBridge;
use aleph_desktop::pim_types::*;
use aleph_desktop::traits::PimCapability;
use aleph_desktop::Result;

/// macOS PIM implementation that delegates to the `aleph-bridge` Swift CLI.
pub struct MacOSPim {
    bridge: SwiftBridge,
}

impl MacOSPim {
    /// Create with default bridge binary discovery.
    pub fn new() -> Self {
        Self {
            bridge: SwiftBridge::default(),
        }
    }

}

#[async_trait]
impl PimCapability for MacOSPim {
    // ── Notes ───────────────────────────────────────────────────

    async fn notes_list(&self, folder: Option<&str>) -> Result<Vec<NoteInfo>> {
        #[derive(Deserialize)]
        struct Resp {
            notes: Vec<NoteInfo>,
        }
        let mut args = vec![];
        if let Some(f) = folder {
            args.push(("folder", f));
        }
        let resp: Resp = self.bridge.call("notes", "list", &args).await?;
        Ok(resp.notes)
    }

    async fn notes_read(&self, note_id: &str) -> Result<NoteContent> {
        let args = [("id", note_id)];
        self.bridge.call("notes", "get", &args).await
    }

    async fn notes_create(&self, folder: &str, title: &str, body: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Resp {
            id: String,
        }
        let mut args = vec![("title", title), ("body", body)];
        args.push(("folder", folder));
        let resp: Resp = self.bridge.call("notes", "create", &args).await?;
        Ok(resp.id)
    }

    async fn notes_update(
        &self,
        note_id: &str,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<()> {
        let mut args = vec![("id", note_id)];
        if let Some(t) = title {
            args.push(("title", t));
        }
        if let Some(b) = body {
            args.push(("body", b));
        }
        let _: serde_json::Value = self.bridge.call("notes", "update", &args).await?;
        Ok(())
    }

    async fn notes_delete(&self, note_id: &str) -> Result<()> {
        let args = [("id", note_id)];
        let _: serde_json::Value = self.bridge.call("notes", "delete", &args).await?;
        Ok(())
    }

    async fn notes_folders(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct FolderEntry {
            name: String,
        }
        #[derive(Deserialize)]
        struct Resp {
            folders: Vec<FolderEntry>,
        }
        let resp: Resp = self.bridge.call("notes", "folders", &[]).await?;
        Ok(resp.folders.into_iter().map(|f| f.name).collect())
    }

    // ── Calendar ────────────────────────────────────────────────

    async fn calendar_list_events(
        &self,
        from: &str,
        to: &str,
        calendar_id: Option<&str>,
    ) -> Result<Vec<CalendarEvent>> {
        #[derive(Deserialize)]
        struct Resp {
            events: Vec<CalendarEvent>,
        }
        let mut args = vec![("from", from), ("to", to)];
        if let Some(cid) = calendar_id {
            args.push(("calendar-id", cid));
        }
        let resp: Resp = self.bridge.call("calendar", "events", &args).await?;
        Ok(resp.events)
    }

    async fn calendar_get_event(&self, event_id: &str) -> Result<CalendarEvent> {
        let args = [("id", event_id)];
        self.bridge.call("calendar", "get", &args).await
    }

    async fn calendar_create_event(&self, event: NewCalendarEvent) -> Result<String> {
        #[derive(Deserialize)]
        struct Resp {
            id: String,
        }
        let all_day_str = event.all_day.to_string();
        let mut args = vec![
            ("title", event.title.as_str()),
            ("start", event.start.as_str()),
            ("end", event.end.as_str()),
            ("calendar-id", event.calendar_id.as_str()),
            ("all-day", all_day_str.as_str()),
        ];
        if let Some(ref loc) = event.location {
            args.push(("location", loc.as_str()));
        }
        if let Some(ref notes) = event.notes {
            args.push(("notes", notes.as_str()));
        }
        let resp: Resp = self.bridge.call("calendar", "create", &args).await?;
        Ok(resp.id)
    }

    async fn calendar_update_event(
        &self,
        event_id: &str,
        event: NewCalendarEvent,
    ) -> Result<()> {
        let mut args = vec![("id", event_id), ("title", event.title.as_str())];
        args.push(("start", event.start.as_str()));
        args.push(("end", event.end.as_str()));
        if let Some(ref loc) = event.location {
            args.push(("location", loc.as_str()));
        }
        if let Some(ref notes) = event.notes {
            args.push(("notes", notes.as_str()));
        }
        let _: serde_json::Value = self.bridge.call("calendar", "update", &args).await?;
        Ok(())
    }

    async fn calendar_delete_event(&self, event_id: &str) -> Result<()> {
        let args = [("id", event_id)];
        let _: serde_json::Value = self.bridge.call("calendar", "delete", &args).await?;
        Ok(())
    }

    async fn calendar_calendars(&self) -> Result<Vec<CalendarInfo>> {
        #[derive(Deserialize)]
        struct Resp {
            calendars: Vec<CalendarInfo>,
        }
        let resp: Resp = self.bridge.call("calendar", "calendars", &[]).await?;
        Ok(resp.calendars)
    }

    // ── Reminders ───────────────────────────────────────────────

    async fn reminders_list(
        &self,
        list_id: Option<&str>,
        include_completed: bool,
    ) -> Result<Vec<Reminder>> {
        #[derive(Deserialize)]
        struct Resp {
            reminders: Vec<Reminder>,
        }
        let mut args = vec![];
        if let Some(lid) = list_id {
            args.push(("list-id", lid));
        }
        if include_completed {
            args.push(("include-completed", "true"));
        }
        let resp: Resp = self.bridge.call("reminders", "list", &args).await?;
        Ok(resp.reminders)
    }

    async fn reminders_get(&self, reminder_id: &str) -> Result<Reminder> {
        let args = [("id", reminder_id)];
        self.bridge.call("reminders", "get", &args).await
    }

    async fn reminders_create(&self, reminder: NewReminder) -> Result<String> {
        #[derive(Deserialize)]
        struct Resp {
            id: String,
        }
        let priority_str = reminder.priority.to_string();
        let mut args = vec![
            ("title", reminder.title.as_str()),
            ("list-id", reminder.list_id.as_str()),
            ("priority", priority_str.as_str()),
        ];
        if let Some(ref due) = reminder.due_date {
            args.push(("due-date", due.as_str()));
        }
        if let Some(ref notes) = reminder.notes {
            args.push(("notes", notes.as_str()));
        }
        let resp: Resp = self.bridge.call("reminders", "create", &args).await?;
        Ok(resp.id)
    }

    async fn reminders_complete(&self, reminder_id: &str) -> Result<()> {
        let args = [("id", reminder_id)];
        let _: serde_json::Value = self.bridge.call("reminders", "complete", &args).await?;
        Ok(())
    }

    async fn reminders_delete(&self, reminder_id: &str) -> Result<()> {
        let args = [("id", reminder_id)];
        let _: serde_json::Value = self.bridge.call("reminders", "delete", &args).await?;
        Ok(())
    }

    async fn reminders_lists(&self) -> Result<Vec<ReminderList>> {
        #[derive(Deserialize)]
        struct Resp {
            lists: Vec<ReminderList>,
        }
        let resp: Resp = self.bridge.call("reminders", "lists", &[]).await?;
        Ok(resp.lists)
    }

    // ── Contacts ────────────────────────────────────────────────

    async fn contacts_search(&self, query: &str) -> Result<Vec<Contact>> {
        #[derive(Deserialize)]
        struct Resp {
            contacts: Vec<Contact>,
        }
        let args = [("query", query)];
        let resp: Resp = self.bridge.call("contacts", "search", &args).await?;
        Ok(resp.contacts)
    }

    async fn contacts_get(&self, contact_id: &str) -> Result<ContactDetail> {
        let args = [("id", contact_id)];
        self.bridge.call("contacts", "get", &args).await
    }

    async fn contacts_groups(&self) -> Result<Vec<ContactGroup>> {
        #[derive(Deserialize)]
        struct Resp {
            groups: Vec<ContactGroup>,
        }
        let resp: Resp = self.bridge.call("contacts", "groups", &[]).await?;
        Ok(resp.groups)
    }
}
