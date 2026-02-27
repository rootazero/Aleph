# macOS PIM Native API Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add Calendar, Reminders, Notes, and Contacts access to Aleph via macOS native frameworks, exposed as a unified `pim` tool.

**Architecture:** Extend the existing Desktop Bridge (UDS JSON-RPC 2.0) with `pim.*` methods. Swift services use EventKit (Calendar/Reminders), Contacts.framework, and AppleScript (Notes). Rust Core gets a single `PimTool` that routes actions through `DesktopBridgeClient`.

**Tech Stack:** Swift (EventKit, Contacts, Foundation/Process), Rust (serde, schemars, async-trait), JSON-RPC 2.0 over UDS.

**Design doc:** `docs/plans/2026-02-27-macos-pim-native-api-design.md`

---

## Task 1: Add Privacy Usage Descriptions to Info.plist

**Files:**
- Modify: `apps/macos-native/Aleph/Info.plist`
- Modify: `apps/macos-native/project.yml` (add info properties)

**Step 1: Add privacy keys to Info.plist**

Add before the closing `</dict>` tag in `apps/macos-native/Aleph/Info.plist`:

```xml
	<key>NSCalendarsFullAccessUsageDescription</key>
	<string>Aleph needs calendar access to help you manage events and schedules.</string>
	<key>NSRemindersFullAccessUsageDescription</key>
	<string>Aleph needs reminders access to help you manage tasks and to-dos.</string>
	<key>NSContactsUsageDescription</key>
	<string>Aleph needs contacts access to help you find and manage people.</string>
	<key>NSAppleEventsUsageDescription</key>
	<string>Aleph needs automation access to interact with Notes and other apps.</string>
```

**Step 2: Add matching properties to project.yml**

In `apps/macos-native/project.yml`, under `targets.Aleph.info.properties`, add:

```yaml
        NSCalendarsFullAccessUsageDescription: "Aleph needs calendar access to help you manage events and schedules."
        NSRemindersFullAccessUsageDescription: "Aleph needs reminders access to help you manage tasks and to-dos."
        NSContactsUsageDescription: "Aleph needs contacts access to help you find and manage people."
        NSAppleEventsUsageDescription: "Aleph needs automation access to interact with Notes and other apps."
```

**Step 3: Regenerate Xcode project and verify**

```bash
cd /Users/zouguojun/Workspace/Aleph/apps/macos-native && xcodegen generate
```

Expected: "Generated project" with no errors.

**Step 4: Commit**

```bash
git add apps/macos-native/Aleph/Info.plist apps/macos-native/project.yml
git commit -m "macos: add privacy usage descriptions for PIM APIs"
```

---

## Task 2: Create CalendarService.swift

**Files:**
- Create: `apps/macos-native/Aleph/PIM/CalendarService.swift`

**Step 1: Create the CalendarService**

Create `apps/macos-native/Aleph/PIM/CalendarService.swift`:

```swift
import EventKit
import Foundation

/// Wraps EventKit calendar operations for the Bridge.
///
/// All public methods accept `[String: AnyCodable]` params and return
/// `Result<AnyCodable, BridgeServer.HandlerError>` to match the Bridge handler signature.
final class CalendarService {
    private let store = EKEventStore()
    private var accessGranted = false

    // MARK: - Permission

    /// Request full calendar access. Caches result after first call.
    func ensureAccess() async throws {
        guard !accessGranted else { return }
        if #available(macOS 14.0, *) {
            let granted = try await store.requestFullAccessToEvents()
            guard granted else {
                throw PIMError.permissionDenied(
                    "Calendar access denied. Grant in System Settings > Privacy & Security > Calendars.")
            }
            accessGranted = true
        } else {
            let granted = try await store.requestAccess(to: .event)
            guard granted else {
                throw PIMError.permissionDenied(
                    "Calendar access denied. Grant in System Settings > Privacy & Security > Calendars.")
            }
            accessGranted = true
        }
    }

    // MARK: - List Events

    func listEvents(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let fromStr = params["from"]?.stringValue,
              let toStr = params["to"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams,
                                  message: "Missing required 'from' and 'to' date params (ISO 8601)"))
        }

        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]

        guard let from = formatter.date(from: fromStr) ?? ISO8601DateFormatter().date(from: fromStr) else {
            return .failure(.init(code: BridgeErrorCode.invalidParams,
                                  message: "Invalid 'from' date format. Expected ISO 8601."))
        }
        guard let to = formatter.date(from: toStr) ?? ISO8601DateFormatter().date(from: toStr) else {
            return .failure(.init(code: BridgeErrorCode.invalidParams,
                                  message: "Invalid 'to' date format. Expected ISO 8601."))
        }

        // Optional calendar filter
        var calendars: [EKCalendar]? = nil
        if let calId = params["calendar_id"]?.stringValue {
            if let cal = store.calendar(withIdentifier: calId) {
                calendars = [cal]
            }
        }

        let predicate = store.predicateForEvents(withStart: from, end: to, calendars: calendars)
        let events = store.events(matching: predicate)

        let result = events.map { eventToDict($0) }
        return .success(AnyCodable(["events": AnyCodable(result.map { AnyCodable($0) })]))
    }

    // MARK: - Get Event

    func getEvent(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }
        guard let event = store.event(withIdentifier: id) else {
            return .failure(.init(code: PIMErrorCode.notFound, message: "Event '\(id)' not found"))
        }
        return .success(AnyCodable(eventToDict(event)))
    }

    // MARK: - Create Event

    func createEvent(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let title = params["title"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'title' param"))
        }
        guard let startStr = params["start"]?.stringValue,
              let endStr = params["end"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams,
                                  message: "Missing required 'start' and 'end' date params"))
        }

        let formatter = ISO8601DateFormatter()
        guard let start = formatter.date(from: startStr),
              let end = formatter.date(from: endStr) else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Invalid date format"))
        }

        let event = EKEvent(eventStore: store)
        event.title = title
        event.startDate = start
        event.endDate = end
        event.isAllDay = params["all_day"]?.boolValue ?? false

        if let location = params["location"]?.stringValue { event.location = location }
        if let notes = params["notes"]?.stringValue { event.notes = notes }

        // Calendar selection
        if let calId = params["calendar_id"]?.stringValue,
           let cal = store.calendar(withIdentifier: calId) {
            event.calendar = cal
        } else {
            event.calendar = store.defaultCalendarForNewEvents
        }

        do {
            try store.save(event, span: .thisEvent)
            return .success(AnyCodable([
                "id": AnyCodable(event.eventIdentifier ?? ""),
                "success": AnyCodable(true),
            ]))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to save event: \(error.localizedDescription)"))
        }
    }

    // MARK: - Update Event

    func updateEvent(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }
        guard let event = store.event(withIdentifier: id) else {
            return .failure(.init(code: PIMErrorCode.notFound, message: "Event '\(id)' not found"))
        }

        let formatter = ISO8601DateFormatter()
        if let title = params["title"]?.stringValue { event.title = title }
        if let startStr = params["start"]?.stringValue, let d = formatter.date(from: startStr) { event.startDate = d }
        if let endStr = params["end"]?.stringValue, let d = formatter.date(from: endStr) { event.endDate = d }
        if let location = params["location"]?.stringValue { event.location = location }
        if let notes = params["notes"]?.stringValue { event.notes = notes }

        do {
            try store.save(event, span: .thisEvent)
            return .success(AnyCodable(["success": AnyCodable(true)]))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to update event: \(error.localizedDescription)"))
        }
    }

    // MARK: - Delete Event

    func deleteEvent(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }
        guard let event = store.event(withIdentifier: id) else {
            return .failure(.init(code: PIMErrorCode.notFound, message: "Event '\(id)' not found"))
        }

        do {
            try store.remove(event, span: .thisEvent)
            return .success(AnyCodable(["success": AnyCodable(true)]))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to delete event: \(error.localizedDescription)"))
        }
    }

    // MARK: - List Calendars

    func listCalendars(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        let calendars = store.calendars(for: .event)
        let result = calendars.map { cal -> [String: AnyCodable] in
            [
                "id": AnyCodable(cal.calendarIdentifier),
                "title": AnyCodable(cal.title),
                "color": AnyCodable(cal.cgColor?.components?.description ?? ""),
                "type": AnyCodable(cal.type.rawValue),
                "allows_modify": AnyCodable(cal.allowsContentModifications),
            ]
        }
        return .success(AnyCodable(["calendars": AnyCodable(result.map { AnyCodable($0) })]))
    }

    // MARK: - Helpers

    private func eventToDict(_ event: EKEvent) -> [String: AnyCodable] {
        let formatter = ISO8601DateFormatter()
        return [
            "id": AnyCodable(event.eventIdentifier ?? ""),
            "title": AnyCodable(event.title ?? ""),
            "start": AnyCodable(formatter.string(from: event.startDate)),
            "end": AnyCodable(formatter.string(from: event.endDate)),
            "calendar": AnyCodable(event.calendar?.title ?? ""),
            "calendar_id": AnyCodable(event.calendar?.calendarIdentifier ?? ""),
            "location": AnyCodable(event.location ?? ""),
            "notes": AnyCodable(event.notes ?? ""),
            "all_day": AnyCodable(event.isAllDay),
            "recurring": AnyCodable(event.hasRecurrenceRules),
        ]
    }
}
```

