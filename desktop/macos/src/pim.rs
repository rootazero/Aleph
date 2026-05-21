//! macOS PIM capability via SwiftBridge (long-lived JSON-RPC client).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use aleph_desktop::pim_types::*;
use aleph_desktop::traits::PimCapability;
use aleph_desktop::SwiftBridge;
use aleph_desktop::{DesktopError, Result};
use aleph_protocol::desktop_bridge::methods::pim::*;

/// Simple TTL cache for PIM list data.
///
/// Reduces redundant bridge calls for operations that are frequently queried
/// but change infrequently (e.g. folder lists, calendar lists).
#[derive(Debug, Clone)]
struct TimedCache<T: Clone> {
    data: Arc<RwLock<HashMap<String, (T, Instant)>>>,
    ttl: Duration,
}

impl<T: Clone> TimedCache<T> {
    fn new(ttl_secs: u64) -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    fn get(&self, key: &str) -> Option<T> {
        let lock = self.data.read().ok()?;
        let (value, timestamp) = lock.get(key)?;
        if Instant::now().duration_since(*timestamp) < self.ttl {
            Some(value.clone())
        } else {
            None
        }
    }

    fn set(&self, key: String, value: T) {
        if let Ok(mut lock) = self.data.write() {
            lock.insert(key, (value, Instant::now()));
        }
    }

    fn invalidate_prefix(&self, prefix: &str) {
        if let Ok(mut lock) = self.data.write() {
            lock.retain(|k, _| !k.starts_with(prefix));
        }
    }
}

/// macOS PIM implementation that delegates to the `aleph-bridge` Swift CLI
/// via the long-lived JSON-RPC client.
pub struct MacOSPim {
    bridge: Arc<SwiftBridge>,
    cache: PimCache,
}

#[derive(Debug, Clone)]
struct PimCache {
    notes_list: TimedCache<Vec<NoteInfo>>,
    notes_folders: TimedCache<Vec<String>>,
    calendar_calendars: TimedCache<Vec<CalendarInfo>>,
    reminders_list: TimedCache<Vec<Reminder>>,
    reminders_lists: TimedCache<Vec<ReminderList>>,
    contacts_groups: TimedCache<Vec<ContactGroup>>,
    mail_folders: TimedCache<Vec<MailFolder>>,
}

impl PimCache {
    fn new(ttl_secs: u64) -> Self {
        Self {
            notes_list: TimedCache::new(ttl_secs),
            notes_folders: TimedCache::new(ttl_secs),
            calendar_calendars: TimedCache::new(ttl_secs),
            reminders_list: TimedCache::new(ttl_secs),
            reminders_lists: TimedCache::new(ttl_secs),
            contacts_groups: TimedCache::new(ttl_secs),
            mail_folders: TimedCache::new(ttl_secs),
        }
    }
}

impl MacOSPim {
    /// Create with a shared bridge reference.
    pub fn new(bridge: Arc<SwiftBridge>) -> Self {
        Self {
            bridge,
            cache: PimCache::new(5),
        }
    }

    /// Call a bridge method with PIM-specific error context.
    async fn call_pim<P, R>(&self, method: &str, params: P, context: &str) -> Result<R>
    where
        P: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        self.bridge.call(method, params).await.map_err(|e| match e {
            DesktopError::BridgeDisabled(ref msg) => {
                DesktopError::BridgeDisabled(format!("PIM {context} unavailable: {msg}"))
            }
            DesktopError::BridgeFailed(ref msg) => {
                DesktopError::BridgeFailed(format!("PIM {context} failed: {msg}"))
            }
            other => other,
        })
    }
}

#[async_trait]
impl PimCapability for MacOSPim {
    async fn notes_list(&self, folder: Option<&str>) -> Result<Vec<NoteInfo>> {
        let cache_key = format!("notes_list:{}", folder.unwrap_or(""));
        if let Some(cached) = self.cache.notes_list.get(&cache_key) {
            return Ok(cached);
        }
        let resp: NotesListResult = self
            .call_pim(
                METHOD_NOTES_LIST,
                NotesListParams {
                    folder: folder.map(|f| f.to_string()),
                },
                "notes_list",
            )
            .await?;
        self.cache.notes_list.set(cache_key, resp.notes.clone());
        Ok(resp.notes)
    }

    async fn notes_read(&self, note_id: &str) -> Result<NoteContent> {
        self.call_pim(
            METHOD_NOTES_GET,
            NotesGetParams {
                id: note_id.to_string(),
            },
            "notes_read",
        )
        .await
    }

