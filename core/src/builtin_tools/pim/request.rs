//! PIM request builder — converts PimArgs into DesktopRequest.

use crate::desktop::DesktopRequest;

use super::args::PimArgs;

/// Build a `DesktopRequest` from PIM tool args, returning an error message string if invalid.
pub fn build_pim_request(args: &PimArgs) -> std::result::Result<DesktopRequest, String> {
    let req = match args.action.as_str() {
        // -- Calendar --------------------------------------------------------
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

        // -- Reminders -------------------------------------------------------
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

        // -- Notes -----------------------------------------------------------
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

        // -- Contacts --------------------------------------------------------
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