**Step 2: Create PIM error types**

Create `apps/macos-native/Aleph/PIM/PIMError.swift`:

```swift
import Foundation

/// PIM-specific error types.
enum PIMError: Error, LocalizedError {
    case permissionDenied(String)
    case notFound(String)
    case scriptError(String)

    var errorDescription: String? {
        switch self {
        case .permissionDenied(let msg): return msg
        case .notFound(let msg): return msg
        case .scriptError(let msg): return msg
        }
    }
}

/// PIM-specific JSON-RPC error codes (-32001 to -32009).
enum PIMErrorCode {
    static let permissionDenied: Int32 = -32001
    static let notFound: Int32 = -32002
    static let validationFailed: Int32 = -32003
    static let scriptError: Int32 = -32004
}
```

**Step 3: Regenerate and build**

```bash
cd /Users/zouguojun/Workspace/Aleph/apps/macos-native && xcodegen generate
xcodebuild -scheme Aleph -configuration Debug build 2>&1 | tail -5
```

Expected: BUILD SUCCEEDED

**Step 4: Commit**

```bash
git add apps/macos-native/Aleph/PIM/
git commit -m "macos: add CalendarService with EventKit and PIM error types"
```

---

## Task 3: Create RemindersService.swift

**Files:**
- Create: `apps/macos-native/Aleph/PIM/RemindersService.swift`

**Step 1: Create the RemindersService**

Create `apps/macos-native/Aleph/PIM/RemindersService.swift`:

```swift
import EventKit
import Foundation

/// Wraps EventKit reminder operations for the Bridge.
final class RemindersService {
    private let store = EKEventStore()
    private var accessGranted = false

    // MARK: - Permission

    func ensureAccess() async throws {
        guard !accessGranted else { return }
        if #available(macOS 14.0, *) {
            let granted = try await store.requestFullAccessToReminders()
            guard granted else {
                throw PIMError.permissionDenied(
                    "Reminders access denied. Grant in System Settings > Privacy & Security > Reminders.")
            }
            accessGranted = true
        } else {
            let granted = try await store.requestAccess(to: .reminder)
            guard granted else {
                throw PIMError.permissionDenied(
                    "Reminders access denied. Grant in System Settings > Privacy & Security > Reminders.")
            }
            accessGranted = true
        }
    }

    // MARK: - List Reminders

    func listReminders(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        var calendars: [EKCalendar]? = nil
        if let listId = params["list_id"]?.stringValue,
           let cal = store.calendar(withIdentifier: listId) {
            calendars = [cal]
        }

        let includeCompleted = params["include_completed"]?.boolValue ?? false
        let predicate = store.predicateForReminders(in: calendars)

        // fetchReminders is callback-based; use semaphore for sync bridge handler
        var fetched: [EKReminder]?
        let semaphore = DispatchSemaphore(value: 0)
        store.fetchReminders(matching: predicate) { reminders in
            fetched = reminders
            semaphore.signal()
        }
        semaphore.wait()

        let reminders = (fetched ?? []).filter { includeCompleted || !$0.isCompleted }
        let result = reminders.map { reminderToDict($0) }
        return .success(AnyCodable(["reminders": AnyCodable(result.map { AnyCodable($0) })]))
    }

    // MARK: - Get Reminder

    func getReminder(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }
        guard let item = store.calendarItem(withIdentifier: id) as? EKReminder else {
            return .failure(.init(code: PIMErrorCode.notFound, message: "Reminder '\(id)' not found"))
        }
        return .success(AnyCodable(reminderToDict(item)))
    }

    // MARK: - Create Reminder

    func createReminder(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let title = params["title"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'title' param"))
        }

        let reminder = EKReminder(eventStore: store)
        reminder.title = title

        if let notes = params["notes"]?.stringValue { reminder.notes = notes }
        if let priority = params["priority"]?.intValue { reminder.priority = priority }

        // Due date
        if let dueDateStr = params["due_date"]?.stringValue {
            let formatter = ISO8601DateFormatter()
            if let date = formatter.date(from: dueDateStr) {
                reminder.dueDateComponents = Calendar.current.dateComponents(
                    [.year, .month, .day, .hour, .minute], from: date)
            }
        }

        // List selection
        if let listId = params["list_id"]?.stringValue,
           let cal = store.calendar(withIdentifier: listId) {
            reminder.calendar = cal
        } else {
            reminder.calendar = store.defaultCalendarForNewReminders()
        }

        do {
            try store.save(reminder, commit: true)
            return .success(AnyCodable([
                "id": AnyCodable(reminder.calendarItemIdentifier),
                "success": AnyCodable(true),
            ]))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to save reminder: \(error.localizedDescription)"))
        }
    }

    // MARK: - Complete Reminder

    func completeReminder(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }
        guard let reminder = store.calendarItem(withIdentifier: id) as? EKReminder else {
            return .failure(.init(code: PIMErrorCode.notFound, message: "Reminder '\(id)' not found"))
        }

        reminder.isCompleted = params["completed"]?.boolValue ?? true

        do {
            try store.save(reminder, commit: true)
            return .success(AnyCodable(["success": AnyCodable(true)]))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to update reminder: \(error.localizedDescription)"))
        }
    }

    // MARK: - Delete Reminder

    func deleteReminder(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }
        guard let reminder = store.calendarItem(withIdentifier: id) as? EKReminder else {
            return .failure(.init(code: PIMErrorCode.notFound, message: "Reminder '\(id)' not found"))
        }

        do {
            try store.remove(reminder, commit: true)
            return .success(AnyCodable(["success": AnyCodable(true)]))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to delete reminder: \(error.localizedDescription)"))
        }
    }

    // MARK: - List Reminder Lists

    func listReminderLists(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        let calendars = store.calendars(for: .reminder)
        let result = calendars.map { cal -> [String: AnyCodable] in
            [
                "id": AnyCodable(cal.calendarIdentifier),
                "title": AnyCodable(cal.title),
                "color": AnyCodable(cal.cgColor?.components?.description ?? ""),
                "allows_modify": AnyCodable(cal.allowsContentModifications),
            ]
        }
        return .success(AnyCodable(["lists": AnyCodable(result.map { AnyCodable($0) })]))
    }

    // MARK: - Helpers

    private func reminderToDict(_ reminder: EKReminder) -> [String: AnyCodable] {
        let formatter = ISO8601DateFormatter()
        var dueDate: String? = nil
        if let components = reminder.dueDateComponents,
           let date = Calendar.current.date(from: components) {
            dueDate = formatter.string(from: date)
        }

        return [
            "id": AnyCodable(reminder.calendarItemIdentifier),
            "title": AnyCodable(reminder.title ?? ""),
            "notes": AnyCodable(reminder.notes ?? ""),
            "completed": AnyCodable(reminder.isCompleted),
            "priority": AnyCodable(reminder.priority),
            "due_date": AnyCodable(dueDate as Any),
            "list": AnyCodable(reminder.calendar?.title ?? ""),
            "list_id": AnyCodable(reminder.calendar?.calendarIdentifier ?? ""),
        ]
    }
}
```

