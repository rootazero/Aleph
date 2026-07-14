import Foundation

// MARK: - Wire types
//
// These mirror `aleph_protocol::desktop_bridge::methods::pim` field-for-field:
// snake_case keys, `chrono::DateTime<Utc>` as an RFC 3339 string, `NaiveDate` as
// "yyyy-MM-dd". Property names are deliberately snake_case (like PermGuide.swift)
// so no CodingKeys mapping can drift away from the Rust side.
//
// Every Optional property below maps to an `Option<T>` in Rust — that pairing is
// what makes the wire work in both directions, and it must hold for any field
// added here:
//   • Swift → Rust: the synthesised `encode(to:)` calls `encodeIfPresent`, so a
//     nil is *omitted* (not `null`), and serde reads a missing `Option` field as
//     `None`. Declaring a Swift field Optional whose Rust twin is *not* an
//     `Option` would therefore drop a required key and fail decoding at runtime.
//   • Rust → Swift: `skip_serializing_if` omits a `None`, which the synthesised
//     `init(from:)` decodes back to nil.

struct NotesListParams: Codable {
    let folder: String?
}

/// Every `pim.*` method that addresses one object by id takes exactly `{ id }`
/// (notes.get/delete, calendar.get/delete, reminders.get/complete/delete,
/// contacts.get, mail.get), so one Swift struct covers all of them.
struct PimIdParams: Codable {
    let id: String
}

struct NotesCreateParams: Codable {
    let folder: String
    let title: String
    let body: String
}

struct NotesUpdateParams: Codable {
    let id: String
    let title: String?
    let body: String?
}

struct NoteInfo: Codable {
    let id: String
    let title: String
    let folder: String
    let modified_at: String
    let snippet: String
}

struct NotesListResult: Codable {
    let notes: [NoteInfo]
}

struct NoteContent: Codable {
    let id: String
    let title: String
    let folder: String
    let body: String
    let created_at: String
    let modified_at: String
}

struct NotesCreateResult: Codable {
    let id: String
}

struct NotesFolderEntry: Codable {
    let name: String
}

struct NotesFoldersResult: Codable {
    let folders: [NotesFolderEntry]
}

struct CalendarEventsParams: Codable {
    let from: String
    let to: String
    let calendar_id: String?
}

struct CalendarCreateParams: Codable {
    let title: String
    let start: String
    let end: String
    let calendar_id: String
    let all_day: Bool
    let location: String?
    let notes: String?
}

struct CalendarUpdateParams: Codable {
    let id: String
    let title: String
    let start: String
    let end: String
    let location: String?
    let notes: String?
}

struct CalendarEvent: Codable {
    let id: String
    let title: String
    let calendar_id: String
    let start: String
    let end: String
    let all_day: Bool
    let location: String?
    let notes: String?
}

struct CalendarEventsResult: Codable {
    let events: [CalendarEvent]
}

struct CalendarCreateResult: Codable {
    let id: String
}

struct CalendarInfo: Codable {
    let id: String
    let title: String
    let read_only: Bool
    let color: String?
}

struct CalendarListsResult: Codable {
    let calendars: [CalendarInfo]
}

struct RemindersListParams: Codable {
    let list_id: String?
    let include_completed: Bool
}

struct RemindersCreateParams: Codable {
    let title: String
    let list_id: String
    let priority: Int
    let due_date: String?
    let notes: String?
}

struct Reminder: Codable {
    let id: String
    let title: String
    let list_id: String
    let completed: Bool
    let due_date: String?
    let priority: Int
    let notes: String?
}

struct RemindersListResult: Codable {
    let reminders: [Reminder]
}

struct RemindersCreateResult: Codable {
    let id: String
}

struct ReminderList: Codable {
    let id: String
    let title: String
    let count: Int
}

struct RemindersListsResult: Codable {
    let lists: [ReminderList]
}

struct ContactsSearchParams: Codable {
    let query: String
}

struct Contact: Codable {
    let id: String
    let name: String
    let email: String?
    let phone: String?
}

struct ContactsSearchResult: Codable {
    let contacts: [Contact]
}

struct LabeledValue: Codable {
    let label: String
    let value: String
}

struct ContactDetail: Codable {
    let id: String
    let name: String
    let emails: [LabeledValue]
    let phones: [LabeledValue]
    let addresses: [LabeledValue]
    let organization: String?
    let job_title: String?
    let birthday: String?
    let notes: String?
}

