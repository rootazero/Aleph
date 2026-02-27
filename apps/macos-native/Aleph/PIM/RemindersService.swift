import EventKit
import Foundation
import os

/// Service for interacting with macOS Reminders via EventKit.
///
/// Provides CRUD operations on reminders exposed as Bridge handlers.
/// Each method accepts `[String: AnyCodable]` params and returns
/// `Result<AnyCodable, BridgeServer.HandlerError>`, matching the
/// `BridgeServer.Handler` signature.
final class RemindersService {

    // MARK: - Singleton

    static let shared = RemindersService()

    // MARK: - Properties

    private let store = EKEventStore()
    private let logger = Logger(subsystem: "com.aleph.app", category: "RemindersService")
    private let dateFormatter: ISO8601DateFormatter = {
        let fmt = ISO8601DateFormatter()
        fmt.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fmt
    }()

    /// Fallback formatter without fractional seconds for broader compatibility.
    private let dateFormatterNoFraction: ISO8601DateFormatter = {
        let fmt = ISO8601DateFormatter()
        fmt.formatOptions = [.withInternetDateTime]
        return fmt
    }()

    private init() {}

    // MARK: - Access

    /// Request full reminders access from the user.
    ///
    /// On macOS 14+ uses `requestFullAccessToReminders()`.
    /// On older versions falls back to `requestAccess(to: .reminder)`.
    /// Blocks the calling thread until the user responds.
    func ensureAccess() -> Result<Void, BridgeServer.HandlerError> {
        let semaphore = DispatchSemaphore(value: 0)
        var granted = false
        var accessError: Error?

        if #available(macOS 14.0, *) {
            store.requestFullAccessToReminders { ok, error in
                granted = ok
                accessError = error
                semaphore.signal()
            }
        } else {
            store.requestAccess(to: .reminder) { ok, error in
                granted = ok
                accessError = error
                semaphore.signal()
            }
        }

        semaphore.wait()

        if let error = accessError {
            logger.error("Reminders access error: \(error.localizedDescription)")
            return .failure(.init(
                code: PIMErrorCode.permissionDenied,
                message: "Reminders access denied: \(error.localizedDescription)"
            ))
        }

        guard granted else {
            return .failure(.init(
                code: PIMErrorCode.permissionDenied,
                message: "Reminders access denied. Enable in System Settings > Privacy & Security > Reminders."
            ))
        }

