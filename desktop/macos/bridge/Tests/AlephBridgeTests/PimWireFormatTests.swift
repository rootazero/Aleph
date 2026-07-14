import XCTest
@testable import AlephBridge

/// The `pim.*` wire types are only checked at runtime — a renamed or reshaped
/// field fails as a serde error on the Rust side, never at compile time. These
/// tests pin the JSON contract against
/// `aleph_protocol::desktop_bridge::methods::pim` (field names hand-copied from
/// the Rust structs) plus the two format conversions the bridge has to perform:
/// AppleScript's zone-less dates and Contacts' year-optional birthday.
final class PimWireFormatTests: XCTestCase {
    // MARK: - Result shapes

    func testNoteInfoKeysMatchRust() throws {
        let note = NoteInfo(
            id: "x-coredata://1",
            title: "t",
            folder: "Notes",
            modified_at: "2026-07-14T10:00:00Z",
            snippet: "s"
        )
        XCTAssertEqual(
            try encodedKeys(note),
            ["id", "title", "folder", "modified_at", "snippet"]
        )
    }

    func testCalendarEventOmitsNilOptionalsAndNamesTheRestLikeRust() throws {
        // Swift's synthesised `encode(to:)` calls `encodeIfPresent` for an
        // Optional, so a nil is *omitted*, not written as null. That is
        // on-contract: `location` / `notes` are `Option<String>` on the Rust
        // side, and serde reads a missing Option field as `None`.
        let bare = CalendarEvent(
            id: "e1",
            title: "standup",
            calendar_id: "c1",
            start: "2026-07-14T10:00:00Z",
            end: "2026-07-14T10:15:00Z",
            all_day: false,
            location: nil,
            notes: nil
        )
        XCTAssertEqual(
            try encodedKeys(bare),
            ["id", "title", "calendar_id", "start", "end", "all_day"]
        )

        let full = CalendarEvent(
            id: "e1",
            title: "standup",
            calendar_id: "c1",
            start: "2026-07-14T10:00:00Z",
            end: "2026-07-14T10:15:00Z",
            all_day: false,
            location: "Room 1",
            notes: "bring laptop"
        )
        XCTAssertEqual(
            try encodedKeys(full),
            ["id", "title", "calendar_id", "start", "end", "all_day", "location", "notes"]
        )
    }

    func testReminderOmitsNilOptionalsAndNamesTheRestLikeRust() throws {
        let bare = Reminder(
            id: "r1",
            title: "buy milk",
            list_id: "l1",
            completed: false,
            due_date: nil,
            priority: 0,
            notes: nil
        )
        XCTAssertEqual(
            try encodedKeys(bare),
            ["id", "title", "list_id", "completed", "priority"]
        )

        let full = Reminder(
            id: "r1",
            title: "buy milk",
            list_id: "l1",
            completed: false,
            due_date: "2026-07-14T10:00:00Z",
            priority: 0,
            notes: "semi-skimmed"
        )
        XCTAssertEqual(
            try encodedKeys(full),
            ["id", "title", "list_id", "completed", "due_date", "priority", "notes"]
        )
    }

    func testContactDetailOmitsNilOptionalsAndNamesTheRestLikeRust() throws {
        let bare = ContactDetail(
            id: "c1",
            name: "Ada",
            emails: [LabeledValue(label: "home", value: "a@b.c")],
            phones: [],
            addresses: [],
            organization: nil,
            job_title: nil,
            birthday: nil,
            notes: nil
        )
        // The three Vec fields are non-Option in Rust, so they must survive even
        // when empty — an omitted `phones` would be a hard serde error there.
        XCTAssertEqual(try encodedKeys(bare), ["id", "name", "emails", "phones", "addresses"])

        let full = ContactDetail(
            id: "c1",
            name: "Ada",
            emails: [],
            phones: [],
            addresses: [],
            organization: "Analytical Engines",
            job_title: "Programmer",
            birthday: "1815-12-10",
            notes: "n"
        )
        XCTAssertEqual(
            try encodedKeys(full),
            [
                "id", "name", "emails", "phones", "addresses",
                "organization", "job_title", "birthday", "notes",
            ]
        )
    }

    func testCalendarInfoKeysMatchRust() throws {
        let info = CalendarInfo(id: "c1", title: "Work", read_only: false, color: "#FF0000")
        XCTAssertEqual(try encodedKeys(info), ["id", "title", "read_only", "color"])
    }

    // MARK: - Param shapes

    func testNotesListParamsDecodesOmittedFolder() throws {
        let args = try decode(NotesListParams.self, from: "{}")
        XCTAssertNil(args.folder)
    }