**Step 2: Build and verify**

```bash
cd /Users/zouguojun/Workspace/Aleph/apps/macos-native && xcodegen generate
xcodebuild -scheme Aleph -configuration Debug build 2>&1 | tail -5
```

Expected: BUILD SUCCEEDED

**Step 3: Commit**

```bash
git add apps/macos-native/Aleph/PIM/RemindersService.swift
git commit -m "macos: add RemindersService with EventKit"
```

---

## Task 4: Create ContactsService.swift

**Files:**
- Create: `apps/macos-native/Aleph/PIM/ContactsService.swift`

**Step 1: Create the ContactsService**

Create `apps/macos-native/Aleph/PIM/ContactsService.swift`:

```swift
import Contacts
import Foundation

/// Wraps Contacts.framework operations for the Bridge.
final class ContactsService {
    private let store = CNContactStore()
    private var accessGranted = false

    /// Standard keys to fetch for contacts.
    private let fetchKeys: [CNKeyDescriptor] = [
        CNContactIdentifierKey as CNKeyDescriptor,
        CNContactGivenNameKey as CNKeyDescriptor,
        CNContactFamilyNameKey as CNKeyDescriptor,
        CNContactOrganizationNameKey as CNKeyDescriptor,
        CNContactPhoneNumbersKey as CNKeyDescriptor,
        CNContactEmailAddressesKey as CNKeyDescriptor,
        CNContactPostalAddressesKey as CNKeyDescriptor,
        CNContactNoteKey as CNKeyDescriptor,
    ]

    // MARK: - Permission

    func ensureAccess() async throws {
        guard !accessGranted else { return }
        let granted = try await store.requestAccess(for: .contacts)
        guard granted else {
            throw PIMError.permissionDenied(
                "Contacts access denied. Grant in System Settings > Privacy & Security > Contacts.")
        }
        accessGranted = true
    }

    // MARK: - Search Contacts

    func searchContacts(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let query = params["query"]?.stringValue, !query.isEmpty else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'query' param"))
        }

        do {
            let predicate = CNContact.predicateForContacts(matchingName: query)
            let contacts = try store.unifiedContacts(matching: predicate, keysToFetch: fetchKeys)
            let result = contacts.map { contactToDict($0) }
            return .success(AnyCodable(["contacts": AnyCodable(result.map { AnyCodable($0) })]))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Search failed: \(error.localizedDescription)"))
        }
    }

    // MARK: - Get Contact

    func getContact(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }

        do {
            let predicate = CNContact.predicateForContacts(withIdentifiers: [id])
            let contacts = try store.unifiedContacts(matching: predicate, keysToFetch: fetchKeys)
            guard let contact = contacts.first else {
                return .failure(.init(code: PIMErrorCode.notFound, message: "Contact '\(id)' not found"))
            }
            return .success(AnyCodable(contactToDict(contact)))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Fetch failed: \(error.localizedDescription)"))
        }
    }

    // MARK: - Create Contact

    func createContact(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let givenName = params["given_name"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams,
                                  message: "Missing required 'given_name' param"))
        }

        let contact = CNMutableContact()
        contact.givenName = givenName
        if let familyName = params["family_name"]?.stringValue { contact.familyName = familyName }
        if let org = params["organization"]?.stringValue { contact.organizationName = org }
        if let note = params["notes"]?.stringValue { contact.note = note }

        // Phone numbers
        if let phones = params["phone_numbers"]?.arrayValue {
            contact.phoneNumbers = phones.compactMap { $0.stringValue }
                .map { CNLabeledValue(label: CNLabelPhoneNumberMain, value: CNPhoneNumber(stringValue: $0)) }
        }

        // Emails
        if let emails = params["emails"]?.arrayValue {
            contact.emailAddresses = emails.compactMap { $0.stringValue }
                .map { CNLabeledValue(label: CNLabelHome, value: $0 as NSString) }
        }

        let saveRequest = CNSaveRequest()
        saveRequest.add(contact, toContainerWithIdentifier: nil)

        do {
            try store.execute(saveRequest)
            return .success(AnyCodable([
                "id": AnyCodable(contact.identifier),
                "success": AnyCodable(true),
            ]))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to save contact: \(error.localizedDescription)"))
        }
    }

    // MARK: - Update Contact

    func updateContact(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }

        do {
            let predicate = CNContact.predicateForContacts(withIdentifiers: [id])
            let contacts = try store.unifiedContacts(matching: predicate, keysToFetch: fetchKeys)
            guard let existing = contacts.first else {
                return .failure(.init(code: PIMErrorCode.notFound, message: "Contact '\(id)' not found"))
            }

            let mutable = existing.mutableCopy() as! CNMutableContact
            if let givenName = params["given_name"]?.stringValue { mutable.givenName = givenName }
            if let familyName = params["family_name"]?.stringValue { mutable.familyName = familyName }
            if let org = params["organization"]?.stringValue { mutable.organizationName = org }
            if let note = params["notes"]?.stringValue { mutable.note = note }

            if let phones = params["phone_numbers"]?.arrayValue {
                mutable.phoneNumbers = phones.compactMap { $0.stringValue }
                    .map { CNLabeledValue(label: CNLabelPhoneNumberMain, value: CNPhoneNumber(stringValue: $0)) }
            }
            if let emails = params["emails"]?.arrayValue {
                mutable.emailAddresses = emails.compactMap { $0.stringValue }
                    .map { CNLabeledValue(label: CNLabelHome, value: $0 as NSString) }
            }

            let saveRequest = CNSaveRequest()
            saveRequest.update(mutable)
            try store.execute(saveRequest)
            return .success(AnyCodable(["success": AnyCodable(true)]))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to update contact: \(error.localizedDescription)"))
        }
    }

    // MARK: - Delete Contact

    func deleteContact(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }

        do {
            let predicate = CNContact.predicateForContacts(withIdentifiers: [id])
            let contacts = try store.unifiedContacts(matching: predicate, keysToFetch: fetchKeys)
            guard let existing = contacts.first else {
                return .failure(.init(code: PIMErrorCode.notFound, message: "Contact '\(id)' not found"))
            }

            let mutable = existing.mutableCopy() as! CNMutableContact
            let saveRequest = CNSaveRequest()
            saveRequest.delete(mutable)
            try store.execute(saveRequest)
            return .success(AnyCodable(["success": AnyCodable(true)]))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to delete contact: \(error.localizedDescription)"))
        }
    }

    // MARK: - List Groups

    func listGroups(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        do {
            let groups = try store.groups(matching: nil)
            let result = groups.map { group -> [String: AnyCodable] in
                [
                    "id": AnyCodable(group.identifier),
                    "name": AnyCodable(group.name),
                ]
            }
            return .success(AnyCodable(["groups": AnyCodable(result.map { AnyCodable($0) })]))
        } catch {
            return .failure(.init(code: BridgeErrorCode.internalError,
                                  message: "Failed to list groups: \(error.localizedDescription)"))
        }
    }

    // MARK: - Helpers

    private func contactToDict(_ contact: CNContact) -> [String: AnyCodable] {
        [
            "id": AnyCodable(contact.identifier),
            "given_name": AnyCodable(contact.givenName),
            "family_name": AnyCodable(contact.familyName),
            "organization": AnyCodable(contact.organizationName),
            "phone_numbers": AnyCodable(
                contact.phoneNumbers.map { AnyCodable($0.value.stringValue) }
            ),
            "emails": AnyCodable(
                contact.emailAddresses.map { AnyCodable($0.value as String) }
            ),
            "notes": AnyCodable(contact.note),
        ]
    }
}
```

**Step 2: Build and verify**

```bash
cd /Users/zouguojun/Workspace/Aleph/apps/macos-native && xcodegen generate
xcodebuild -scheme Aleph -configuration Debug build 2>&1 | tail -5
```