        return .success(())
    }

    // MARK: - List Reminders

    /// Query reminders, optionally filtered by list.
    ///
    /// Params:
    /// - `list_id` (optional): Filter to a specific reminder list.
    /// - `include_completed` (optional): Include completed reminders. Default false.
    ///
    /// Returns: `{ "reminders": [{ "id", "title", "completed", ... }] }`
    func listReminders(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        let includeCompleted = params["include_completed"]?.boolValue ?? false

        var calendars: [EKCalendar]? = nil
        if let listId = params["list_id"]?.stringValue {
            if let cal = store.calendar(withIdentifier: listId) {
                calendars = [cal]
            } else {
                return .failure(.init(
                    code: PIMErrorCode.notFound,
                    message: "Reminder list not found: \(listId)"
                ))
            }
        }

        let predicate: NSPredicate
        if includeCompleted {
            // Fetch both incomplete and completed reminders
            let incompletePred = store.predicateForIncompleteReminders(
                withDueDateStarting: nil, ending: nil, calendars: calendars
            )
            let completedPred = store.predicateForCompletedReminders(
                withCompletionDateStarting: nil, ending: nil, calendars: calendars
            )
            predicate = NSCompoundPredicate(orPredicateWithSubpredicates: [incompletePred, completedPred])
        } else {
            predicate = store.predicateForIncompleteReminders(
                withDueDateStarting: nil, ending: nil, calendars: calendars
            )
        }

        // fetchReminders(matching:) is asynchronous; use semaphore for sync bridge handler.
        let semaphore = DispatchSemaphore(value: 0)
        var fetchedReminders: [EKReminder]?
        store.fetchReminders(matching: predicate) { reminders in
            fetchedReminders = reminders
            semaphore.signal()
        }
        semaphore.wait()

        let reminders = fetchedReminders ?? []
        let reminderDicts: [AnyCodable] = reminders.map { AnyCodable(reminderToDict($0)) }
        return .success(AnyCodable(["reminders": AnyCodable(reminderDicts)]))
    }

    // MARK: - Get Reminder

    /// Get a single reminder by identifier.
    ///
    /// Params:
    /// - `id` (required): Reminder identifier string.
    ///
    /// Returns: `{ "reminder": { "id", "title", ... } }`
    func getReminder(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let reminderId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        guard let reminder = store.calendarItem(withIdentifier: reminderId) as? EKReminder else {
            return .failure(.init(
                code: PIMErrorCode.notFound,
                message: "Reminder not found: \(reminderId)"
            ))
        }

        return .success(AnyCodable(["reminder": AnyCodable(reminderToDict(reminder))]))
    }

    // MARK: - Create Reminder

    /// Create a new reminder.
    ///
    /// Params:
    /// - `title` (required): Reminder title.
    /// - `list_id` (optional): Target reminder list identifier.
    /// - `due_date` (optional): ISO 8601 due date.
    /// - `priority` (optional): Priority (0 = none, 1 = high, 5 = medium, 9 = low).
    /// - `notes` (optional): Notes/description.
    ///
    /// Returns: `{ "reminder": { ... } }`
    func createReminder(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let title = params["title"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: title (string)"
            ))
        }

        let reminder = EKReminder(eventStore: store)
        reminder.title = title

        // Set reminder list
        if let listId = params["list_id"]?.stringValue {
            if let cal = store.calendar(withIdentifier: listId) {
                reminder.calendar = cal
            } else {
                return .failure(.init(
                    code: PIMErrorCode.notFound,
                    message: "Reminder list not found: \(listId)"
                ))
            }
        } else {
            reminder.calendar = store.defaultCalendarForNewReminders()
        }

        // Set due date
        if let dueDateStr = params["due_date"]?.stringValue {
            if let dueDate = parseDate(dueDateStr) {
                let components = Calendar.current.dateComponents(
                    [.year, .month, .day, .hour, .minute, .second],
                    from: dueDate
                )
                reminder.dueDateComponents = components
            } else {
                return .failure(.init(
                    code: PIMErrorCode.validationFailed,
                    message: "Invalid 'due_date' param (ISO 8601 date string)"
                ))
            }
        }

        // Set priority
        if let priority = params["priority"]?.intValue {
            reminder.priority = priority
        }

        // Set notes
        if let notes = params["notes"]?.stringValue {
            reminder.notes = notes
        }

        do {
            try store.save(reminder, commit: true)
        } catch {
            logger.error("Failed to create reminder: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to create reminder: \(error.localizedDescription)"
            ))
        }

        return .success(AnyCodable(["reminder": AnyCodable(reminderToDict(reminder))]))
    }

    // MARK: - Complete Reminder

    /// Mark a reminder as completed (or uncomplete it).
    ///
    /// Params:
    /// - `id` (required): Reminder identifier.
    /// - `completed` (optional): Boolean, default true.
    ///
    /// Returns: `{ "reminder": { ... } }`
    func completeReminder(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let reminderId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        guard let reminder = store.calendarItem(withIdentifier: reminderId) as? EKReminder else {
            return .failure(.init(
                code: PIMErrorCode.notFound,
                message: "Reminder not found: \(reminderId)"
            ))
        }

        let completed = params["completed"]?.boolValue ?? true
        reminder.isCompleted = completed

        do {
            try store.save(reminder, commit: true)
        } catch {
            logger.error("Failed to complete reminder: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to update reminder: \(error.localizedDescription)"
            ))
        }

        return .success(AnyCodable(["reminder": AnyCodable(reminderToDict(reminder))]))
    }

    // MARK: - Delete Reminder

    /// Delete a reminder.
    ///
    /// Params:
    /// - `id` (required): Reminder identifier.
    ///
    /// Returns: `{ "deleted": true }`
    func deleteReminder(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let reminderId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        guard let reminder = store.calendarItem(withIdentifier: reminderId) as? EKReminder else {
            return .failure(.init(
                code: PIMErrorCode.notFound,
                message: "Reminder not found: \(reminderId)"
            ))
        }

        do {
            try store.remove(reminder, commit: true)
        } catch {
            logger.error("Failed to delete reminder: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to delete reminder: \(error.localizedDescription)"
            ))
        }

        return .success(AnyCodable(["deleted": AnyCodable(true)]))
    }

    // MARK: - List Reminder Lists

    /// List all available reminder lists (calendars for reminders).
    ///
    /// No params required.
    ///
    /// Returns: `{ "lists": [{ "id", "title", "color", "allows_modify" }] }`
    func listReminderLists(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        let calendars = store.calendars(for: .reminder)
        let listDicts: [AnyCodable] = calendars.map { cal in
            AnyCodable([
                "id": AnyCodable(cal.calendarIdentifier),
                "title": AnyCodable(cal.title),
                "color": AnyCodable(colorHexString(cal.cgColor)),
                "allows_modify": AnyCodable(cal.allowsContentModifications),
            ])
        }

        return .success(AnyCodable(["lists": AnyCodable(listDicts)]))
    }

    // MARK: - Helpers

    /// Parse an ISO 8601 date string, trying fractional seconds first then without.
    private func parseDate(_ string: String) -> Date? {
        if let date = dateFormatter.date(from: string) {
            return date
        }
        return dateFormatterNoFraction.date(from: string)
    }

    /// Format a date to ISO 8601 string.
    private func formatDate(_ date: Date) -> String {
        dateFormatter.string(from: date)
    }

    /// Convert an EKReminder to a dictionary suitable for JSON-RPC response.
    private func reminderToDict(_ reminder: EKReminder) -> [String: AnyCodable] {
        var dict: [String: AnyCodable] = [
            "id": AnyCodable(reminder.calendarItemIdentifier),
            "title": AnyCodable(reminder.title ?? ""),
            "completed": AnyCodable(reminder.isCompleted),
            "priority": AnyCodable(reminder.priority),
        ]

        // Due date
        if let dueDateComponents = reminder.dueDateComponents,
           let dueDate = Calendar.current.date(from: dueDateComponents) {
            dict["due_date"] = AnyCodable(formatDate(dueDate))
        } else {
            dict["due_date"] = AnyCodable(NSNull())
        }

        // Notes
        if let notes = reminder.notes, !notes.isEmpty {
            dict["notes"] = AnyCodable(notes)
        } else {
            dict["notes"] = AnyCodable(NSNull())
        }

        // List (calendar) info
        if let calendar = reminder.calendar {
            dict["list"] = AnyCodable(calendar.title)
            dict["list_id"] = AnyCodable(calendar.calendarIdentifier)
        }

        return dict
    }

    /// Convert a CGColor to a hex string (e.g. "#FF5733").
    private func colorHexString(_ cgColor: CGColor?) -> String {
        guard let color = cgColor,
              let components = color.components,
              components.count >= 3 else {
            return "#000000"
        }
        let r = Int(components[0] * 255.0)
        let g = Int(components[1] * 255.0)
        let b = Int(components[2] * 255.0)
        return String(format: "#%02X%02X%02X", r, g, b)
    }
}
