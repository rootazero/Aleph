import EventKit
import Foundation

// MARK: - Reminders (EventKit)

private let reminderStore = EKEventStore()

/// Request reminders access synchronously using a semaphore.
///
/// Safe only off the main thread — the RPC entry point parks the main thread on
/// a semaphore, and PIM calls run on the serial PIM queue.
private func requireRemindersAccess() throws {
    let semaphore = DispatchSemaphore(value: 0)
    var granted = false

    if #available(macOS 14.0, *) {
        reminderStore.requestFullAccessToReminders { g, error in
            granted = g
            if let error = error {
                fputs("Reminders access error: \(error.localizedDescription)\n", stderr)
            }
            semaphore.signal()
        }
    } else {
        reminderStore.requestAccess(to: .reminder) { g, error in
            granted = g
            if let error = error {
                fputs("Reminders access error: \(error.localizedDescription)\n", stderr)
            }
            semaphore.signal()
        }
    }
    semaphore.wait()
    guard granted else {
        throw RpcError(
            code: -32001,
            message: "permission denied: reminders",
            data: try encodeCodable(Perm.guide(.reminders))
        )
    }
}

private let remFmt = iso8601Formatter()

private func reminderToWire(_ reminder: EKReminder) -> Reminder {
    var dueDate: String?
    if let components = reminder.dueDateComponents,
       let date = Foundation.Calendar.current.date(from: components) {
        dueDate = remFmt.string(from: date)
    }
    let notes = reminder.notes.flatMap { $0.isEmpty ? nil : $0 }
    return Reminder(
        id: reminder.calendarItemIdentifier,
        title: reminder.title ?? "",
        list_id: reminder.calendar.calendarIdentifier,
        completed: reminder.isCompleted,
        due_date: dueDate,
        priority: reminder.priority,
        notes: notes
    )
}

/// Fetch reminders synchronously using a semaphore.
private func fetchReminders(matching predicate: NSPredicate) -> [EKReminder] {
    let semaphore = DispatchSemaphore(value: 0)
    var results: [EKReminder] = []
    reminderStore.fetchReminders(matching: predicate) { reminders in
        results = reminders ?? []
        semaphore.signal()
    }
    semaphore.wait()
    return results
}

/// Apple Reminders operations. Synchronous and blocking; call from the serial PIM
/// queue (see `PimHandlers.swift`).
enum RemindersCommands {
    static func list(listId: String?, includeCompleted: Bool) throws -> RemindersListResult {
        try requireRemindersAccess()

        var calendars: [EKCalendar]?
        if let lid = listId, !lid.isEmpty {
            guard let cal = reminderStore.calendar(withIdentifier: lid) else {
                throw RpcError(
                    code: -32602,
                    message: "pim.reminders.list: reminder list not found: \(lid)",
                    data: nil
                )
            }
            calendars = [cal]
        }

        let predicate: NSPredicate
        if includeCompleted {
            predicate = reminderStore.predicateForReminders(in: calendars)
        } else {
            predicate = reminderStore.predicateForIncompleteReminders(
                withDueDateStarting: nil, ending: nil, calendars: calendars
            )
        }

        let reminders = fetchReminders(matching: predicate)
        return RemindersListResult(reminders: reminders.map { reminderToWire($0) })
    }

    static func get(id: String) throws -> Reminder {
        try requireRemindersAccess()

        guard let reminder = reminderStore.calendarItem(withIdentifier: id) as? EKReminder else {
            throw RpcError(
                code: -32602,
                message: "pim.reminders.get: reminder not found: \(id)",
                data: nil
            )
        }
        return reminderToWire(reminder)
    }

    static func create(
        title: String,
        listId: String,
        priority: Int,
        dueDate: Date?,
        notes: String?
    ) throws -> RemindersCreateResult {
        try requireRemindersAccess()

        let reminder = EKReminder(eventStore: reminderStore)
        reminder.title = title

        // An empty or unknown list_id means "wherever new reminders go".
        if !listId.isEmpty, let cal = reminderStore.calendar(withIdentifier: listId) {
            reminder.calendar = cal
        } else if let fallback = reminderStore.defaultCalendarForNewReminders() {
            reminder.calendar = fallback
        } else {
            throw RpcError(
                code: -32003,
                message: "pim.reminders.create: no writable reminder list available",
                data: nil
            )
        }

        if let dueDate = dueDate {
            reminder.dueDateComponents = Foundation.Calendar.current.dateComponents(
                [.year, .month, .day, .hour, .minute, .second], from: dueDate
            )
        }
        reminder.priority = priority
        reminder.notes = notes

        do {
            try reminderStore.save(reminder, commit: true)
        } catch {
            throw RpcError(
                code: -32003,
                message: "pim.reminders.create: \(error.localizedDescription)",
                data: nil
            )
        }

        return RemindersCreateResult(id: reminder.calendarItemIdentifier)
    }

    static func complete(id: String) throws {
        try requireRemindersAccess()

        guard let reminder = reminderStore.calendarItem(withIdentifier: id) as? EKReminder else {
            throw RpcError(
                code: -32602,
                message: "pim.reminders.complete: reminder not found: \(id)",
                data: nil
            )
        }

        reminder.isCompleted = true
        reminder.completionDate = Date()

        do {
            try reminderStore.save(reminder, commit: true)
        } catch {
            throw RpcError(
                code: -32003,
                message: "pim.reminders.complete: \(error.localizedDescription)",
                data: nil
            )
        }
    }

    static func delete(id: String) throws {
        try requireRemindersAccess()

        guard let reminder = reminderStore.calendarItem(withIdentifier: id) as? EKReminder else {
            throw RpcError(
                code: -32602,
                message: "pim.reminders.delete: reminder not found: \(id)",
                data: nil
            )
        }

        do {
            try reminderStore.remove(reminder, commit: true)
        } catch {
            throw RpcError(
                code: -32003,
                message: "pim.reminders.delete: \(error.localizedDescription)",
                data: nil
            )
        }
    }

    static func lists() throws -> RemindersListsResult {
        try requireRemindersAccess()

        // `count` is the incomplete-reminder count, one fetch per list.
        let lists = reminderStore.calendars(for: .reminder).map { cal -> ReminderList in
            let predicate = reminderStore.predicateForIncompleteReminders(
                withDueDateStarting: nil, ending: nil, calendars: [cal]
            )
            return ReminderList(
                id: cal.calendarIdentifier,
                title: cal.title,
                count: fetchReminders(matching: predicate).count
            )
        }
        return RemindersListsResult(lists: lists)
    }
}