Expected: BUILD SUCCEEDED

**Step 3: Commit**

```bash
git add apps/macos-native/Aleph/PIM/ContactsService.swift
git commit -m "macos: add ContactsService with Contacts.framework"
```

---

## Task 5: Create NotesService.swift

**Files:**
- Create: `apps/macos-native/Aleph/PIM/NotesService.swift`

**Step 1: Create the NotesService**

Create `apps/macos-native/Aleph/PIM/NotesService.swift`:

```swift
import Foundation

/// Wraps Notes.app operations via AppleScript for the Bridge.
///
/// Notes.app has no native Swift API. All operations use `osascript` to execute
/// AppleScript commands. Notes.app launches automatically when invoked.
final class NotesService {

    // MARK: - List Notes

    func listNotes(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        let folder = params["folder"]?.stringValue
        let target = folder.map { "folder \"\($0)\"" } ?? "default account"

        let script = """
        tell application "Notes"
            set noteList to {}
            repeat with n in notes of \(target)
                set noteId to id of n
                set noteName to name of n
                set modDate to modification date of n as «class isot» as string
                set end of noteList to noteId & "|||" & noteName & "|||" & modDate
            end repeat
            set AppleScript's text item delimiters to "\\n"
            return noteList as text
        end tell
        """

        switch runAppleScript(script) {
        case .success(let output):
            let notes = parseNotesList(output)
            return .success(AnyCodable(["notes": AnyCodable(notes.map { AnyCodable($0) })]))
        case .failure(let error):
            return .failure(error)
        }
    }

    // MARK: - Get Note

    func getNote(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }

        let script = """
        tell application "Notes"
            set n to first note whose id is "\(escapeAppleScript(id))"
            set noteId to id of n
            set noteName to name of n
            set noteBody to plaintext of n
            set modDate to modification date of n as «class isot» as string
            set folderName to name of container of n
            return noteId & "|||" & noteName & "|||" & noteBody & "|||" & modDate & "|||" & folderName
        end tell
        """

        switch runAppleScript(script) {
        case .success(let output):
            let parts = output.components(separatedBy: "|||")
            guard parts.count >= 5 else {
                return .failure(.init(code: PIMErrorCode.notFound, message: "Note '\(id)' not found"))
            }
            let result: [String: AnyCodable] = [
                "id": AnyCodable(parts[0]),
                "title": AnyCodable(parts[1]),
                "body": AnyCodable(parts[2]),
                "modified": AnyCodable(parts[3]),
                "folder": AnyCodable(parts[4]),
            ]
            return .success(AnyCodable(result))
        case .failure(let error):
            return .failure(error)
        }
    }

    // MARK: - Create Note

    func createNote(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let title = params["title"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'title' param"))
        }

        let body = params["body"]?.stringValue ?? ""
        let folder = params["folder"]?.stringValue

        let target = folder.map { "folder \"\(escapeAppleScript($0))\"" } ?? "default account"

        // Notes.app uses HTML for the body property
        let htmlBody = "<h1>\(escapeHTML(title))</h1><br>\(escapeHTML(body).replacingOccurrences(of: "\n", with: "<br>"))"

        let script = """
        tell application "Notes"
            set newNote to make new note at \(target) with properties {name:"\(escapeAppleScript(title))", body:"\(escapeAppleScript(htmlBody))"}
            return id of newNote
        end tell
        """

        switch runAppleScript(script) {
        case .success(let noteId):
            return .success(AnyCodable([
                "id": AnyCodable(noteId.trimmingCharacters(in: .whitespacesAndNewlines)),
                "success": AnyCodable(true),
            ]))
        case .failure(let error):
            return .failure(error)
        }
    }

    // MARK: - Update Note

    func updateNote(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }

        var setProps: [String] = []
        if let title = params["title"]?.stringValue {
            setProps.append("set name of n to \"\(escapeAppleScript(title))\"")
        }
        if let body = params["body"]?.stringValue {
            let htmlBody = escapeHTML(body).replacingOccurrences(of: "\n", with: "<br>")
            setProps.append("set body of n to \"\(escapeAppleScript(htmlBody))\"")
        }

        guard !setProps.isEmpty else {
            return .failure(.init(code: BridgeErrorCode.invalidParams,
                                  message: "Nothing to update. Provide 'title' and/or 'body'."))
        }

        let script = """
        tell application "Notes"
            set n to first note whose id is "\(escapeAppleScript(id))"
            \(setProps.joined(separator: "\n            "))
            return "ok"
        end tell
        """

        switch runAppleScript(script) {
        case .success:
            return .success(AnyCodable(["success": AnyCodable(true)]))
        case .failure(let error):
            return .failure(error)
        }
    }

    // MARK: - Delete Note

    func deleteNote(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let id = params["id"]?.stringValue else {
            return .failure(.init(code: BridgeErrorCode.invalidParams, message: "Missing required 'id' param"))
        }

        let script = """
        tell application "Notes"
            delete (first note whose id is "\(escapeAppleScript(id))")
            return "ok"
        end tell
        """

        switch runAppleScript(script) {
        case .success:
            return .success(AnyCodable(["success": AnyCodable(true)]))
        case .failure(let error):
            return .failure(error)
        }
    }

    // MARK: - List Folders

    func listFolders(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        let script = """
        tell application "Notes"
            set folderList to {}
            repeat with f in folders
                set folderId to id of f
                set folderName to name of f
                set end of folderList to folderId & "|||" & folderName
            end repeat
            set AppleScript's text item delimiters to "\\n"
            return folderList as text
        end tell
        """

        switch runAppleScript(script) {
        case .success(let output):
            let folders = output.components(separatedBy: "\n")
                .filter { !$0.isEmpty }
                .map { line -> [String: AnyCodable] in
                    let parts = line.components(separatedBy: "|||")
                    return [
                        "id": AnyCodable(parts.count > 0 ? parts[0] : ""),
                        "name": AnyCodable(parts.count > 1 ? parts[1] : ""),
                    ]
                }
            return .success(AnyCodable(["folders": AnyCodable(folders.map { AnyCodable($0) })]))
        case .failure(let error):
            return .failure(error)
        }
    }

    // MARK: - AppleScript Execution

    private func runAppleScript(_ script: String) -> Result<String, BridgeServer.HandlerError> {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        process.arguments = ["-e", script]

        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr

        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return .failure(.init(code: PIMErrorCode.scriptError,
                                  message: "Failed to run osascript: \(error.localizedDescription)"))
        }

        if process.terminationStatus != 0 {
            let errorOutput = String(data: stderr.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
            return .failure(.init(code: PIMErrorCode.scriptError,
                                  message: "AppleScript error: \(errorOutput.trimmingCharacters(in: .whitespacesAndNewlines))"))
        }

        let output = String(data: stdout.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        return .success(output.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    // MARK: - Helpers

    private func parseNotesList(_ output: String) -> [[String: AnyCodable]] {
        output.components(separatedBy: "\n")
            .filter { !$0.isEmpty }
            .map { line in
                let parts = line.components(separatedBy: "|||")
                return [
                    "id": AnyCodable(parts.count > 0 ? parts[0] : ""),
                    "title": AnyCodable(parts.count > 1 ? parts[1] : ""),
                    "modified": AnyCodable(parts.count > 2 ? parts[2] : ""),
                ]
            }
    }

    private func escapeAppleScript(_ str: String) -> String {
        str.replacingOccurrences(of: "\\", with: "\\\\")
           .replacingOccurrences(of: "\"", with: "\\\"")
    }

    private func escapeHTML(_ str: String) -> String {
        str.replacingOccurrences(of: "&", with: "&amp;")
           .replacingOccurrences(of: "<", with: "&lt;")
           .replacingOccurrences(of: ">", with: "&gt;")
           .replacingOccurrences(of: "\"", with: "&quot;")
    }
}
```

**Step 2: Build and verify**

```bash
cd /Users/zouguojun/Workspace/Aleph/apps/macos-native && xcodegen generate
xcodebuild -scheme Aleph -configuration Debug build 2>&1 | tail -5
```

