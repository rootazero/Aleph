import ArgumentParser
import EventKit
import Foundation

// MARK: - Reminders (EventKit)

private let reminderStore = EKEventStore()

/// Request reminders access synchronously using a semaphore.
private func requestRemindersAccess() {
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
    if !granted {
        printError("Reminders access denied. Grant access in System Settings > Privacy & Security > Reminders.")
    }
}

private let remFmt = iso8601Formatter()

private func reminderToDict(_ reminder: EKReminder) -> [String: Any] {
    var dict: [String: Any] = [
        "id": reminder.calendarItemIdentifier,
        "title": reminder.title ?? "",
        "list_id": reminder.calendar.calendarIdentifier,
        "completed": reminder.isCompleted,
        "priority": reminder.priority,
    ]
    if let dueDateComponents = reminder.dueDateComponents,
       let dueDate = Foundation.Calendar.current.date(from: dueDateComponents) {
        dict["due_date"] = remFmt.string(from: dueDate)
    }
    if let notes = reminder.notes, !notes.isEmpty {
        dict["notes"] = notes
    }
    return dict
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

extension AlephBridge {
    struct Reminders: ParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Apple Reminders operations",
            subcommands: [List.self, Get.self, Create.self, Complete.self, Delete.self, Lists.self]
        )

        struct List: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List reminders")

            @Option(name: .long, help: "Reminder list ID")
            var listId: String?

            @Option(name: .long, help: "Include completed reminders (true/false)")
            var includeCompleted: String?

            func run() {
                requestRemindersAccess()

                var calendars: [EKCalendar]? = nil
                if let lid = listId {
                    if let cal = reminderStore.calendar(withIdentifier: lid) {
                        calendars = [cal]
                    } else {
                        printError("Reminder list not found: \(lid)")
                    }
                }

                let predicate: NSPredicate
                if includeCompleted == "true" {
                    predicate = reminderStore.predicateForReminders(in: calendars)
                } else {
                    predicate = reminderStore.predicateForIncompleteReminders(
                        withDueDateStarting: nil, ending: nil, calendars: calendars
                    )
                }

                let reminders = fetchReminders(matching: predicate)
                let results = reminders.map { reminderToDict($0) }
                printJSON(["reminders": results])
            }
        }

        struct Get: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Get a reminder by ID")

            @Option(name: .long, help: "Reminder ID")
            var id: String

            func run() {
                requestRemindersAccess()

                guard let item = reminderStore.calendarItem(withIdentifier: id) as? EKReminder else {
                    printError("Reminder not found: \(id)")
                }
                printJSON(reminderToDict(item))
            }
        }

        struct Create: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Create a reminder")

            @Option(name: .long, help: "Reminder title")
            var title: String = ""

            @Option(name: .long, help: "Reminder list ID")
            var listId: String?

            @Option(name: .long, help: "Due date (ISO 8601)")
            var dueDate: String?

            @Option(name: .long, help: "Priority (0=none, 1=high, 5=medium, 9=low)")
            var priority: Int?

            @Option(name: .long, help: "Notes")
            var notes: String?

            func run() {
                requestRemindersAccess()

                let reminder = EKReminder(eventStore: reminderStore)
                reminder.title = title

                if let lid = listId, let cal = reminderStore.calendar(withIdentifier: lid) {
                    reminder.calendar = cal
                } else {
                    reminder.calendar = reminderStore.defaultCalendarForNewReminders()
                }

                if let ds = dueDate, let d = parseISO8601(ds) {
                    reminder.dueDateComponents = Foundation.Calendar.current.dateComponents(
                        [.year, .month, .day, .hour, .minute, .second], from: d
                    )
                }

                if let p = priority {
                    reminder.priority = p
                }

                reminder.notes = notes

                do {
                    try reminderStore.save(reminder, commit: true)
                } catch {
                    printError("Failed to create reminder: \(error.localizedDescription)")
                }

                printJSON(reminderToDict(reminder))
            }
        }

        struct Complete: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Mark a reminder as completed or not")

            @Option(name: .long, help: "Reminder ID")
            var id: String

            @Option(name: .long, help: "Mark as completed (true/false, default: true)")
            var completed: String?

            func run() {
                requestRemindersAccess()

                guard let reminder = reminderStore.calendarItem(withIdentifier: id) as? EKReminder else {
                    printError("Reminder not found: \(id)")
                }

                let isCompleted = completed != "false" // default true
                reminder.isCompleted = isCompleted
                if isCompleted {
                    reminder.completionDate = Date()
                } else {
                    reminder.completionDate = nil
                }

                do {
                    try reminderStore.save(reminder, commit: true)
                } catch {
                    printError("Failed to update reminder: \(error.localizedDescription)")
                }

                printJSON(reminderToDict(reminder))
            }
        }

        struct Delete: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Delete a reminder")

            @Option(name: .long, help: "Reminder ID")
            var id: String

            func run() {
                requestRemindersAccess()

                guard let reminder = reminderStore.calendarItem(withIdentifier: id) as? EKReminder else {
                    printError("Reminder not found: \(id)")
                }

                do {
                    try reminderStore.remove(reminder, commit: true)
                } catch {
                    printError("Failed to delete reminder: \(error.localizedDescription)")
                }

                printJSON(["deleted": true, "id": id])
            }
        }

        struct Lists: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List all reminder lists")

            func run() {
                requestRemindersAccess()

                let calendars = reminderStore.calendars(for: .reminder)

                // Count incomplete reminders per list
                let results: [[String: Any]] = calendars.map { cal in
                    let pred = reminderStore.predicateForIncompleteReminders(
                        withDueDateStarting: nil, ending: nil, calendars: [cal]
                    )
                    let count = fetchReminders(matching: pred).count

                    return [
                        "id": cal.calendarIdentifier,
                        "title": cal.title,
                        "count": count,
                    ]
                }
                printJSON(["lists": results])
            }
        }
    }
}
