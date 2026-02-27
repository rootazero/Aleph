import Foundation
import os

/// Service for interacting with macOS Notes.app via AppleScript (osascript).
///
/// Notes.app has no native Swift framework, so all operations are performed
/// by executing AppleScript through `/usr/bin/osascript`. Each method accepts
/// `[String: AnyCodable]` params and returns
/// `Result<AnyCodable, BridgeServer.HandlerError>`, matching the
/// `BridgeServer.Handler` signature.
final class NotesService {

    // MARK: - Singleton

    static let shared = NotesService()

    // MARK: - Properties

    private let logger = Logger(subsystem: "com.aleph.app", category: "NotesService")

    /// Delimiter between fields within a single record.
    private let fieldDelimiter = "|||"

    /// Delimiter between records (lines).
    private let recordDelimiter = "\n"

    private init() {}

    // MARK: - List Notes

    /// List notes, optionally filtered by folder name.
    ///
    /// Params:
    /// - `folder` (optional): Folder name to filter by.
    ///
    /// Returns: `{ "notes": [{ "id", "title", "modified" }] }`
    func listNotes(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        let folderName = params["folder"]?.stringValue

        let target: String
        if let folder = folderName {
            target = "folder \"\(folder.escapeAppleScript())\""
        } else {
            target = "default account"
        }

        let script = """
        tell application "Notes"
            set output to ""
            repeat with n in notes of \(target)
                set noteId to id of n
                set noteTitle to name of n
                set noteMod to modification date of n
                set output to output & noteId & "\(fieldDelimiter)" & noteTitle & "\(fieldDelimiter)" & (noteMod as text) & "\n"
            end repeat
            return output
        end tell
        """

        switch runAppleScript(script) {
        case .failure(let err):
            return .failure(err)
        case .success(let output):
            let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else {
                return .success(AnyCodable(["notes": AnyCodable([AnyCodable]())]))
            }

            let lines = trimmed.components(separatedBy: recordDelimiter)
            var notes: [AnyCodable] = []

            for line in lines {
                let line = line.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !line.isEmpty else { continue }
                let fields = line.components(separatedBy: fieldDelimiter)
                guard fields.count >= 3 else { continue }

                let dict: [String: AnyCodable] = [
                    "id": AnyCodable(fields[0]),
                    "title": AnyCodable(fields[1]),
                    "modified": AnyCodable(fields[2]),
                ]
                notes.append(AnyCodable(dict))
            }

            return .success(AnyCodable(["notes": AnyCodable(notes)]))
        }
    }

    // MARK: - Get Note

    /// Get a single note by its identifier.
    ///
    /// Params:
    /// - `id` (required): Note identifier string (e.g. "x-coredata://...").
    ///
    /// Returns: `{ "note": { "id", "title", "body", "modified", "folder" } }`
    func getNote(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let noteId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        let script = """
        tell application "Notes"
            set n to first note whose id is "\(noteId.escapeAppleScript())"
            set noteId to id of n
            set noteTitle to name of n
            set noteBody to plaintext of n
            set noteMod to modification date of n
            set noteFolder to name of container of n
            return noteId & "\(fieldDelimiter)" & noteTitle & "\(fieldDelimiter)" & noteBody & "\(fieldDelimiter)" & (noteMod as text) & "\(fieldDelimiter)" & noteFolder
        end tell
        """

        switch runAppleScript(script) {
        case .failure(let err):
            // Check if the error message suggests the note was not found
            if err.message.contains("Can't get") || err.message.contains("Invalid index") {
                return .failure(.init(
                    code: PIMErrorCode.notFound,
                    message: "Note not found: \(noteId)"
                ))
            }
            return .failure(err)
        case .success(let output):
            let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
            let fields = trimmed.components(separatedBy: fieldDelimiter)
            guard fields.count >= 5 else {
                return .failure(.init(
                    code: PIMErrorCode.scriptError,
                    message: "Unexpected output format from Notes.app"
                ))
            }

            let dict: [String: AnyCodable] = [
                "id": AnyCodable(fields[0]),
                "title": AnyCodable(fields[1]),
                "body": AnyCodable(fields[2]),
                "modified": AnyCodable(fields[3]),
                "folder": AnyCodable(fields[4]),
            ]

            return .success(AnyCodable(["note": AnyCodable(dict)]))
        }
    }

    // MARK: - Create Note