Expected: BUILD SUCCEEDED

**Step 3: Commit**

```bash
git add apps/macos-native/Aleph/PIM/NotesService.swift
git commit -m "macos: add NotesService with AppleScript"
```

---

## Task 6: Create PIMHandlers.swift and Wire into AppDelegate

**Files:**
- Create: `apps/macos-native/Aleph/Bridge/PIMHandlers.swift`
- Modify: `apps/macos-native/Aleph/AppDelegate.swift:87` (add `registerPIMHandlers()` call)
- Modify: `apps/macos-native/Aleph/Bridge/BridgeServer.swift:~285-298` (add PIM capabilities to handshake)

**Step 1: Create PIMHandlers.swift**

Create `apps/macos-native/Aleph/Bridge/PIMHandlers.swift`:

```swift
import Foundation

/// Registers all PIM (Personal Information Management) handlers on the BridgeServer.
///
/// Routes `pim.*` JSON-RPC methods to the appropriate service:
/// - `pim.calendar.*` → CalendarService (EventKit)
/// - `pim.reminders.*` → RemindersService (EventKit)
/// - `pim.contacts.*` → ContactsService (Contacts.framework)
/// - `pim.notes.*` → NotesService (AppleScript)
extension BridgeServer {

    func registerPIMHandlers() {
        let calendar = CalendarService()
        let reminders = RemindersService()
        let contacts = ContactsService()
        let notes = NotesService()

        // MARK: - Calendar

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

        // MARK: - Reminders

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

        // MARK: - Notes

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

        // MARK: - Contacts

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
```

**Step 2: Wire into AppDelegate**

In `apps/macos-native/Aleph/AppDelegate.swift`, add `bridge.registerPIMHandlers()` right after `bridge.registerDesktopHandlers()` at line 87:

```swift
        bridge.registerDesktopHandlers()
        bridge.registerPIMHandlers()  // ← ADD THIS LINE
```

**Step 3: Add PIM capabilities to handshake**

In `apps/macos-native/Aleph/Bridge/BridgeServer.swift`, find the `capabilities` array (around line 280-297) and add 4 new entries after the existing ones:

```swift
                AnyCodable(["name": AnyCodable("ax_inspect"), "version": AnyCodable("1.0")]),
                // PIM capabilities
                AnyCodable(["name": AnyCodable("pim_calendar"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("pim_reminders"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("pim_notes"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("pim_contacts"), "version": AnyCodable("1.0")]),
```

**Step 4: Build and verify**

```bash
cd /Users/zouguojun/Workspace/Aleph/apps/macos-native && xcodegen generate
xcodebuild -scheme Aleph -configuration Debug build 2>&1 | tail -5
```

Expected: BUILD SUCCEEDED

**Step 5: Commit**

```bash
git add apps/macos-native/Aleph/Bridge/PIMHandlers.swift apps/macos-native/Aleph/AppDelegate.swift apps/macos-native/Aleph/Bridge/BridgeServer.swift
git commit -m "macos: register PIM handlers and wire into Bridge"
```

---

## Task 7: Extend DesktopRequest with PIM Variants (Rust)

**Files:**
- Modify: `core/src/desktop/types.rs:63-135` (add PIM variants to DesktopRequest enum)
- Modify: `core/src/desktop/client.rs:131-217` (add PIM arms to request_to_jsonrpc)

**Step 1: Add PIM variants to DesktopRequest**

In `core/src/desktop/types.rs`, add after the `Ping` variant (around line 134):

```rust
    // Internal
    Ping,

    // ========= PIM (Personal Information Management) =========

    // Calendar
    PimCalendarList { from: String, to: String, calendar_id: Option<String> },
    PimCalendarGet { id: String },
    PimCalendarCreate {
        title: String, start: String, end: String,
        calendar_id: Option<String>, location: Option<String>,
        notes: Option<String>, all_day: Option<bool>,
    },
    PimCalendarUpdate {
        id: String, title: Option<String>, start: Option<String>,
        end: Option<String>, location: Option<String>, notes: Option<String>,
    },
    PimCalendarDelete { id: String },
    PimCalendarCalendars,

    // Reminders
    PimRemindersList { list_id: Option<String>, include_completed: Option<bool> },
    PimRemindersGet { id: String },
    PimRemindersCreate {
        title: String, list_id: Option<String>,
        due_date: Option<String>, priority: Option<i32>, notes: Option<String>,
    },
    PimRemindersComplete { id: String, completed: bool },
    PimRemindersDelete { id: String },
    PimRemindersLists,

    // Notes
    PimNotesList { folder: Option<String> },
    PimNotesGet { id: String },
    PimNotesCreate { title: String, body: Option<String>, folder: Option<String> },
    PimNotesUpdate { id: String, title: Option<String>, body: Option<String> },
    PimNotesDelete { id: String },
    PimNotesFolders,

    // Contacts
    PimContactsSearch { query: String },
    PimContactsGet { id: String },
    PimContactsCreate {
        given_name: String, family_name: Option<String>,
        organization: Option<String>, notes: Option<String>,
        phone_numbers: Option<Vec<String>>, emails: Option<Vec<String>>,
    },
    PimContactsUpdate {
        id: String, given_name: Option<String>, family_name: Option<String>,
        organization: Option<String>, notes: Option<String>,
        phone_numbers: Option<Vec<String>>, emails: Option<Vec<String>>,
    },
    PimContactsDelete { id: String },
    PimContactsGroups,
```

**Step 2: Add PIM arms to request_to_jsonrpc**

In `core/src/desktop/client.rs`, add before the closing `}` of the match in `request_to_jsonrpc` (around line 217):

```rust
        // PIM: Calendar
        DesktopRequest::PimCalendarList { from, to, calendar_id } => {
            ("pim.calendar.list", json!({ "from": from, "to": to, "calendar_id": calendar_id }))
        }
        DesktopRequest::PimCalendarGet { id } => {
            ("pim.calendar.get", json!({ "id": id }))
        }
        DesktopRequest::PimCalendarCreate { title, start, end, calendar_id, location, notes, all_day } => {
            ("pim.calendar.create", json!({
                "title": title, "start": start, "end": end,
                "calendar_id": calendar_id, "location": location,
                "notes": notes, "all_day": all_day,
            }))
        }
        DesktopRequest::PimCalendarUpdate { id, title, start, end, location, notes } => {
            ("pim.calendar.update", json!({
                "id": id, "title": title, "start": start, "end": end,
                "location": location, "notes": notes,
            }))
        }
        DesktopRequest::PimCalendarDelete { id } => {
            ("pim.calendar.delete", json!({ "id": id }))
        }
        DesktopRequest::PimCalendarCalendars => ("pim.calendar.calendars", json!({})),

        // PIM: Reminders
        DesktopRequest::PimRemindersList { list_id, include_completed } => {
            ("pim.reminders.list", json!({ "list_id": list_id, "include_completed": include_completed }))
        }
        DesktopRequest::PimRemindersGet { id } => {
            ("pim.reminders.get", json!({ "id": id }))
        }
        DesktopRequest::PimRemindersCreate { title, list_id, due_date, priority, notes } => {
            ("pim.reminders.create", json!({
                "title": title, "list_id": list_id, "due_date": due_date,
                "priority": priority, "notes": notes,
            }))
        }
        DesktopRequest::PimRemindersComplete { id, completed } => {
            ("pim.reminders.complete", json!({ "id": id, "completed": completed }))
        }
        DesktopRequest::PimRemindersDelete { id } => {
            ("pim.reminders.delete", json!({ "id": id }))
        }
        DesktopRequest::PimRemindersLists => ("pim.reminders.lists", json!({})),

        // PIM: Notes
        DesktopRequest::PimNotesList { folder } => {
            ("pim.notes.list", json!({ "folder": folder }))
        }
        DesktopRequest::PimNotesGet { id } => {
            ("pim.notes.get", json!({ "id": id }))
        }
        DesktopRequest::PimNotesCreate { title, body, folder } => {
            ("pim.notes.create", json!({ "title": title, "body": body, "folder": folder }))
        }
        DesktopRequest::PimNotesUpdate { id, title, body } => {
            ("pim.notes.update", json!({ "id": id, "title": title, "body": body }))
        }
        DesktopRequest::PimNotesDelete { id } => {
            ("pim.notes.delete", json!({ "id": id }))
        }
        DesktopRequest::PimNotesFolders => ("pim.notes.folders", json!({})),

        // PIM: Contacts
        DesktopRequest::PimContactsSearch { query } => {
            ("pim.contacts.search", json!({ "query": query }))
        }
        DesktopRequest::PimContactsGet { id } => {
            ("pim.contacts.get", json!({ "id": id }))
        }
        DesktopRequest::PimContactsCreate { given_name, family_name, organization, notes, phone_numbers, emails } => {
            ("pim.contacts.create", json!({
                "given_name": given_name, "family_name": family_name,
                "organization": organization, "notes": notes,
                "phone_numbers": phone_numbers, "emails": emails,
            }))
        }
        DesktopRequest::PimContactsUpdate { id, given_name, family_name, organization, notes, phone_numbers, emails } => {
            ("pim.contacts.update", json!({
                "id": id, "given_name": given_name, "family_name": family_name,
                "organization": organization, "notes": notes,
                "phone_numbers": phone_numbers, "emails": emails,
            }))
        }
        DesktopRequest::PimContactsDelete { id } => {
            ("pim.contacts.delete", json!({ "id": id }))
        }
        DesktopRequest::PimContactsGroups => ("pim.contacts.groups", json!({})),
```