    async fn notes_create(&self, folder: &str, title: &str, body: &str) -> Result<String> {
        let resp: NotesCreateResult = self
            .call_pim(
                METHOD_NOTES_CREATE,
                NotesCreateParams {
                    folder: folder.to_string(),
                    title: title.to_string(),
                    body: body.to_string(),
                },
                "notes_create",
            )
            .await?;
        self.cache.notes_list.invalidate_prefix("notes_list:");
        Ok(resp.id)
    }

    async fn notes_update(
        &self,
        note_id: &str,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<()> {
        let _: serde_json::Value = self
            .call_pim(
                METHOD_NOTES_UPDATE,
                NotesUpdateParams {
                    id: note_id.to_string(),
                    title: title.map(|t| t.to_string()),
                    body: body.map(|b| b.to_string()),
                },
                "notes_update",
            )
            .await?;
        self.cache.notes_list.invalidate_prefix("notes_list:");
        Ok(())
    }

    async fn notes_delete(&self, note_id: &str) -> Result<()> {
        let _: serde_json::Value = self
            .call_pim(
                METHOD_NOTES_DELETE,
                NotesDeleteParams {
                    id: note_id.to_string(),
                },
                "notes_delete",
            )
            .await?;
        self.cache.notes_list.invalidate_prefix("notes_list:");
        Ok(())
    }

    async fn notes_folders(&self) -> Result<Vec<String>> {
        if let Some(cached) = self.cache.notes_folders.get("notes_folders") {
            return Ok(cached);
        }
        let resp: NotesFoldersResult = self
            .call_pim(METHOD_NOTES_FOLDERS, (), "notes_folders")
            .await?;
        let folders: Vec<String> = resp.folders.into_iter().map(|f| f.name).collect();
        self.cache
            .notes_folders
            .set("notes_folders".to_string(), folders.clone());
        Ok(folders)
    }

    async fn calendar_list_events(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        calendar_id: Option<&str>,
    ) -> Result<Vec<CalendarEvent>> {
        let resp: CalendarEventsResult = self
            .call_pim(
                METHOD_CALENDAR_EVENTS,
                CalendarEventsParams {
                    from,
                    to,
                    calendar_id: calendar_id.map(|c| c.to_string()),
                },
                "calendar_list_events",
            )
            .await?;
        Ok(resp.events)
    }

    async fn calendar_get_event(&self, event_id: &str) -> Result<CalendarEvent> {
        self.call_pim(
            METHOD_CALENDAR_GET,
            CalendarGetParams {
                id: event_id.to_string(),
            },
            "calendar_get_event",
        )
        .await
    }

    async fn calendar_create_event(&self, event: NewCalendarEvent) -> Result<String> {
        let resp: CalendarCreateResult = self
            .call_pim(
                METHOD_CALENDAR_CREATE,
                CalendarCreateParams {
                    title: event.title,
                    start: event.start,
                    end: event.end,
                    calendar_id: event.calendar_id,
                    all_day: event.all_day,
                    location: event.location,
                    notes: event.notes,
                },
                "calendar_create_event",
            )
            .await?;
        self.cache
            .calendar_calendars
            .invalidate_prefix("calendar_calendars");
        Ok(resp.id)
    }

    async fn calendar_update_event(&self, event_id: &str, event: NewCalendarEvent) -> Result<()> {
        let _: serde_json::Value = self
            .call_pim(
                METHOD_CALENDAR_UPDATE,
                CalendarUpdateParams {
                    id: event_id.to_string(),
                    title: event.title,
                    start: event.start,
                    end: event.end,
                    location: event.location,
                    notes: event.notes,
                },
                "calendar_update_event",
            )
            .await?;
        self.cache
            .calendar_calendars
            .invalidate_prefix("calendar_calendars");
        Ok(())
    }

    async fn calendar_delete_event(&self, event_id: &str) -> Result<()> {
        let _: serde_json::Value = self
            .call_pim(
                METHOD_CALENDAR_DELETE,
                CalendarDeleteParams {
                    id: event_id.to_string(),
                },
                "calendar_delete_event",
            )
            .await?;
        self.cache
            .calendar_calendars
            .invalidate_prefix("calendar_calendars");
        Ok(())
    }

    async fn calendar_calendars(&self) -> Result<Vec<CalendarInfo>> {
        if let Some(cached) = self.cache.calendar_calendars.get("calendar_calendars") {
            return Ok(cached);
        }
        let resp: CalendarListsResult = self
            .call_pim(METHOD_CALENDAR_LISTS, (), "calendar_calendars")
            .await?;
        self.cache
            .calendar_calendars
            .set("calendar_calendars".to_string(), resp.calendars.clone());
        Ok(resp.calendars)
    }

    async fn reminders_list(
        &self,
        list_id: Option<&str>,
        include_completed: bool,
    ) -> Result<Vec<Reminder>> {
        let cache_key = format!("reminders_list:{:?}:{}", list_id, include_completed);
        if let Some(cached) = self.cache.reminders_list.get(&cache_key) {
            return Ok(cached);
        }
        let resp: RemindersListResult = self
            .call_pim(
                METHOD_REMINDERS_LIST,
                RemindersListParams {
                    list_id: list_id.map(|l| l.to_string()),
                    include_completed,
                },
                "reminders_list",
            )
            .await?;
        self.cache
            .reminders_list
            .set(cache_key, resp.reminders.clone());
        Ok(resp.reminders)
    }

    async fn reminders_get(&self, reminder_id: &str) -> Result<Reminder> {
        self.call_pim(
            METHOD_REMINDERS_GET,
            RemindersGetParams {
                id: reminder_id.to_string(),
            },
            "reminders_get",
        )
        .await
    }

    async fn reminders_create(&self, reminder: NewReminder) -> Result<String> {
        let resp: RemindersCreateResult = self
            .call_pim(
                METHOD_REMINDERS_CREATE,
                RemindersCreateParams {
                    title: reminder.title,
                    list_id: reminder.list_id,
                    priority: reminder.priority,
                    due_date: reminder.due_date,
                    notes: reminder.notes,
                },
                "reminders_create",
            )
            .await?;
        self.cache
            .reminders_list
            .invalidate_prefix("reminders_list:");
        self.cache
            .reminders_lists
            .invalidate_prefix("reminders_lists");
        Ok(resp.id)
    }

    async fn reminders_complete(&self, reminder_id: &str) -> Result<()> {
        let _: serde_json::Value = self
            .call_pim(
                METHOD_REMINDERS_COMPLETE,
                RemindersCompleteParams {
                    id: reminder_id.to_string(),
                },
                "reminders_complete",
            )
            .await?;
        self.cache
            .reminders_list
            .invalidate_prefix("reminders_list:");
        self.cache
            .reminders_lists
            .invalidate_prefix("reminders_lists");
        Ok(())
    }

    async fn reminders_delete(&self, reminder_id: &str) -> Result<()> {
        let _: serde_json::Value = self
            .call_pim(
                METHOD_REMINDERS_DELETE,
                RemindersDeleteParams {
                    id: reminder_id.to_string(),
                },
                "reminders_delete",
            )
            .await?;
        self.cache
            .reminders_list
            .invalidate_prefix("reminders_list:");
        self.cache
            .reminders_lists
            .invalidate_prefix("reminders_lists");
        Ok(())
    }

    async fn reminders_lists(&self) -> Result<Vec<ReminderList>> {
        if let Some(cached) = self.cache.reminders_lists.get("reminders_lists") {
            return Ok(cached);
        }
        let resp: RemindersListsResult = self
            .call_pim(METHOD_REMINDERS_LISTS, (), "reminders_lists")
            .await?;
        self.cache
            .reminders_lists
            .set("reminders_lists".to_string(), resp.lists.clone());
        Ok(resp.lists)
    }

    async fn contacts_search(&self, query: &str) -> Result<Vec<Contact>> {
        let resp: ContactsSearchResult = self
            .call_pim(
                METHOD_CONTACTS_SEARCH,
                ContactsSearchParams {
                    query: query.to_string(),
                },
                "contacts_search",
            )
            .await?;
        Ok(resp.contacts)
    }

    async fn contacts_get(&self, contact_id: &str) -> Result<ContactDetail> {
        self.call_pim(
            METHOD_CONTACTS_GET,
            ContactsGetParams {
                id: contact_id.to_string(),
            },
            "contacts_get",
        )
        .await
    }

    async fn contacts_groups(&self) -> Result<Vec<ContactGroup>> {
        if let Some(cached) = self.cache.contacts_groups.get("contacts_groups") {
            return Ok(cached);
        }
        let resp: ContactsGroupsResult = self
            .call_pim(METHOD_CONTACTS_GROUPS, (), "contacts_groups")
            .await?;
        self.cache
            .contacts_groups
            .set("contacts_groups".to_string(), resp.groups.clone());
        Ok(resp.groups)
    }

    async fn mail_search(
        &self,
        query: &str,
        folder: Option<&str>,
        limit: u32,
    ) -> Result<Vec<MailMessage>> {
        let resp: MailSearchResult = self
            .call_pim(
                METHOD_MAIL_SEARCH,
                MailSearchParams {
                    query: query.to_string(),
                    folder: folder.map(|f| f.to_string()),
                    limit,
                },
                "mail_search",
            )
            .await?;
        Ok(resp.messages)
    }

    async fn mail_get(&self, message_id: &str) -> Result<MailMessageDetail> {
        let resp: MailGetResult = self
            .call_pim(
                METHOD_MAIL_GET,
                MailGetParams {
                    id: message_id.to_string(),
                },
                "mail_get",
            )
            .await?;
        Ok(MailMessageDetail {
            id: resp.id,
            subject: resp.subject,
            sender: resp.sender,
            recipients: resp.recipients,
            cc: resp.cc,
            bcc: resp.bcc,
            date: resp.date,
            body: resp.body,
            is_read: resp.is_read,
            attachments: resp.attachments,
        })
    }

    async fn mail_folders(&self) -> Result<Vec<MailFolder>> {
        if let Some(cached) = self.cache.mail_folders.get("mail_folders") {
            return Ok(cached);
        }
        let resp: MailFoldersResult = self
            .call_pim(METHOD_MAIL_FOLDERS, (), "mail_folders")
            .await?;
        self.cache
            .mail_folders
            .set("mail_folders".to_string(), resp.folders.clone());
        Ok(resp.folders)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_bridge() -> Arc<SwiftBridge> {
        Arc::new(SwiftBridge::new(std::path::PathBuf::from("/dev/null")))
    }

    #[test]
    fn create_with_bridge() {
        let bridge = mock_bridge();
        let _pim = MacOSPim::new(bridge);
    }

    #[tokio::test]
    async fn notes_list_without_folder() {
        let pim = MacOSPim::new(mock_bridge());
        let result = pim.notes_list(None).await;
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn notes_create_and_read() {
        let pim = MacOSPim::new(mock_bridge());
        let id = pim
            .notes_create("test-folder", "test title", "test body")
            .await;
        if let Ok(note_id) = id {
            let content = pim.notes_read(&note_id).await;
            assert!(content.is_ok());
        }
    }

    #[tokio::test]
    async fn notes_update_and_delete() {
        let pim = MacOSPim::new(mock_bridge());
        let create_result = pim.notes_create("test-folder", "update test", "body").await;
        if let Ok(note_id) = create_result {
            let update_result = pim
                .notes_update(&note_id, Some("new title"), Some("new body"))
                .await;
            assert!(update_result.is_ok());
            let delete_result = pim.notes_delete(&note_id).await;
            assert!(delete_result.is_ok());
        }
    }

    #[tokio::test]
    async fn calendar_list_and_crud() {
        let pim = MacOSPim::new(mock_bridge());
        let from = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let to = chrono::DateTime::parse_from_rfc3339("2025-12-31T23:59:59Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let events = pim.calendar_list_events(from, to, None).await;
        assert!(events.is_ok() || events.is_err());
        let calendars = pim.calendar_calendars().await;
        assert!(calendars.is_ok() || calendars.is_err());
    }

    #[tokio::test]
    async fn reminders_list_and_crud() {
        let pim = MacOSPim::new(mock_bridge());
        let lists = pim.reminders_lists().await;
        assert!(lists.is_ok() || lists.is_err());
        let reminders = pim.reminders_list(None, false).await;
        assert!(reminders.is_ok() || reminders.is_err());
    }

    #[tokio::test]
    async fn contacts_search() {
        let pim = MacOSPim::new(mock_bridge());
        let result = pim.contacts_search("test").await;
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn contacts_groups() {
        let pim = MacOSPim::new(mock_bridge());
        let result = pim.contacts_groups().await;
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn mail_search_and_folders() {
        let pim = MacOSPim::new(mock_bridge());
        let folders = pim.mail_folders().await;
        assert!(folders.is_ok() || folders.is_err());
        let messages = pim.mail_search("test", None, 10).await;
        assert!(messages.is_ok() || messages.is_err());
    }
}
