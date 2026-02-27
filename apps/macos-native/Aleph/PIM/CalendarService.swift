import EventKit
import Foundation
import os

/// Service for interacting with macOS Calendar via EventKit.
///
/// Provides CRUD operations on calendar events exposed as Bridge handlers.
/// Each method accepts `[String: AnyCodable]` params and returns
/// `Result<AnyCodable, BridgeServer.HandlerError>`, matching the
/// `BridgeServer.Handler` signature.
final class CalendarService {

    // MARK: - Singleton

    static let shared = CalendarService()

    // MARK: - Properties

    private let store = EKEventStore()
    private let logger = Logger(subsystem: "com.aleph.app", category: "CalendarService")
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

    /// Request full calendar access from the user.
    ///
    /// On macOS 14+ uses `requestFullAccessToEvents()`.
    /// On older versions falls back to `requestAccess(to: .event)`.
    /// Blocks the calling thread until the user responds.
    func ensureAccess() -> Result<Void, BridgeServer.HandlerError> {
        let semaphore = DispatchSemaphore(value: 0)
        var granted = false
        var accessError: Error?

        if #available(macOS 14.0, *) {
            store.requestFullAccessToEvents { ok, error in
                granted = ok
                accessError = error
                semaphore.signal()
            }
        } else {
            store.requestAccess(to: .event) { ok, error in
                granted = ok
                accessError = error
                semaphore.signal()
            }
        }

        semaphore.wait()

        if let error = accessError {
            logger.error("Calendar access error: \(error.localizedDescription)")
            return .failure(.init(
                code: PIMErrorCode.permissionDenied,
                message: "Calendar access denied: \(error.localizedDescription)"
            ))
        }

        guard granted else {
            return .failure(.init(
                code: PIMErrorCode.permissionDenied,
                message: "Calendar access denied. Enable in System Settings > Privacy & Security > Calendars."
            ))
        }