**Step 3: Build and verify**

```bash
cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | tail -5
```

Expected: Compiles successfully.

**Step 4: Commit**

```bash
git add core/src/desktop/types.rs core/src/desktop/client.rs
git commit -m "desktop: add PIM variants to DesktopRequest and JSON-RPC mapping"
```

---

## Task 8: Create PimTool (Rust)

**Files:**
- Create: `core/src/builtin_tools/pim.rs`
- Modify: `core/src/builtin_tools/mod.rs` (add `pub mod pim;` and re-exports)

**Step 1: Create pim.rs**

Create `core/src/builtin_tools/pim.rs`:

```rust
//! PIM (Personal Information Management) tool — access macOS Calendar, Reminders,
//! Notes, and Contacts via the Desktop Bridge.
//!
//! Requires the Aleph Desktop Bridge to be connected. When the bridge is absent,
//! all operations return a friendly message instead of an error.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::desktop::{DesktopBridgeClient, DesktopRequest};
use crate::error::Result;
use crate::tools::AlephTool;

/// Arguments for the PIM tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PimArgs {
    /// The PIM action to perform.
    ///
    /// Calendar: "calendar_list", "calendar_get", "calendar_create", "calendar_update", "calendar_delete", "calendar_calendars"
    /// Reminders: "reminders_list", "reminders_get", "reminders_create", "reminders_complete", "reminders_delete", "reminders_lists"
    /// Notes: "notes_list", "notes_get", "notes_create", "notes_update", "notes_delete", "notes_folders"
    /// Contacts: "contacts_search", "contacts_get", "contacts_create", "contacts_update", "contacts_delete", "contacts_groups"
    pub action: String,

    // --- Shared params ---

    /// Item ID for get/update/delete operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Title for create/update operations (events, reminders, notes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Notes/description field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,

    // --- Calendar params ---

    /// Start of date range (ISO 8601) for calendar_list, or event start for calendar_create.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,

    /// End of date range (ISO 8601) for calendar_list, or event end for calendar_create.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,

    /// Event start time (ISO 8601) for calendar_create/update. Alias for `from` in event context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,

    /// Event end time (ISO 8601) for calendar_create/update. Alias for `to` in event context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,

    /// Calendar ID for filtering or target calendar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_id: Option<String>,

    /// Location for calendar events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,

    /// Whether the event is all-day.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all_day: Option<bool>,

    // --- Reminders params ---

    /// Reminder list ID for filtering or target list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,

    /// Due date (ISO 8601) for reminders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,

    /// Priority (0=none, 1=high, 5=medium, 9=low) for reminders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,

    /// Whether to mark a reminder as completed (default: true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<bool>,

    /// Whether to include completed reminders in list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_completed: Option<bool>,

    // --- Notes params ---

    /// Note body content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,

    /// Notes folder name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,

    // --- Contacts params ---

    /// Search query for contacts_search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    /// Contact given (first) name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub given_name: Option<String>,

    /// Contact family (last) name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family_name: Option<String>,

    /// Contact organization name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,

    /// Contact phone numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_numbers: Option<Vec<String>>,

    /// Contact email addresses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emails: Option<Vec<String>>,
}

/// Output from the PIM tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PimOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// PIM tool — access macOS personal information via the Desktop Bridge.
#[derive(Clone)]
pub struct PimTool {
    client: DesktopBridgeClient,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl PimTool {
    pub fn new() -> Self {
        Self {
            client: DesktopBridgeClient::new(),
            approval_policy: None,
        }
    }

    /// Attach an approval policy to gate write operations.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }

    /// Check if the action is a write (mutating) operation.
    fn is_write_action(action: &str) -> bool {
        matches!(action,
            "calendar_create" | "calendar_update" | "calendar_delete" |
            "reminders_create" | "reminders_complete" | "reminders_delete" |
            "notes_create" | "notes_update" | "notes_delete" |
            "contacts_create" | "contacts_update" | "contacts_delete"
        )
    }

    /// Build a human-readable description for approval prompts.
    fn describe_action(args: &PimArgs) -> String {
        let target = args.title.as_deref()
            .or(args.query.as_deref())
            .or(args.given_name.as_deref())
            .or(args.id.as_deref())
            .unwrap_or("unknown");
        format!("{}: {}", args.action, target)
    }

    /// Check approval policy for write actions.
    async fn check_approval(&self, args: &PimArgs) -> Option<PimOutput> {
        if !Self::is_write_action(&args.action) {
            return None;
        }

        let policy = self.approval_policy.as_ref()?;
        let request = ActionRequest {
            // Reuse DesktopClick as the closest ActionType for PIM writes.
            // TODO: Add PIM-specific ActionTypes when the approval module is extended.
            action_type: ActionType::DesktopClick,
            target: Self::describe_action(args),
            agent_id: String::new(),
            context: format!("PIM write operation: {}", args.action),
            timestamp: chrono::Utc::now(),
        };

        let decision = policy.check(&request).await;
        match decision {
            ApprovalDecision::Allow => {
                policy.record(&request, &decision).await;
                None
            }
            ApprovalDecision::Deny { ref reason } => {
                policy.record(&request, &decision).await;
                Some(PimOutput {
                    success: false,
                    data: None,
                    message: Some(format!("Action denied: {reason}")),
                })
            }
            ApprovalDecision::Ask { ref prompt } => {
                Some(PimOutput {
                    success: false,
                    data: Some(serde_json::json!({
                        "approval_required": true,
                        "prompt": prompt,
                    })),
                    message: Some(format!("Approval required: {prompt}")),
                })
            }
        }
    }
}

impl Default for PimTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AlephTool for PimTool {
    const NAME: &'static str = "pim";
    const DESCRIPTION: &'static str = r#"Access macOS personal information: Calendar, Reminders, Notes, and Contacts.

Requires the Aleph Desktop Bridge (macOS app). Read operations are automatic; write operations require user approval.

Calendar:
- calendar_list: List events in date range. Requires: from, to (ISO 8601). Optional: calendar_id
- calendar_get: Get event details. Requires: id
- calendar_create: Create event. Requires: title, start, end. Optional: calendar_id, location, notes, all_day
- calendar_update: Modify event. Requires: id. Optional: title, start, end, location, notes
- calendar_delete: Delete event. Requires: id
- calendar_calendars: List all calendars.

Reminders:
- reminders_list: List reminders. Optional: list_id, include_completed
- reminders_get: Get reminder details. Requires: id
- reminders_create: Create reminder. Requires: title. Optional: list_id, due_date, priority (1=high,5=med,9=low), notes
- reminders_complete: Mark complete/incomplete. Requires: id. Optional: completed (default true)
- reminders_delete: Delete reminder. Requires: id
- reminders_lists: List all reminder lists.

Notes:
- notes_list: List notes. Optional: folder
- notes_get: Get note content. Requires: id
- notes_create: Create note. Requires: title. Optional: body, folder
- notes_update: Modify note. Requires: id. Optional: title, body
- notes_delete: Delete note. Requires: id
- notes_folders: List all folders.

Contacts:
- contacts_search: Search contacts. Requires: query
- contacts_get: Get contact details. Requires: id
- contacts_create: Create contact. Requires: given_name. Optional: family_name, organization, phone_numbers, emails, notes
- contacts_update: Modify contact. Requires: id. Optional: given_name, family_name, organization, phone_numbers, emails, notes
- contacts_delete: Delete contact. Requires: id
- contacts_groups: List contact groups."#;

    type Args = PimArgs;
    type Output = PimOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"{"action":"calendar_list","from":"2026-02-27T00:00:00+08:00","to":"2026-03-06T00:00:00+08:00"}"#.into(),
            r#"{"action":"calendar_create","title":"Team Meeting","start":"2026-02-28T10:00:00+08:00","end":"2026-02-28T11:00:00+08:00","location":"Room A"}"#.into(),
            r#"{"action":"reminders_create","title":"Buy groceries","list_id":null,"due_date":"2026-02-28T18:00:00+08:00","priority":1}"#.into(),
            r#"{"action":"notes_create","title":"Meeting Notes","body":"Discussion points..."}"#.into(),
            r#"{"action":"contacts_search","query":"John"}"#.into(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Check approval for write operations
        if let Some(denied) = self.check_approval(&args).await {
            return Ok(denied);
        }

        // Check bridge availability
        if !self.client.is_available() {
            return Ok(PimOutput {
                success: false,
                data: None,
                message: Some(
                    "Desktop bridge not connected. PIM features require the Aleph macOS app to be running.".into(),
                ),
            });
        }

        // Build desktop request from args
        let request = match build_pim_request(&args) {
            Ok(r) => r,
            Err(msg) => {
                return Ok(PimOutput {
                    success: false,
                    data: None,
                    message: Some(msg),
                });
            }
        };

        // Send to bridge and return result
        match self.client.send(request).await {
            Ok(result) => Ok(PimOutput {
                success: true,
                data: Some(result),
                message: None,
            }),
            Err(e) => Ok(PimOutput {
                success: false,
                data: None,
                message: Some(e.to_string()),
            }),
        }
    }
}

/// Convert PimArgs to the corresponding DesktopRequest variant.
fn build_pim_request(args: &PimArgs) -> std::result::Result<DesktopRequest, String> {
    let req = match args.action.as_str() {
        // Calendar
        "calendar_list" => {
            let from = args.from.clone().ok_or("calendar_list requires 'from' param")?;
            let to = args.to.clone().ok_or("calendar_list requires 'to' param")?;
            DesktopRequest::PimCalendarList { from, to, calendar_id: args.calendar_id.clone() }
        }
        "calendar_get" => {
            let id = args.id.clone().ok_or("calendar_get requires 'id' param")?;
            DesktopRequest::PimCalendarGet { id }
        }
        "calendar_create" => {
            let title = args.title.clone().ok_or("calendar_create requires 'title' param")?;
            let start = args.start.clone().ok_or("calendar_create requires 'start' param")?;
            let end = args.end.clone().ok_or("calendar_create requires 'end' param")?;
            DesktopRequest::PimCalendarCreate {
                title, start, end,
                calendar_id: args.calendar_id.clone(),
                location: args.location.clone(),
                notes: args.notes.clone(),
                all_day: args.all_day,
            }
        }
        "calendar_update" => {
            let id = args.id.clone().ok_or("calendar_update requires 'id' param")?;
            DesktopRequest::PimCalendarUpdate {
                id, title: args.title.clone(), start: args.start.clone(),
                end: args.end.clone(), location: args.location.clone(),
                notes: args.notes.clone(),
            }
        }
        "calendar_delete" => {
            let id = args.id.clone().ok_or("calendar_delete requires 'id' param")?;
            DesktopRequest::PimCalendarDelete { id }
        }
        "calendar_calendars" => DesktopRequest::PimCalendarCalendars,

        // Reminders
        "reminders_list" => {
            DesktopRequest::PimRemindersList {
                list_id: args.list_id.clone(),
                include_completed: args.include_completed,
            }
        }
        "reminders_get" => {
            let id = args.id.clone().ok_or("reminders_get requires 'id' param")?;
            DesktopRequest::PimRemindersGet { id }
        }
        "reminders_create" => {
            let title = args.title.clone().ok_or("reminders_create requires 'title' param")?;
            DesktopRequest::PimRemindersCreate {
                title, list_id: args.list_id.clone(),
                due_date: args.due_date.clone(), priority: args.priority,
                notes: args.notes.clone(),
            }
        }
        "reminders_complete" => {
            let id = args.id.clone().ok_or("reminders_complete requires 'id' param")?;
            DesktopRequest::PimRemindersComplete {
                id, completed: args.completed.unwrap_or(true),
            }
        }
        "reminders_delete" => {
            let id = args.id.clone().ok_or("reminders_delete requires 'id' param")?;
            DesktopRequest::PimRemindersDelete { id }
        }
        "reminders_lists" => DesktopRequest::PimRemindersLists,

        // Notes
        "notes_list" => DesktopRequest::PimNotesList { folder: args.folder.clone() },
        "notes_get" => {
            let id = args.id.clone().ok_or("notes_get requires 'id' param")?;
            DesktopRequest::PimNotesGet { id }
        }
        "notes_create" => {
            let title = args.title.clone().ok_or("notes_create requires 'title' param")?;
            DesktopRequest::PimNotesCreate {
                title, body: args.body.clone(), folder: args.folder.clone(),
            }
        }
        "notes_update" => {
            let id = args.id.clone().ok_or("notes_update requires 'id' param")?;
            DesktopRequest::PimNotesUpdate {
                id, title: args.title.clone(), body: args.body.clone(),
            }
        }
        "notes_delete" => {
            let id = args.id.clone().ok_or("notes_delete requires 'id' param")?;
            DesktopRequest::PimNotesDelete { id }
        }
        "notes_folders" => DesktopRequest::PimNotesFolders,

        // Contacts
        "contacts_search" => {
            let query = args.query.clone().ok_or("contacts_search requires 'query' param")?;
            DesktopRequest::PimContactsSearch { query }
        }
        "contacts_get" => {
            let id = args.id.clone().ok_or("contacts_get requires 'id' param")?;
            DesktopRequest::PimContactsGet { id }
        }
        "contacts_create" => {
            let given_name = args.given_name.clone().ok_or("contacts_create requires 'given_name' param")?;
            DesktopRequest::PimContactsCreate {
                given_name, family_name: args.family_name.clone(),
                organization: args.organization.clone(), notes: args.notes.clone(),
                phone_numbers: args.phone_numbers.clone(), emails: args.emails.clone(),
            }
        }
        "contacts_update" => {
            let id = args.id.clone().ok_or("contacts_update requires 'id' param")?;
            DesktopRequest::PimContactsUpdate {
                id, given_name: args.given_name.clone(), family_name: args.family_name.clone(),
                organization: args.organization.clone(), notes: args.notes.clone(),
                phone_numbers: args.phone_numbers.clone(), emails: args.emails.clone(),
            }
        }
        "contacts_delete" => {
            let id = args.id.clone().ok_or("contacts_delete requires 'id' param")?;
            DesktopRequest::PimContactsDelete { id }
        }
        "contacts_groups" => DesktopRequest::PimContactsGroups,

        other => return Err(format!("Unknown PIM action: '{}'. See tool description for valid actions.", other)),
    };
    Ok(req)
}
```