    /// Create a new note in Notes.app.
    ///
    /// Params:
    /// - `title` (required): Note title.
    /// - `body` (optional): Note body text (plain text, newlines converted to `<br>`).
    /// - `folder` (optional): Target folder name. Defaults to default folder.
    ///
    /// Returns: `{ "note": { "id", "title", "body", "modified", "folder" } }`
    func createNote(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let title = params["title"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: title (string)"
            ))
        }

        let bodyText = params["body"]?.stringValue ?? ""
        let escapedTitle = title.escapeHTML()
        let escapedBody = bodyText.escapeHTML().replacingOccurrences(of: "\n", with: "<br>")
        let htmlBody = "<h1>\(escapedTitle)</h1><br>\(escapedBody)"

        let folderName = params["folder"]?.stringValue

        let target: String
        if let folder = folderName {
            target = "folder \"\(folder.escapeAppleScript())\""
        } else {
            target = "default account"
        }

        let script = """
        tell application "Notes"
            set newNote to make new note at \(target) with properties {name:"\(title.escapeAppleScript())", body:"\(htmlBody.escapeAppleScript())"}
            set noteId to id of newNote
            set noteTitle to name of newNote
            set noteBody to plaintext of newNote
            set noteMod to modification date of newNote
            set noteFolder to name of container of newNote
            return noteId & "\(fieldDelimiter)" & noteTitle & "\(fieldDelimiter)" & noteBody & "\(fieldDelimiter)" & (noteMod as text) & "\(fieldDelimiter)" & noteFolder
        end tell
        """

        switch runAppleScript(script) {
        case .failure(let err):
            return .failure(err)
        case .success(let output):
            let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
            let fields = trimmed.components(separatedBy: fieldDelimiter)
            guard fields.count >= 5 else {
                return .failure(.init(
                    code: PIMErrorCode.scriptError,
                    message: "Unexpected output format from Notes.app"
                ))
            }

            let dict: [String: AnyCodable] = [
                "id": AnyCodable(fields[0]),
                "title": AnyCodable(fields[1]),
                "body": AnyCodable(fields[2]),
                "modified": AnyCodable(fields[3]),
                "folder": AnyCodable(fields[4]),
            ]

            return .success(AnyCodable(["note": AnyCodable(dict)]))
        }
    }

    // MARK: - Update Note

    /// Update an existing note's title and/or body.
    ///
    /// Params:
    /// - `id` (required): Note identifier.
    /// - `title` (optional): New title.
    /// - `body` (optional): New body text (plain text, newlines converted to `<br>`).
    ///
    /// Returns: `{ "note": { "id", "title", "body", "modified", "folder" } }`
    func updateNote(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let noteId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        let newTitle = params["title"]?.stringValue
        let newBody = params["body"]?.stringValue

        // Build the update statements
        var updateStatements = ""
        if let title = newTitle {
            updateStatements += "set name of n to \"\(title.escapeAppleScript())\"\n"
        }
        if let body = newBody {
            let escapedBody = body.escapeHTML().replacingOccurrences(of: "\n", with: "<br>")
            // Rebuild the HTML body with the current or new title
            if let title = newTitle {
                let escapedTitle = title.escapeHTML()
                let htmlBody = "<h1>\(escapedTitle)</h1><br>\(escapedBody)"
                updateStatements += "set body of n to \"\(htmlBody.escapeAppleScript())\"\n"
            } else {
                // Keep existing title, just update body HTML
                let htmlBody = escapedBody
                updateStatements += "set body of n to \"\(htmlBody.escapeAppleScript())\"\n"
            }
        }

        guard !updateStatements.isEmpty else {
            // Nothing to update; just return the current note
            return getNote(params: params)
        }

        let script = """
        tell application "Notes"
            set n to first note whose id is "\(noteId.escapeAppleScript())"
            \(updateStatements)
            set noteId to id of n
            set noteTitle to name of n
            set noteBody to plaintext of n
            set noteMod to modification date of n
            set noteFolder to name of container of n
            return noteId & "\(fieldDelimiter)" & noteTitle & "\(fieldDelimiter)" & noteBody & "\(fieldDelimiter)" & (noteMod as text) & "\(fieldDelimiter)" & noteFolder
        end tell
        """

        switch runAppleScript(script) {
        case .failure(let err):
            if err.message.contains("Can't get") || err.message.contains("Invalid index") {
                return .failure(.init(
                    code: PIMErrorCode.notFound,
                    message: "Note not found: \(noteId)"
                ))
            }
            return .failure(err)
        case .success(let output):
            let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
            let fields = trimmed.components(separatedBy: fieldDelimiter)
            guard fields.count >= 5 else {
                return .failure(.init(
                    code: PIMErrorCode.scriptError,
                    message: "Unexpected output format from Notes.app"
                ))
            }

            let dict: [String: AnyCodable] = [
                "id": AnyCodable(fields[0]),
                "title": AnyCodable(fields[1]),
                "body": AnyCodable(fields[2]),
                "modified": AnyCodable(fields[3]),
                "folder": AnyCodable(fields[4]),
            ]

            return .success(AnyCodable(["note": AnyCodable(dict)]))
        }
    }

    // MARK: - Delete Note

    /// Delete a note by its identifier.
    ///
    /// Params:
    /// - `id` (required): Note identifier.
    ///
    /// Returns: `{ "deleted": true }`
    func deleteNote(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let noteId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        let script = """
        tell application "Notes"
            delete (first note whose id is "\(noteId.escapeAppleScript())")
        end tell
        """

        switch runAppleScript(script) {
        case .failure(let err):
            if err.message.contains("Can't get") || err.message.contains("Invalid index") {
                return .failure(.init(
                    code: PIMErrorCode.notFound,
                    message: "Note not found: \(noteId)"
                ))
            }
            return .failure(err)
        case .success:
            return .success(AnyCodable(["deleted": AnyCodable(true)]))
        }
    }

    // MARK: - List Folders

    /// List all folders in Notes.app.
    ///
    /// No params required.
    ///
    /// Returns: `{ "folders": [{ "id", "name" }] }`
    func listFolders(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        let script = """
        tell application "Notes"
            set output to ""
            repeat with f in folders
                set folderId to id of f
                set folderName to name of f
                set output to output & folderId & "\(fieldDelimiter)" & folderName & "\n"
            end repeat
            return output
        end tell
        """

        switch runAppleScript(script) {
        case .failure(let err):
            return .failure(err)
        case .success(let output):
            let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else {
                return .success(AnyCodable(["folders": AnyCodable([AnyCodable]())]))
            }

            let lines = trimmed.components(separatedBy: recordDelimiter)
            var folders: [AnyCodable] = []

            for line in lines {
                let line = line.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !line.isEmpty else { continue }
                let fields = line.components(separatedBy: fieldDelimiter)
                guard fields.count >= 2 else { continue }

                let dict: [String: AnyCodable] = [
                    "id": AnyCodable(fields[0]),
                    "name": AnyCodable(fields[1]),
                ]
                folders.append(AnyCodable(dict))
            }

            return .success(AnyCodable(["folders": AnyCodable(folders)]))
        }
    }

    // MARK: - AppleScript Execution

    /// Execute an AppleScript string via `/usr/bin/osascript` and return stdout.
    ///
    /// On non-zero exit, returns a `HandlerError` with `PIMErrorCode.scriptError`
    /// and the stderr output as the message.
    private func runAppleScript(_ script: String) -> Result<String, BridgeServer.HandlerError> {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        process.arguments = ["-e", script]

        let stdoutPipe = Pipe()
        let stderrPipe = Pipe()
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe

        do {
            try process.run()
        } catch {
            logger.error("Failed to launch osascript: \(error.localizedDescription)")
            return .failure(.init(
                code: PIMErrorCode.scriptError,
                message: "Failed to launch osascript: \(error.localizedDescription)"
            ))
        }

        process.waitUntilExit()

        let stdoutData = stdoutPipe.fileHandleForReading.readDataToEndOfFile()
        let stderrData = stderrPipe.fileHandleForReading.readDataToEndOfFile()
        let stdout = String(data: stdoutData, encoding: .utf8) ?? ""
        let stderr = String(data: stderrData, encoding: .utf8) ?? ""

        guard process.terminationStatus == 0 else {
            let errorMsg = stderr.trimmingCharacters(in: .whitespacesAndNewlines)
            logger.error("osascript failed (\(process.terminationStatus)): \(errorMsg)")
            return .failure(.init(
                code: PIMErrorCode.scriptError,
                message: errorMsg.isEmpty ? "AppleScript execution failed with exit code \(process.terminationStatus)" : errorMsg
            ))
        }

        return .success(stdout)
    }
}

// MARK: - String Escaping Extensions

private extension String {

    /// Escape special characters for embedding in an AppleScript quoted string.
    ///
    /// Escapes backslashes and double-quotes so the string can be safely
    /// placed inside `"..."` in AppleScript source.
    func escapeAppleScript() -> String {
        self
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
    }

    /// Escape HTML special characters for embedding in Notes.app HTML body.
    ///
    /// Notes.app stores note bodies as HTML, so user-supplied text must have
    /// `&`, `<`, `>`, and `"` escaped to their HTML entity equivalents.
    func escapeHTML() -> String {
        self
            .replacingOccurrences(of: "&", with: "&amp;")
            .replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
            .replacingOccurrences(of: "\"", with: "&quot;")
    }
}
