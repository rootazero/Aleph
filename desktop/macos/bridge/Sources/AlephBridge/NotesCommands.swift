import Foundation

// MARK: - Notes (AppleScript-based)

/// errAEEventNotPermitted — AppleScript's "this process may not send Apple events
/// to that app" code, raised until the user grants Automation access.
private let appleEventNotPermitted = -1743

private let notesFmt = iso8601Formatter()

/// Parses the local-time, zone-less ISO 8601 string AppleScript's `«class isot»`
/// produces ("2026-07-14T21:30:00").
private let appleScriptDateParser: DateFormatter = {
    let f = DateFormatter()
    f.locale = Locale(identifier: "en_US_POSIX")
    f.timeZone = TimeZone.current
    f.dateFormat = "yyyy-MM-dd'T'HH:mm:ss"
    return f
}()

/// Re-stamp an AppleScript date as RFC 3339 UTC.
///
/// `«class isot»` carries no zone designator, which `chrono::DateTime<Utc>` on
/// the Rust side rejects outright — so the conversion has to happen here, not in
/// the script.
func appleScriptDateToRfc3339(_ raw: String) throws -> String {
    let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    if let date = appleScriptDateParser.date(from: trimmed) {
        return notesFmt.string(from: date)
    }
    if let date = parseISO8601(trimmed) {
        return notesFmt.string(from: date)
    }
    throw RpcError(
        code: -32003,
        message: "pim.notes: unparseable AppleScript date: \(trimmed)",
        data: nil
    )
}

/// Apple Notes exposes no framework API, so every operation drives the Notes app
/// over AppleScript. All entry points are synchronous and blocking; they must be
/// called from the serial PIM queue (see `PimHandlers.swift`), never from the
/// main thread — the RPC entry point parks it on a semaphore.
enum NotesCommands {
    static func list(folder: String?) throws -> NotesListResult {
        let folderFilter: String
        if let f = folder, !f.isEmpty {
            folderFilter = "of folder \"\(escapeAppleScript(f))\""
        } else {
            folderFilter = ""
        }

        // Get note properties via AppleScript.
        // Returns tab-separated lines: id \t name \t folder \t modification date \t body snippet
        let script = """
        tell application "Notes"
            set output to ""
            set noteList to every note \(folderFilter)
            repeat with n in noteList
                set noteId to id of n
                set noteTitle to name of n
                set noteFolder to name of container of n
                set modDate to modification date of n
                set noteBody to plaintext of n
                -- Truncate body snippet to 200 chars
                if length of noteBody > 200 then
                    set noteBody to text 1 thru 200 of noteBody
                end if
                -- Replace tabs and newlines in body snippet
                set tid to AppleScript's text item delimiters
                set AppleScript's text item delimiters to tab
                set noteBody to text items of noteBody
                set AppleScript's text item delimiters to " "
                set noteBody to noteBody as text
                set AppleScript's text item delimiters to return
                set noteBody to text items of noteBody
                set AppleScript's text item delimiters to " "
                set noteBody to noteBody as text
                set AppleScript's text item delimiters to linefeed
                set noteBody to text items of noteBody
                set AppleScript's text item delimiters to " "
                set noteBody to noteBody as text
                set AppleScript's text item delimiters to tid
                set output to output & noteId & tab & noteTitle & tab & noteFolder & tab & (modDate as «class isot» as string) & tab & noteBody & linefeed
            end repeat
            return output
        end tell
        """

        let result = try runAppleScript(script)
        var notes: [NoteInfo] = []
        for line in result.split(separator: "\n", omittingEmptySubsequences: true) {
            let parts = line.split(separator: "\t", maxSplits: 4, omittingEmptySubsequences: false)
            if parts.count >= 5 {
                notes.append(
                    NoteInfo(
                        id: String(parts[0]),
                        title: String(parts[1]),
                        folder: String(parts[2]),
                        modified_at: try appleScriptDateToRfc3339(String(parts[3])),
                        snippet: String(parts[4])
                    )
                )
            }
        }
        return NotesListResult(notes: notes)
    }