    func testRemindersListParamsDecodesRustPayload() throws {
        let args = try decode(RemindersListParams.self, from: #"{"include_completed":true}"#)
        XCTAssertTrue(args.include_completed)
        XCTAssertNil(args.list_id)
    }

    func testCalendarEventsParamsDecodesChronoTimestamps() throws {
        let args = try decode(
            CalendarEventsParams.self,
            from: #"{"from":"2026-07-14T10:00:00.123456789Z","to":"2026-07-21T10:00:00Z"}"#
        )
        XCTAssertNotNil(parseISO8601(args.from))
        XCTAssertNotNil(parseISO8601(args.to))
        XCTAssertNil(args.calendar_id)
    }

    // MARK: - Timestamp parsing

    /// chrono's `SecondsFormat::AutoSi` emits 0, 3, 6 or 9 fractional digits;
    /// ISO8601DateFormatter natively accepts only 0 or 3.
    func testParseISO8601AcceptsEveryChronoFraction() throws {
        let whole = try XCTUnwrap(parseISO8601("2026-07-14T10:00:00Z"))

        for stamp in [
            "2026-07-14T10:00:00.123Z",
            "2026-07-14T10:00:00.123456Z",
            "2026-07-14T10:00:00.123456789Z",
        ] {
            let parsed = try XCTUnwrap(parseISO8601(stamp), "failed to parse \(stamp)")
            XCTAssertEqual(
                parsed.timeIntervalSince1970,
                whole.timeIntervalSince1970,
                accuracy: 1.0,
                "\(stamp) parsed to the wrong second"
            )
        }
    }

    func testParseISO8601RejectsZonelessStamp() {
        // What AppleScript hands us — and what chrono's DateTime<Utc> rejects.
        XCTAssertNil(parseISO8601("2026-07-14T21:30:00"))
    }

    // MARK: - Format conversions

    func testAppleScriptDateBecomesUtcRfc3339() throws {
        let local = DateFormatter()
        local.locale = Locale(identifier: "en_US_POSIX")
        local.timeZone = TimeZone.current
        local.dateFormat = "yyyy-MM-dd'T'HH:mm:ss"
        let expected = try XCTUnwrap(local.date(from: "2026-07-14T21:30:00"))

        let converted = try appleScriptDateToRfc3339("2026-07-14T21:30:00")

        XCTAssertTrue(converted.hasSuffix("Z"), "expected a UTC stamp, got \(converted)")
        XCTAssertEqual(parseISO8601(converted), expected)
    }

    func testAppleScriptDateRejectsGarbage() {
        XCTAssertThrowsError(try appleScriptDateToRfc3339("not a date"))
    }

    func testContactBirthdayUsesNaiveDateFormat() {
        var components = DateComponents()
        components.year = 1990
        components.month = 5
        components.day = 7
        XCTAssertEqual(contactBirthdayString(components), "1990-05-07")
    }

    func testContactBirthdayWithoutYearIsOmitted() {
        // Contacts allows a year-less birthday; chrono's NaiveDate cannot hold one.
        var components = DateComponents()
        components.month = 5
        components.day = 7
        XCTAssertNil(contactBirthdayString(components))
        XCTAssertNil(contactBirthdayString(nil))
    }

    // MARK: - Integer transport

    /// A handler's result does not go out as the struct encoded it: it is turned
    /// into a `JSONValue` by `encodeCodable`, and `JSONValue` carries every number
    /// as a `Double`. `Reminder.priority` is a `u8` on the Rust side and
    /// `ReminderList.count` a `u32` — if that round-trip re-emitted them as `5.0`,
    /// serde would reject the whole payload at runtime, which no type-level check
    /// on either side would ever catch.
    func testIntegersSurviveTheJSONValueRoundTripAsIntegers() throws {
        let list = ReminderList(id: "l1", title: "Inbox", count: 5)
        let wire = try Codec.encode(encodeCodable(list))
        let text = try XCTUnwrap(String(data: wire, encoding: .utf8))

        XCTAssertTrue(text.contains("\"count\":5"), "count must stay an integer, got: \(text)")
        XCTAssertFalse(text.contains("5.0"), "a fractional count would fail serde's u32: \(text)")
    }

    // MARK: - Helpers

    private func encodedKeys<T: Encodable>(_ value: T) throws -> Set<String> {
        let data = try JSONEncoder().encode(value)
        let object = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
        return Set(object.keys)
    }

    private func decode<T: Decodable>(_ type: T.Type, from json: String) throws -> T {
        let data = try XCTUnwrap(json.data(using: .utf8))
        return try JSONDecoder().decode(type, from: data)
    }
}