        return .success(())
    }

    // MARK: - List Events

    /// Query events within a date range.
    ///
    /// Params:
    /// - `from` (required): ISO 8601 start date.
    /// - `to` (required): ISO 8601 end date.
    /// - `calendar_id` (optional): Filter to a specific calendar.
    ///
    /// Returns: `{ "events": [{ "id", "title", "start", "end", ... }] }`
    func listEvents(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let fromStr = params["from"]?.stringValue,
              let startDate = parseDate(fromStr) else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing or invalid 'from' param (ISO 8601 date string)"
            ))
        }

        guard let toStr = params["to"]?.stringValue,
              let endDate = parseDate(toStr) else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing or invalid 'to' param (ISO 8601 date string)"
            ))
        }

        var calendars: [EKCalendar]? = nil
        if let calId = params["calendar_id"]?.stringValue {
            if let cal = store.calendar(withIdentifier: calId) {
                calendars = [cal]
            } else {
                return .failure(.init(
                    code: PIMErrorCode.notFound,
                    message: "Calendar not found: \(calId)"
                ))
            }
        }

        let predicate = store.predicateForEvents(withStart: startDate, end: endDate, calendars: calendars)
        let events = store.events(matching: predicate)

        let eventDicts: [AnyCodable] = events.map { AnyCodable(eventToDict($0)) }
        return .success(AnyCodable(["events": AnyCodable(eventDicts)]))
    }

    // MARK: - Get Event

    /// Get a single event by identifier.
    ///
    /// Params:
    /// - `id` (required): Event identifier string.
    ///
    /// Returns: `{ "event": { "id", "title", ... } }`
    func getEvent(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let eventId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        guard let event = store.event(withIdentifier: eventId) else {
            return .failure(.init(
                code: PIMErrorCode.notFound,
                message: "Event not found: \(eventId)"
            ))
        }

        return .success(AnyCodable(["event": AnyCodable(eventToDict(event))]))
    }

    // MARK: - Create Event

    /// Create a new calendar event.
    ///
    /// Params:
    /// - `title` (required): Event title.
    /// - `start` (required): ISO 8601 start date.
    /// - `end` (required): ISO 8601 end date.
    /// - `calendar_id` (optional): Target calendar identifier.
    /// - `location` (optional): Location string.
    /// - `notes` (optional): Notes/description.
    /// - `all_day` (optional): Boolean, default false.
    ///
    /// Returns: `{ "event": { ... } }`
    func createEvent(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
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

        guard let startStr = params["start"]?.stringValue,
              let startDate = parseDate(startStr) else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing or invalid 'start' param (ISO 8601 date string)"
            ))
        }

        guard let endStr = params["end"]?.stringValue,
              let endDate = parseDate(endStr) else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing or invalid 'end' param (ISO 8601 date string)"
            ))
        }

        let event = EKEvent(eventStore: store)
        event.title = title
        event.startDate = startDate
        event.endDate = endDate
        event.isAllDay = params["all_day"]?.boolValue ?? false

        if let location = params["location"]?.stringValue {
            event.location = location
        }
        if let notes = params["notes"]?.stringValue {
            event.notes = notes
        }

        // Set calendar (default to the user's default calendar for events)
        if let calId = params["calendar_id"]?.stringValue {
            if let cal = store.calendar(withIdentifier: calId) {
                event.calendar = cal
            } else {
                return .failure(.init(
                    code: PIMErrorCode.notFound,
                    message: "Calendar not found: \(calId)"
                ))
            }
        } else {
            event.calendar = store.defaultCalendarForNewEvents
        }

        do {
            try store.save(event, span: .thisEvent)
        } catch {
            logger.error("Failed to create event: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to create event: \(error.localizedDescription)"
            ))
        }

        return .success(AnyCodable(["event": AnyCodable(eventToDict(event))]))
    }

    // MARK: - Update Event

    /// Update an existing calendar event.
    ///
    /// Params:
    /// - `id` (required): Event identifier.
    /// - `title` (optional): New title.
    /// - `start` (optional): New ISO 8601 start date.
    /// - `end` (optional): New ISO 8601 end date.
    /// - `location` (optional): New location.
    /// - `notes` (optional): New notes.
    /// - `all_day` (optional): New all-day flag.
    /// - `calendar_id` (optional): Move to a different calendar.
    ///
    /// Returns: `{ "event": { ... } }`
    func updateEvent(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let eventId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        guard let event = store.event(withIdentifier: eventId) else {
            return .failure(.init(
                code: PIMErrorCode.notFound,
                message: "Event not found: \(eventId)"
            ))
        }

        // Apply optional updates
        if let title = params["title"]?.stringValue {
            event.title = title
        }
        if let startStr = params["start"]?.stringValue {
            if let startDate = parseDate(startStr) {
                event.startDate = startDate
            } else {
                return .failure(.init(
                    code: PIMErrorCode.validationFailed,
                    message: "Invalid 'start' param (ISO 8601 date string)"
                ))
            }
        }
        if let endStr = params["end"]?.stringValue {
            if let endDate = parseDate(endStr) {
                event.endDate = endDate
            } else {
                return .failure(.init(
                    code: PIMErrorCode.validationFailed,
                    message: "Invalid 'end' param (ISO 8601 date string)"
                ))
            }
        }
        if let location = params["location"]?.stringValue {
            event.location = location
        }
        if let notes = params["notes"]?.stringValue {
            event.notes = notes
        }
        if let allDay = params["all_day"]?.boolValue {
            event.isAllDay = allDay
        }
        if let calId = params["calendar_id"]?.stringValue {
            if let cal = store.calendar(withIdentifier: calId) {
                event.calendar = cal
            } else {
                return .failure(.init(
                    code: PIMErrorCode.notFound,
                    message: "Calendar not found: \(calId)"
                ))
            }
        }

        do {
            try store.save(event, span: .thisEvent)
        } catch {
            logger.error("Failed to update event: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to update event: \(error.localizedDescription)"
            ))
        }

        return .success(AnyCodable(["event": AnyCodable(eventToDict(event))]))
    }

    // MARK: - Delete Event

    /// Delete a calendar event.
    ///
    /// Params:
    /// - `id` (required): Event identifier.
    ///
    /// Returns: `{ "deleted": true }`
    func deleteEvent(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let eventId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        guard let event = store.event(withIdentifier: eventId) else {
            return .failure(.init(
                code: PIMErrorCode.notFound,
                message: "Event not found: \(eventId)"
            ))
        }

        do {
            try store.remove(event, span: .thisEvent)
        } catch {
            logger.error("Failed to delete event: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to delete event: \(error.localizedDescription)"
            ))
        }

        return .success(AnyCodable(["deleted": AnyCodable(true)]))
    }

    // MARK: - List Calendars

    /// List all available calendars for events.
    ///
    /// No params required.
    ///
    /// Returns: `{ "calendars": [{ "id", "title", "type", "allows_modify" }] }`
    func listCalendars(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        let calendars = store.calendars(for: .event)
        let calDicts: [AnyCodable] = calendars.map { cal in
            AnyCodable([
                "id": AnyCodable(cal.calendarIdentifier),
                "title": AnyCodable(cal.title),
                "type": AnyCodable(calendarTypeName(cal.type)),
                "allows_modify": AnyCodable(cal.allowsContentModifications),
            ])
        }

        return .success(AnyCodable(["calendars": AnyCodable(calDicts)]))
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

    /// Convert an EKEvent to a dictionary suitable for JSON-RPC response.
    private func eventToDict(_ event: EKEvent) -> [String: AnyCodable] {
        var dict: [String: AnyCodable] = [
            "id": AnyCodable(event.eventIdentifier ?? ""),
            "title": AnyCodable(event.title ?? ""),
            "start": AnyCodable(formatDate(event.startDate)),
            "end": AnyCodable(formatDate(event.endDate)),
            "all_day": AnyCodable(event.isAllDay),
            "recurring": AnyCodable(event.hasRecurrenceRules),
        ]

        if let calendar = event.calendar {
            dict["calendar"] = AnyCodable(calendar.title)
            dict["calendar_id"] = AnyCodable(calendar.calendarIdentifier)
        }

        if let location = event.location, !location.isEmpty {
            dict["location"] = AnyCodable(location)
        } else {
            dict["location"] = AnyCodable(NSNull())
        }

        if let notes = event.notes, !notes.isEmpty {
            dict["notes"] = AnyCodable(notes)
        } else {
            dict["notes"] = AnyCodable(NSNull())
        }

        return dict
    }

    /// Convert EKCalendarType to a human-readable string.
    private func calendarTypeName(_ type: EKCalendarType) -> String {
        switch type {
        case .local: return "local"
        case .calDAV: return "caldav"
        case .exchange: return "exchange"
        case .subscription: return "subscription"
        case .birthday: return "birthday"
        @unknown default: return "unknown"
        }
    }
}
