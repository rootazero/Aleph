import ArgumentParser
import Foundation

// MARK: - Helpers

func printJSON(_ value: Any) {
    if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
       let string = String(data: data, encoding: .utf8) {
        print(string)
    } else {
        printError("Failed to serialize JSON")
    }
}

func printError(_ message: String) -> Never {
    fputs(message + "\n", stderr)
    Foundation.exit(1)
}

/// ISO 8601 formatter for consistent date output.
func iso8601Formatter() -> ISO8601DateFormatter {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime]
    return f
}

/// Parse an ISO 8601 string into a Date.
func parseISO8601(_ string: String) -> Date? {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let d = f.date(from: string) { return d }
    // Try without fractional seconds
    f.formatOptions = [.withInternetDateTime]
    return f.date(from: string)
}

/// Escape a string for safe use inside AppleScript string literals.
func escapeAppleScript(_ s: String) -> String {
    return s.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
}

// MARK: - Root Command

struct AlephBridge: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "aleph-bridge",
        abstract: "macOS native API bridge for Aleph",
        subcommands: [Notes.self, Calendar.self, Reminders.self, Contacts.self, System.self]
    )
}

AlephBridge.main()