**Step 2: Add module declaration to mod.rs**

In `core/src/builtin_tools/mod.rs`, add:

```rust
pub mod pim;
pub use pim::{PimArgs, PimOutput, PimTool};
```

**Step 3: Build and verify**

```bash
cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | tail -10
```

Expected: Compiles successfully.

**Step 4: Commit**

```bash
git add core/src/builtin_tools/pim.rs core/src/builtin_tools/mod.rs
git commit -m "tools: add PimTool for macOS Calendar/Reminders/Notes/Contacts"
```

---

## Task 9: Register PimTool in BuiltinToolRegistry

**Files:**
- Modify: `core/src/executor/builtin_registry/registry.rs` (3 locations)

**Step 1: Add pim_tool field to struct**

In `core/src/executor/builtin_registry/registry.rs`, add to the `BuiltinToolRegistry` struct (around line 55, after `desktop_tool`):

```rust
    pub(crate) pim_tool: PimTool,
```

**Step 2: Initialize and register in with_config()**

In the `with_config()` method, after `let desktop_tool = DesktopTool::new();` (around line 98):

```rust
        let pim_tool = PimTool::new();
```

After the `desktop` tools.insert block (around line 208):

```rust
        tools.insert(
            "pim".to_string(),
            UnifiedTool::new(
                "builtin:pim",
                "pim",
                PimTool::DESCRIPTION,
                ToolSource::Builtin,
            ),
        );
```