struct ContactGroup: Codable {
    let id: String
    let name: String
    let count: Int
}

struct ContactsGroupsResult: Codable {
    let groups: [ContactGroup]
}

// MARK: - PIM work queue

/// Serial queue every PIM call runs on.
///
/// Two constraints force it. (1) The work is synchronous and can block for
/// seconds (AppleScript round-trips to Notes; EventKit/Contacts access prompts
/// are awaited on a semaphore) — running it inside the handler task would park a
/// Swift-concurrency cooperative thread. (2) The main thread is not available:
/// the RPC entry point parks it on a semaphore, so `DispatchQueue.main` never
/// drains and `NSAppleScript` must be driven from here instead. Serial, so the
/// shared `NSAppleScript` / `EKEventStore` / `CNContactStore` instances are only
/// ever touched from one thread.
private let pimQueue = DispatchQueue(label: "ai.aleph.bridge.pim")

private func onPimQueue<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
    try await withCheckedThrowingContinuation { continuation in
        pimQueue.async {
            do {
                continuation.resume(returning: try work())
            } catch {
                continuation.resume(throwing: error)
            }
        }
    }
}

/// Result for the mutating methods whose Rust callers decode into
/// `serde_json::Value` and discard (notes/calendar/reminders update, delete,
/// complete). Any JSON object satisfies them.
private func ack() -> JSONValue {
    .object(["ok": .bool(true)])
}

/// Parse a required RFC 3339 param, rejecting rather than defaulting: the Rust
/// side always sends these, so an unparseable value is a bug worth surfacing.
private func requireDate(_ raw: String, _ field: String, _ method: String) throws -> Date {
    guard let date = parseISO8601(raw) else {
        throw RpcError(
            code: -32602,
            message: "\(method): '\(field)' is not an RFC 3339 timestamp: \(raw)",
            data: nil
        )
    }
    return date
}

private func optionalDate(_ raw: String?, _ field: String, _ method: String) throws -> Date? {
    guard let raw = raw else { return nil }
    return try requireDate(raw, field, method)
}

// MARK: - Registration

