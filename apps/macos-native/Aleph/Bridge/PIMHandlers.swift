import Foundation

/// Registers all PIM (Personal Information Management) handlers on the BridgeServer.
///
/// This extension keeps PIM handler registration centralized so that
/// `AppDelegate` only needs to call `registerPIMHandlers()`.
///
/// Registers 24 methods across 4 domains:
/// - Calendar (6): list, get, create, update, delete, calendars
/// - Reminders (6): list, get, create, complete, delete, lists
/// - Notes (6): list, get, create, update, delete, folders
/// - Contacts (6): search, get, create, update, delete, groups
extension BridgeServer {

    /// Register all PIM capability handlers.
    func registerPIMHandlers() {
        registerCalendarHandlers()
        registerRemindersHandlers()
        registerNotesHandlers()
        registerContactsHandlers()
    }

    // MARK: - Calendar Handlers

    private func registerCalendarHandlers() {
        let calendar = CalendarService.shared

        register(method: "pim.calendar.list") { params in
            calendar.listEvents(params: params)
        }

        register(method: "pim.calendar.get") { params in
            calendar.getEvent(params: params)
        }

        register(method: "pim.calendar.create") { params in
            calendar.createEvent(params: params)
        }

        register(method: "pim.calendar.update") { params in
            calendar.updateEvent(params: params)
        }

        register(method: "pim.calendar.delete") { params in
            calendar.deleteEvent(params: params)
        }

        register(method: "pim.calendar.calendars") { params in
            calendar.listCalendars(params: params)
        }
    }

    // MARK: - Reminders Handlers

    private func registerRemindersHandlers() {
        let reminders = RemindersService.shared

        register(method: "pim.reminders.list") { params in
            reminders.listReminders(params: params)
        }

        register(method: "pim.reminders.get") { params in
            reminders.getReminder(params: params)
        }

        register(method: "pim.reminders.create") { params in
            reminders.createReminder(params: params)
        }

        register(method: "pim.reminders.complete") { params in
            reminders.completeReminder(params: params)
        }

        register(method: "pim.reminders.delete") { params in
            reminders.deleteReminder(params: params)
        }

        register(method: "pim.reminders.lists") { params in
            reminders.listReminderLists(params: params)
        }
    }

    // MARK: - Notes Handlers

    private func registerNotesHandlers() {
        let notes = NotesService.shared

        register(method: "pim.notes.list") { params in
            notes.listNotes(params: params)
        }

        register(method: "pim.notes.get") { params in
            notes.getNote(params: params)
        }

        register(method: "pim.notes.create") { params in
            notes.createNote(params: params)
        }

        register(method: "pim.notes.update") { params in
            notes.updateNote(params: params)
        }

        register(method: "pim.notes.delete") { params in
            notes.deleteNote(params: params)
        }

        register(method: "pim.notes.folders") { params in
            notes.listFolders(params: params)
        }
    }

    // MARK: - Contacts Handlers

    private func registerContactsHandlers() {
        let contacts = ContactsService.shared

        register(method: "pim.contacts.search") { params in
            contacts.searchContacts(params: params)
        }

        register(method: "pim.contacts.get") { params in
            contacts.getContact(params: params)
        }

        register(method: "pim.contacts.create") { params in
            contacts.createContact(params: params)
        }

        register(method: "pim.contacts.update") { params in
            contacts.updateContact(params: params)
        }

        register(method: "pim.contacts.delete") { params in
            contacts.deleteContact(params: params)
        }

        register(method: "pim.contacts.groups") { params in
            contacts.listGroups(params: params)
        }
    }
}