Make sure to include `pim_tool` in the struct constructor.

**Step 3: Add execute_tool match arm**

In the `execute_tool` method, add after the `"desktop"` arm (around line 434):

```rust
            "pim" => Box::pin(async move { self.pim_tool.call_json(arguments).await }),
```

**Step 4: Add import**

At the top of `registry.rs`, ensure the import exists:

```rust
use crate::builtin_tools::{PimTool, /* ...existing imports... */};
```

**Step 5: Build and run full test suite**

```bash
cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | tail -5
cd /Users/zouguojun/Workspace/Aleph/core && cargo test 2>&1 | tail -10
```

Expected: Build succeeds, existing tests pass.

**Step 6: Commit**

```bash
git add core/src/executor/builtin_registry/registry.rs
git commit -m "registry: register PimTool in BuiltinToolRegistry"
```

---

## Task 10: Add PIM Unit Tests (Rust)

**Files:**
- Create: `core/src/builtin_tools/pim_tests.rs`
- Modify: `core/src/builtin_tools/pim.rs` (add `#[cfg(test)] mod tests;` at bottom)

**Step 1: Create test file**

Create `core/src/builtin_tools/pim_tests.rs`:

```rust
use super::*;

#[test]
fn test_is_write_action() {
    // Write actions
    assert!(PimTool::is_write_action("calendar_create"));
    assert!(PimTool::is_write_action("calendar_update"));
    assert!(PimTool::is_write_action("calendar_delete"));
    assert!(PimTool::is_write_action("reminders_create"));
    assert!(PimTool::is_write_action("reminders_complete"));
    assert!(PimTool::is_write_action("reminders_delete"));
    assert!(PimTool::is_write_action("notes_create"));
    assert!(PimTool::is_write_action("notes_update"));
    assert!(PimTool::is_write_action("notes_delete"));
    assert!(PimTool::is_write_action("contacts_create"));
    assert!(PimTool::is_write_action("contacts_update"));
    assert!(PimTool::is_write_action("contacts_delete"));

    // Read actions
    assert!(!PimTool::is_write_action("calendar_list"));
    assert!(!PimTool::is_write_action("calendar_get"));
    assert!(!PimTool::is_write_action("calendar_calendars"));
    assert!(!PimTool::is_write_action("reminders_list"));
    assert!(!PimTool::is_write_action("reminders_get"));
    assert!(!PimTool::is_write_action("reminders_lists"));
    assert!(!PimTool::is_write_action("notes_list"));
    assert!(!PimTool::is_write_action("notes_get"));
    assert!(!PimTool::is_write_action("notes_folders"));
    assert!(!PimTool::is_write_action("contacts_search"));
    assert!(!PimTool::is_write_action("contacts_get"));
    assert!(!PimTool::is_write_action("contacts_groups"));

    // Unknown
    assert!(!PimTool::is_write_action("unknown_action"));
}

#[test]
fn test_build_pim_request_calendar_list() {
    let args = PimArgs {
        action: "calendar_list".into(),
        from: Some("2026-02-27T00:00:00+08:00".into()),
        to: Some("2026-03-06T00:00:00+08:00".into()),
        calendar_id: None,
        id: None, title: None, notes: None, start: None, end: None,
        location: None, all_day: None, list_id: None, due_date: None,
        priority: None, completed: None, include_completed: None,
        body: None, folder: None, query: None, given_name: None,
        family_name: None, organization: None, phone_numbers: None, emails: None,
    };
    let req = build_pim_request(&args);
    assert!(req.is_ok());
    assert!(matches!(req.unwrap(), DesktopRequest::PimCalendarList { .. }));
}

#[test]
fn test_build_pim_request_missing_required() {
    // calendar_list without 'from'
    let args = PimArgs {
        action: "calendar_list".into(),
        from: None, to: Some("2026-03-06".into()),
        calendar_id: None, id: None, title: None, notes: None,
        start: None, end: None, location: None, all_day: None,
        list_id: None, due_date: None, priority: None, completed: None,
        include_completed: None, body: None, folder: None, query: None,
        given_name: None, family_name: None, organization: None,
        phone_numbers: None, emails: None,
    };
    let req = build_pim_request(&args);
    assert!(req.is_err());
    assert!(req.unwrap_err().contains("'from'"));
}

#[test]
fn test_build_pim_request_unknown_action() {
    let args = PimArgs {
        action: "nonexistent".into(),
        from: None, to: None, calendar_id: None, id: None, title: None,
        notes: None, start: None, end: None, location: None, all_day: None,
        list_id: None, due_date: None, priority: None, completed: None,
        include_completed: None, body: None, folder: None, query: None,
        given_name: None, family_name: None, organization: None,
        phone_numbers: None, emails: None,
    };
    let req = build_pim_request(&args);
    assert!(req.is_err());
    assert!(req.unwrap_err().contains("Unknown PIM action"));
}

#[test]
fn test_describe_action() {
    let args = PimArgs {
        action: "calendar_create".into(),
        title: Some("Team Meeting".into()),
        from: None, to: None, calendar_id: None, id: None, notes: None,
        start: None, end: None, location: None, all_day: None,
        list_id: None, due_date: None, priority: None, completed: None,
        include_completed: None, body: None, folder: None, query: None,
        given_name: None, family_name: None, organization: None,
        phone_numbers: None, emails: None,
    };
    let desc = PimTool::describe_action(&args);
    assert_eq!(desc, "calendar_create: Team Meeting");
}
```

**Step 2: Add test module to pim.rs**

At the bottom of `core/src/builtin_tools/pim.rs`, add:

```rust
#[cfg(test)]
#[path = "pim_tests.rs"]
mod tests;
```

**Step 3: Run tests**

```bash
cd /Users/zouguojun/Workspace/Aleph/core && cargo test pim 2>&1 | tail -15
```

Expected: All PIM tests pass.

**Step 4: Commit**

```bash
git add core/src/builtin_tools/pim.rs core/src/builtin_tools/pim_tests.rs
git commit -m "tests: add PimTool unit tests for action classification and request building"
```

---

## Task 11: Full Build Verification and Final Commit

**Step 1: Build Rust core**

```bash
cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | tail -5
```

Expected: BUILD SUCCEEDED

**Step 2: Run Rust tests**

```bash
cd /Users/zouguojun/Workspace/Aleph/core && cargo test 2>&1 | tail -15
```

Expected: All tests pass.

**Step 3: Build macOS app**

```bash
cd /Users/zouguojun/Workspace/Aleph/apps/macos-native && xcodegen generate && xcodebuild -scheme Aleph -configuration Debug build 2>&1 | tail -5
```

Expected: BUILD SUCCEEDED

**Step 4: Verify no warnings**

```bash
cd /Users/zouguojun/Workspace/Aleph/core && cargo clippy 2>&1 | tail -10
```

Expected: No new warnings.