/// Register every `pim.*` JSON-RPC method.
///
/// The handlers are a thin skin: they decode params, hop onto `pimQueue`, and
/// delegate to the `NotesCommands` / `CalendarCommands` / `RemindersCommands` /
/// `ContactsCommands` types that own the AppleScript / EventKit / Contacts work.
///
/// `pim.mail.*` has no Swift implementation (no Mail command type exists), so
/// those three methods are registered as explicit -32002 NotImplemented rather
/// than silently missing.
func registerPimHandlers(_ router: Router) async {
    // MARK: notes.*

    await router.register("pim.notes.list") { params in
        let args = try decodeCodable(params, as: NotesListParams.self)
        return try await onPimQueue {
            try encodeCodable(NotesCommands.list(folder: args.folder))
        }
    }

    await router.register("pim.notes.get") { params in
        let args = try decodeCodable(params, as: PimIdParams.self)
        return try await onPimQueue {
            try encodeCodable(NotesCommands.get(id: args.id))
        }
    }

    await router.register("pim.notes.create") { params in
        let args = try decodeCodable(params, as: NotesCreateParams.self)
        return try await onPimQueue {
            try encodeCodable(
                NotesCommands.create(folder: args.folder, title: args.title, body: args.body)
            )
        }
    }

    await router.register("pim.notes.update") { params in
        let args = try decodeCodable(params, as: NotesUpdateParams.self)
        return try await onPimQueue {
            try NotesCommands.update(id: args.id, title: args.title, body: args.body)
            return ack()
        }
    }

    await router.register("pim.notes.delete") { params in
        let args = try decodeCodable(params, as: PimIdParams.self)
        return try await onPimQueue {
            try NotesCommands.delete(id: args.id)
            return ack()
        }
    }

    await router.register("pim.notes.folders") { _ in
        try await onPimQueue {
            try encodeCodable(NotesCommands.folders())
        }
    }

    // MARK: calendar.*

    await router.register("pim.calendar.events") { params in
        let args = try decodeCodable(params, as: CalendarEventsParams.self)
        let from = try requireDate(args.from, "from", "pim.calendar.events")
        let to = try requireDate(args.to, "to", "pim.calendar.events")
        return try await onPimQueue {
            try encodeCodable(
                CalendarCommands.events(from: from, to: to, calendarId: args.calendar_id)
            )
        }
    }

    await router.register("pim.calendar.get") { params in
        let args = try decodeCodable(params, as: PimIdParams.self)
        return try await onPimQueue {
            try encodeCodable(CalendarCommands.get(id: args.id))
        }
    }

    await router.register("pim.calendar.create") { params in
        let args = try decodeCodable(params, as: CalendarCreateParams.self)
        let start = try requireDate(args.start, "start", "pim.calendar.create")
        let end = try requireDate(args.end, "end", "pim.calendar.create")
        return try await onPimQueue {
            try encodeCodable(
                CalendarCommands.create(
                    title: args.title,
                    start: start,
                    end: end,
                    calendarId: args.calendar_id,
                    allDay: args.all_day,
                    location: args.location,
                    notes: args.notes
                )
            )
        }
    }

    await router.register("pim.calendar.update") { params in
        let args = try decodeCodable(params, as: CalendarUpdateParams.self)
        let start = try requireDate(args.start, "start", "pim.calendar.update")
        let end = try requireDate(args.end, "end", "pim.calendar.update")
        return try await onPimQueue {
            try CalendarCommands.update(
                id: args.id,
                title: args.title,
                start: start,
                end: end,
                location: args.location,
                notes: args.notes
            )
            return ack()
        }
    }

    await router.register("pim.calendar.delete") { params in
        let args = try decodeCodable(params, as: PimIdParams.self)
        return try await onPimQueue {
            try CalendarCommands.delete(id: args.id)
            return ack()
        }
    }

    await router.register("pim.calendar.lists") { _ in
        try await onPimQueue {
            try encodeCodable(CalendarCommands.calendars())
        }
    }

    // MARK: reminders.*

    await router.register("pim.reminders.list") { params in
        let args = try decodeCodable(params, as: RemindersListParams.self)
        return try await onPimQueue {
            try encodeCodable(
                RemindersCommands.list(
                    listId: args.list_id,
                    includeCompleted: args.include_completed
                )
            )
        }
    }

    await router.register("pim.reminders.get") { params in
        let args = try decodeCodable(params, as: PimIdParams.self)
        return try await onPimQueue {
            try encodeCodable(RemindersCommands.get(id: args.id))
        }
    }

    await router.register("pim.reminders.create") { params in
        let args = try decodeCodable(params, as: RemindersCreateParams.self)
        let dueDate = try optionalDate(args.due_date, "due_date", "pim.reminders.create")
        return try await onPimQueue {
            try encodeCodable(
                RemindersCommands.create(
                    title: args.title,
                    listId: args.list_id,
                    priority: args.priority,
                    dueDate: dueDate,
                    notes: args.notes
                )
            )
        }
    }

    // The Rust `PimCapability::reminders_complete` only ever means "mark done";
    // there is no un-complete direction in the trait, so no flag is decoded.
    await router.register("pim.reminders.complete") { params in
        let args = try decodeCodable(params, as: PimIdParams.self)
        return try await onPimQueue {
            try RemindersCommands.complete(id: args.id)
            return ack()
        }
    }

    await router.register("pim.reminders.delete") { params in
        let args = try decodeCodable(params, as: PimIdParams.self)
        return try await onPimQueue {
            try RemindersCommands.delete(id: args.id)
            return ack()
        }
    }

    await router.register("pim.reminders.lists") { _ in
        try await onPimQueue {
            try encodeCodable(RemindersCommands.lists())
        }
    }

    // MARK: contacts.*

    await router.register("pim.contacts.search") { params in
        let args = try decodeCodable(params, as: ContactsSearchParams.self)
        return try await onPimQueue {
            try encodeCodable(ContactsCommands.search(query: args.query))
        }
    }

    await router.register("pim.contacts.get") { params in
        let args = try decodeCodable(params, as: PimIdParams.self)
        return try await onPimQueue {
            try encodeCodable(ContactsCommands.get(id: args.id))
        }
    }

    await router.register("pim.contacts.groups") { _ in
        try await onPimQueue {
            try encodeCodable(ContactsCommands.groups())
        }
    }

    // MARK: mail.* — genuine gap

    for method in ["pim.mail.search", "pim.mail.get", "pim.mail.folders"] {
        await router.register(method) { _ -> JSONValue in
            throw RpcError(
                code: -32002,
                message: "\(method): not implemented — the macOS bridge has no Mail command",
                data: nil
            )
        }
    }
}
