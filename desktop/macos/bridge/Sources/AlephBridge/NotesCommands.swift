import ArgumentParser
import Foundation

// MARK: - Notes (AppleScript-based)

extension AlephBridge {
    struct Notes: ParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Apple Notes operations",
            subcommands: [List.self, Get.self, Create.self, Update.self, Delete.self, Folders.self]
        )

        // Run an AppleScript and return the result string, or call printError on failure.
        static func runAppleScript(_ source: String) -> String {
            guard let script = NSAppleScript(source: source) else {
                printError("Failed to create AppleScript")
                return ""
            }
            var errorInfo: NSDictionary?
            let result = script.executeAndReturnError(&errorInfo)
            if let err = errorInfo {
                let msg = err[NSAppleScript.errorMessage] as? String ?? "Unknown AppleScript error"
                printError("AppleScript error: \(msg)")
            }
            return result.stringValue ?? ""
        }

        struct List: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List notes")

            @Option(name: .long, help: "Filter by folder name")
            var folder: String?

            func run() {
                let folderFilter: String
                if let f = folder {
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

                let result = Notes.runAppleScript(script)
                var notes: [[String: Any]] = []
                for line in result.split(separator: "\n", omittingEmptySubsequences: true) {
                    let parts = line.split(separator: "\t", maxSplits: 4, omittingEmptySubsequences: false)
                    if parts.count >= 5 {
                        notes.append([
                            "id": String(parts[0]),
                            "title": String(parts[1]),
                            "folder": String(parts[2]),
                            "modified_at": String(parts[3]),
                            "snippet": String(parts[4]),
                        ])
                    }
                }
                printJSON(["notes": notes])
            }
        }

        struct Get: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Get a note by ID")

            @Option(name: .long, help: "Note ID")
            var id: String

            func run() {
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

                let result = Notes.runAppleScript(script)
                let parts = result.split(separator: "\t", maxSplits: 4, omittingEmptySubsequences: false)
                if parts.count >= 5 {
                    printJSON([
                        "id": id,
                        "title": String(parts[0]),
                        "folder": String(parts[1]),
                        "body": String(parts[4]),
                        "created_at": String(parts[2]),
                        "modified_at": String(parts[3]),
                    ])
                } else {
                    printError("Note not found: \(id)")
                }
            }
        }

        struct Create: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Create a new note")

            @Option(name: .long, help: "Note title")
            var title: String = ""

            @Option(name: .long, help: "Note body (plain text or HTML)")
            var body: String = ""

            @Option(name: .long, help: "Folder name")
            var folder: String?

            func run() {
                let escapedTitle = escapeAppleScript(title)
                let escapedBody = escapeAppleScript(body)

                let folderTarget: String
                if let f = folder {
                    folderTarget = "in folder \"\(escapeAppleScript(f))\""
                } else {
                    folderTarget = ""
                }

                // Create note with HTML body. Title is set via <h1> or the name property.
                let htmlBody = "<h1>\(escapedTitle)</h1><br>\(escapedBody)"
                let script = """
                tell application "Notes"
                    set n to make new note \(folderTarget) with properties {body:"\(htmlBody)"}
                    set noteId to id of n
                    set noteTitle to name of n
                    set noteFolder to name of container of n
                    set noteBody to body of n
                    set creDate to creation date of n
                    set modDate to modification date of n
                    return noteId & tab & noteTitle & tab & noteFolder & tab & noteBody & tab & (creDate as «class isot» as string) & tab & (modDate as «class isot» as string)
                end tell
                """

                let result = Notes.runAppleScript(script)
                let parts = result.split(separator: "\t", maxSplits: 5, omittingEmptySubsequences: false)
                if parts.count >= 6 {
                    printJSON([
                        "id": String(parts[0]),
                        "title": String(parts[1]),
                        "folder": String(parts[2]),
                        "body": String(parts[3]),
                        "created_at": String(parts[4]),
                        "modified_at": String(parts[5]),
                    ])
                } else {
                    printError("Failed to create note")
                }
            }
        }

        struct Update: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Update an existing note")

            @Option(name: .long, help: "Note ID")
            var id: String

            @Option(name: .long, help: "New title")
            var title: String?

            @Option(name: .long, help: "New body")
            var body: String?

            func run() {
                let escapedId = escapeAppleScript(id)
                var setProps: [String] = []

                if let t = title {
                    let escaped = escapeAppleScript(t)
                    setProps.append("set name of n to \"\(escaped)\"")
                }
                if let b = body {
                    let escaped = escapeAppleScript(b)
                    setProps.append("set body of n to \"\(escaped)\"")
                }

                if setProps.isEmpty {
                    printError("No update properties specified (use --title or --body)")
                }

                let script = """
                tell application "Notes"
                    set n to first note whose id is "\(escapedId)"
                    \(setProps.joined(separator: "\n    "))
                    set noteTitle to name of n
                    set noteFolder to name of container of n
                    set noteBody to body of n
                    set creDate to creation date of n
                    set modDate to modification date of n
                    return noteTitle & tab & noteFolder & tab & noteBody & tab & (creDate as «class isot» as string) & tab & (modDate as «class isot» as string)
                end tell
                """

                let result = Notes.runAppleScript(script)
                let parts = result.split(separator: "\t", maxSplits: 4, omittingEmptySubsequences: false)
                if parts.count >= 5 {
                    printJSON([
                        "id": id,
                        "title": String(parts[0]),
                        "folder": String(parts[1]),
                        "body": String(parts[2]),
                        "created_at": String(parts[3]),
                        "modified_at": String(parts[4]),
                    ])
                } else {
                    printError("Failed to update note: \(id)")
                }
            }
        }

        struct Delete: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Delete a note")

            @Option(name: .long, help: "Note ID")
            var id: String

            func run() {
                let escapedId = escapeAppleScript(id)
                let script = """
                tell application "Notes"
                    delete (first note whose id is "\(escapedId)")
                    return "ok"
                end tell
                """
                _ = Notes.runAppleScript(script)
                printJSON(["deleted": true, "id": id])
            }
        }

        struct Folders: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List all note folders")

            func run() {
                let script = """
                tell application "Notes"
                    set output to ""
                    repeat with f in every folder
                        set fName to name of f
                        set fId to id of f
                        set fCount to count of notes of f
                        set output to output & fId & tab & fName & tab & fCount & linefeed
                    end repeat
                    return output
                end tell
                """

                let result = Notes.runAppleScript(script)
                var folders: [[String: Any]] = []
                for line in result.split(separator: "\n", omittingEmptySubsequences: true) {
                    let parts = line.split(separator: "\t", maxSplits: 2, omittingEmptySubsequences: false)
                    if parts.count >= 3 {
                        folders.append([
                            "id": String(parts[0]),
                            "name": String(parts[1]),
                            "count": Int(parts[2]) ?? 0,
                        ])
                    }
                }
                printJSON(["folders": folders])
            }
        }
    }
}