    static func get(id: String) throws -> NoteContent {
        let escapedId = escapeAppleScript(id)
        let script = """
        tell application "Notes"
            set n to first note whose id is "\(escapedId)"
            set noteTitle to name of n
            set noteFolder to name of container of n
            set noteBody to body of n
            set creDate to creation date of n
            set modDate to modification date of n
            return noteTitle & tab & noteFolder & tab & (creDate as «class isot» as string) & tab & (modDate as «class isot» as string) & tab & noteBody
        end tell
        """

        let result = try runAppleScript(script)
        let parts = result.split(separator: "\t", maxSplits: 4, omittingEmptySubsequences: false)
        guard parts.count >= 5 else {
            throw RpcError(code: -32003, message: "pim.notes.get: note not found: \(id)", data: nil)
        }
        return NoteContent(
            id: id,
            title: String(parts[0]),
            folder: String(parts[1]),
            body: String(parts[4]),
            created_at: try appleScriptDateToRfc3339(String(parts[2])),
            modified_at: try appleScriptDateToRfc3339(String(parts[3]))
        )
    }

    static func create(folder: String, title: String, body: String) throws -> NotesCreateResult {
        let escapedTitle = escapeAppleScript(title)
        let escapedBody = escapeAppleScript(body)

        let folderTarget = folder.isEmpty ? "" : "in folder \"\(escapeAppleScript(folder))\""

        // Create note with HTML body. Title is set via <h1> or the name property.
        let htmlBody = "<h1>\(escapedTitle)</h1><br>\(escapedBody)"
        let script = """
        tell application "Notes"
            set n to make new note \(folderTarget) with properties {body:"\(htmlBody)"}
            return id of n
        end tell
        """

        let result = try runAppleScript(script).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !result.isEmpty else {
            throw RpcError(code: -32003, message: "pim.notes.create: failed to create note", data: nil)
        }
        return NotesCreateResult(id: result)
    }

    static func update(id: String, title: String?, body: String?) throws {
        let escapedId = escapeAppleScript(id)
        var setProps: [String] = []

        if let t = title {
            setProps.append("set name of n to \"\(escapeAppleScript(t))\"")
        }
        if let b = body {
            setProps.append("set body of n to \"\(escapeAppleScript(b))\"")
        }

        guard !setProps.isEmpty else {
            throw RpcError(
                code: -32602,
                message: "pim.notes.update: neither 'title' nor 'body' was given",
                data: nil
            )
        }

        let script = """
        tell application "Notes"
            set n to first note whose id is "\(escapedId)"
            \(setProps.joined(separator: "\n    "))
            return "ok"
        end tell
        """
        _ = try runAppleScript(script)
    }

    static func delete(id: String) throws {
        let escapedId = escapeAppleScript(id)
        let script = """
        tell application "Notes"
            delete (first note whose id is "\(escapedId)")
            return "ok"
        end tell
        """
        _ = try runAppleScript(script)
    }

    static func folders() throws -> NotesFoldersResult {
        let script = """
        tell application "Notes"
            set output to ""
            repeat with f in every folder
                set output to output & name of f & linefeed
            end repeat
            return output
        end tell
        """

        let result = try runAppleScript(script)
        let folders = result
            .split(separator: "\n", omittingEmptySubsequences: true)
            .map { NotesFolderEntry(name: String($0)) }
        return NotesFoldersResult(folders: folders)
    }

    /// Run an AppleScript source and return its string result.
    ///
    /// AppleScript reports a denied Automation grant as errAEEventNotPermitted
    /// rather than a TCC error, so that one code is remapped to the structured
    /// permission error the Rust side already knows how to render.
    private static func runAppleScript(_ source: String) throws -> String {
        guard let script = NSAppleScript(source: source) else {
            throw RpcError(
                code: -32603,
                message: "pim.notes: failed to compile AppleScript",
                data: nil
            )
        }
        var errorInfo: NSDictionary?
        let result = script.executeAndReturnError(&errorInfo)
        if let err = errorInfo {
            let message = err[NSAppleScript.errorMessage] as? String ?? "unknown AppleScript error"
            let number = err[NSAppleScript.errorNumber] as? Int ?? 0
            if number == appleEventNotPermitted {
                throw RpcError(
                    code: -32001,
                    message: "permission denied: automation (Notes): \(message)",
                    data: try encodeCodable(Perm.guide(.automation))
                )
            }
            throw RpcError(
                code: -32003,
                message: "pim.notes: AppleScript error \(number): \(message)",
                data: nil
            )
        }
        return result.stringValue ?? ""
    }
}
