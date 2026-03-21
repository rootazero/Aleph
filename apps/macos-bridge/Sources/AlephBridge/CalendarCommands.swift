import AppKit
import ArgumentParser
import EventKit
import Foundation

// MARK: - Calendar (EventKit)

private let calendarStore = EKEventStore()

/// Request calendar access synchronously using a semaphore.
private func requestCalendarAccess() {
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
    if !granted {
        printError("Calendar access denied. Grant access in System Settings > Privacy & Security > Calendars.")
    }
}

private let calFmt = iso8601Formatter()

private func eventToDict(_ event: EKEvent) -> [String: Any] {
    var dict: [String: Any] = [
        "id": event.eventIdentifier ?? "",
        "title": event.title ?? "",
        "calendar_id": event.calendar.calendarIdentifier,
        "start": calFmt.string(from: event.startDate),
        "end": calFmt.string(from: event.endDate),
        "all_day": event.isAllDay,
    ]
    if let loc = event.location, !loc.isEmpty { dict["location"] = loc }
    if let notes = event.notes, !notes.isEmpty { dict["notes"] = notes }
    return dict
}

extension AlephBridge {
    struct Calendar: ParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Apple Calendar operations",
            subcommands: [Events.self, Get.self, Create.self, Update.self, Delete.self, Calendars.self]
        )

        struct Events: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List calendar events")

            @Option(name: .long, help: "Start date (ISO 8601)")
            var from: String?

            @Option(name: .long, help: "End date (ISO 8601)")
            var to: String?

            @Option(name: .long, help: "Calendar ID filter")
            var calendarId: String?

            func run() {
                requestCalendarAccess()

                let now = Date()
                let startDate = from.flatMap { parseISO8601($0) } ?? now
                let endDate = to.flatMap { parseISO8601($0) } ?? now.addingTimeInterval(7 * 24 * 3600) // default 7 days

                var calendars: [EKCalendar]? = nil
                if let cid = calendarId {
                    if let cal = calendarStore.calendar(withIdentifier: cid) {
                        calendars = [cal]
                    } else {
                        printError("Calendar not found: \(cid)")
                    }
                }

                let predicate = calendarStore.predicateForEvents(withStart: startDate, end: endDate, calendars: calendars)
                let events = calendarStore.events(matching: predicate)

                let results = events.map { eventToDict($0) }
                printJSON(["events": results])
            }
        }

        struct Get: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Get an event by ID")

            @Argument(help: "Event ID")
            var id: String

            func run() {
                requestCalendarAccess()

                guard let event = calendarStore.event(withIdentifier: id) else {
                    printError("Event not found: \(id)")
                }
                printJSON(eventToDict(event))
            }
        }

        struct Create: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Create a calendar event")

            @Option(name: .long, help: "Event title")
            var title: String = ""

            @Option(name: .long, help: "Start datetime (ISO 8601)")
            var start: String = ""

            @Option(name: .long, help: "End datetime (ISO 8601)")
            var end: String = ""

            @Option(name: .long, help: "Calendar ID")
            var calendarId: String?

            @Option(name: .long, help: "Location")
            var location: String?

            @Option(name: .long, help: "Notes / description")
            var notes: String?

            @Flag(name: .long, help: "All-day event")
            var allDay: Bool = false

            func run() {
                requestCalendarAccess()

                guard let startDate = parseISO8601(start) else {
                    printError("Invalid start date: \(start)")
                }
                guard let endDate = parseISO8601(end) else {
                    printError("Invalid end date: \(end)")
                }

                let event = EKEvent(eventStore: calendarStore)
                event.title = title
                event.startDate = startDate
                event.endDate = endDate
                event.isAllDay = allDay
                event.location = location
                event.notes = notes

                if let cid = calendarId, let cal = calendarStore.calendar(withIdentifier: cid) {
                    event.calendar = cal
                } else {
                    event.calendar = calendarStore.defaultCalendarForNewEvents
                }

                do {
                    try calendarStore.save(event, span: .thisEvent)
                } catch {
                    printError("Failed to create event: \(error.localizedDescription)")
                }

                printJSON(eventToDict(event))
            }
        }

        struct Update: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Update a calendar event")

            @Argument(help: "Event ID")
            var id: String

            @Option(name: .long, help: "New title")
            var title: String?

            @Option(name: .long, help: "New start datetime (ISO 8601)")
            var start: String?

            @Option(name: .long, help: "New end datetime (ISO 8601)")
            var end: String?

            @Option(name: .long, help: "New location")
            var location: String?

            @Option(name: .long, help: "New notes")
            var notes: String?

            func run() {
                requestCalendarAccess()

                guard let event = calendarStore.event(withIdentifier: id) else {
                    printError("Event not found: \(id)")
                }

                if let t = title { event.title = t }
                if let s = start {
                    guard let d = parseISO8601(s) else { printError("Invalid start date: \(s)") }
                    event.startDate = d
                }
                if let e = end {
                    guard let d = parseISO8601(e) else { printError("Invalid end date: \(e)") }
                    event.endDate = d
                }
                if let l = location { event.location = l }
                if let n = notes { event.notes = n }

                do {
                    try calendarStore.save(event, span: .thisEvent)
                } catch {
                    printError("Failed to update event: \(error.localizedDescription)")
                }

                printJSON(eventToDict(event))
            }
        }

        struct Delete: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Delete a calendar event")

            @Argument(help: "Event ID")
            var id: String

            func run() {
                requestCalendarAccess()

                guard let event = calendarStore.event(withIdentifier: id) else {
                    printError("Event not found: \(id)")
                }

                do {
                    try calendarStore.remove(event, span: .thisEvent)
                } catch {
                    printError("Failed to delete event: \(error.localizedDescription)")
                }

                printJSON(["deleted": true, "id": id])
            }
        }

        struct Calendars: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List all calendars")

            func run() {
                requestCalendarAccess()

                let calendars = calendarStore.calendars(for: .event)
                let results: [[String: Any]] = calendars.map { cal in
                    var dict: [String: Any] = [
                        "id": cal.calendarIdentifier,
                        "title": cal.title,
                        "read_only": !cal.allowsContentModifications,
                    ]
                    if let cgColor = cal.cgColor,
                       let color = NSColor(cgColor: cgColor) {
                        let rgb = color.usingColorSpace(.sRGB) ?? color
                        let r = Int(rgb.redComponent * 255)
                        let g = Int(rgb.greenComponent * 255)
                        let b = Int(rgb.blueComponent * 255)
                        dict["color"] = String(format: "#%02X%02X%02X", r, g, b)
                    }
                    return dict
                }
                printJSON(["calendars": results])
            }
        }
    }
}
