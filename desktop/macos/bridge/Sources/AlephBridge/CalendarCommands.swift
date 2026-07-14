import AppKit
import EventKit
import Foundation

// MARK: - Calendar (EventKit)

private let calendarStore = EKEventStore()

/// Request calendar access synchronously using a semaphore.
///
/// Safe only off the main thread — the RPC entry point parks the main thread on
/// a semaphore, and PIM calls run on the serial PIM queue.
private func requireCalendarAccess() throws {
    let semaphore = DispatchSemaphore(value: 0)
    var granted = false

    if #available(macOS 14.0, *) {
        calendarStore.requestFullAccessToEvents { g, error in
            granted = g
            if let error = error {
                fputs("Calendar access error: \(error.localizedDescription)\n", stderr)
            }
            semaphore.signal()
        }
    } else {
        calendarStore.requestAccess(to: .event) { g, error in
            granted = g
            if let error = error {
                fputs("Calendar access error: \(error.localizedDescription)\n", stderr)
            }
            semaphore.signal()
        }
    }
    semaphore.wait()
    guard granted else {
        throw RpcError(
            code: -32001,
            message: "permission denied: calendars",
            data: try encodeCodable(Perm.guide(.calendars))
        )
    }
}

private let calFmt = iso8601Formatter()

private func eventToWire(_ event: EKEvent) -> CalendarEvent {
    let location = event.location.flatMap { $0.isEmpty ? nil : $0 }
    let notes = event.notes.flatMap { $0.isEmpty ? nil : $0 }
    return CalendarEvent(
        id: event.eventIdentifier ?? "",
        title: event.title ?? "",
        calendar_id: event.calendar.calendarIdentifier,
        start: calFmt.string(from: event.startDate),
        end: calFmt.string(from: event.endDate),
        all_day: event.isAllDay,
        location: location,
        notes: notes
    )
}

/// Apple Calendar operations. Synchronous and blocking; call from the serial PIM
/// queue (see `PimHandlers.swift`).
enum CalendarCommands {
    static func events(from: Date, to: Date, calendarId: String?) throws -> CalendarEventsResult {
        try requireCalendarAccess()

        var calendars: [EKCalendar]?
        if let cid = calendarId, !cid.isEmpty {
            guard let cal = calendarStore.calendar(withIdentifier: cid) else {
                throw RpcError(
                    code: -32602,
                    message: "pim.calendar.events: calendar not found: \(cid)",
                    data: nil
                )
            }
            calendars = [cal]
        }

        let predicate = calendarStore.predicateForEvents(
            withStart: from, end: to, calendars: calendars
        )
        let events = calendarStore.events(matching: predicate)
        return CalendarEventsResult(events: events.map { eventToWire($0) })
    }

    static func get(id: String) throws -> CalendarEvent {
        try requireCalendarAccess()

        guard let event = calendarStore.event(withIdentifier: id) else {
            throw RpcError(
                code: -32602,
                message: "pim.calendar.get: event not found: \(id)",
                data: nil
            )
        }
        return eventToWire(event)
    }

    static func create(
        title: String,
        start: Date,
        end: Date,
        calendarId: String,
        allDay: Bool,
        location: String?,
        notes: String?
    ) throws -> CalendarCreateResult {
        try requireCalendarAccess()

        let event = EKEvent(eventStore: calendarStore)
        event.title = title
        event.startDate = start
        event.endDate = end
        event.isAllDay = allDay
        event.location = location
        event.notes = notes

        // An empty or unknown calendar_id means "wherever new events go".
        if !calendarId.isEmpty, let cal = calendarStore.calendar(withIdentifier: calendarId) {
            event.calendar = cal
        } else if let fallback = calendarStore.defaultCalendarForNewEvents {
            event.calendar = fallback
        } else {
            throw RpcError(
                code: -32003,
                message: "pim.calendar.create: no writable calendar available",
                data: nil
            )
        }

        do {
            try calendarStore.save(event, span: .thisEvent)
        } catch {
            throw RpcError(
                code: -32003,
                message: "pim.calendar.create: \(error.localizedDescription)",
                data: nil
            )
        }

        return CalendarCreateResult(id: event.eventIdentifier ?? "")
    }

    static func update(
        id: String,
        title: String,
        start: Date,
        end: Date,
        location: String?,
        notes: String?
    ) throws {
        try requireCalendarAccess()

        guard let event = calendarStore.event(withIdentifier: id) else {
            throw RpcError(
                code: -32602,
                message: "pim.calendar.update: event not found: \(id)",
                data: nil
            )
        }

        event.title = title
        event.startDate = start
        event.endDate = end
        if let location = location { event.location = location }
        if let notes = notes { event.notes = notes }

        do {
            try calendarStore.save(event, span: .thisEvent)
        } catch {
            throw RpcError(
                code: -32003,
                message: "pim.calendar.update: \(error.localizedDescription)",
                data: nil
            )
        }
    }

    static func delete(id: String) throws {
        try requireCalendarAccess()

        guard let event = calendarStore.event(withIdentifier: id) else {
            throw RpcError(
                code: -32602,
                message: "pim.calendar.delete: event not found: \(id)",
                data: nil
            )
        }

        do {
            try calendarStore.remove(event, span: .thisEvent)
        } catch {
            throw RpcError(
                code: -32003,
                message: "pim.calendar.delete: \(error.localizedDescription)",
                data: nil
            )
        }
    }

    static func calendars() throws -> CalendarListsResult {
        try requireCalendarAccess()

        let calendars = calendarStore.calendars(for: .event).map { cal in
            CalendarInfo(
                id: cal.calendarIdentifier,
                title: cal.title,
                read_only: !cal.allowsContentModifications,
                color: hexColor(cal.cgColor)
            )
        }
        return CalendarListsResult(calendars: calendars)
    }
}

private func hexColor(_ cgColor: CGColor?) -> String? {
    guard let cgColor = cgColor, let color = NSColor(cgColor: cgColor) else { return nil }
    let rgb = color.usingColorSpace(.sRGB) ?? color
    let r = Int(rgb.redComponent * 255)
    let g = Int(rgb.greenComponent * 255)
    let b = Int(rgb.blueComponent * 255)
    return String(format: "#%02X%02X%02X", r, g, b)
}
