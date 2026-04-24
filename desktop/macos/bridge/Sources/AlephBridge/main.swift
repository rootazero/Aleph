import ArgumentParser
import Foundation

// MARK: - Shared helpers (reused by CLI subcommands)

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

func iso8601Formatter() -> ISO8601DateFormatter {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime]
    return f
}

func parseISO8601(_ string: String) -> Date? {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let d = f.date(from: string) { return d }
    f.formatOptions = [.withInternetDateTime]
    return f.date(from: string)
}

func escapeAppleScript(_ s: String) -> String {
    return s.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
}

// MARK: - Legacy CLI root command (preserved for pim.rs compatibility)
//
// Named `AlephBridge` (not renamed) because every *Commands.swift file uses
// `extension AlephBridge { struct <Domain>: ParsableCommand { ... } }`.
// The subcommands reference the nested types via their qualified names
// (AlephBridge.Notes etc.) to avoid ambiguity with Foundation's Calendar and
// the Swift stdlib's implicit System namespace.

struct AlephBridge: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "aleph-bridge",
        abstract: "macOS native API bridge for Aleph (legacy CLI mode)",
        subcommands: [
            AlephBridge.Notes.self,
            AlephBridge.Calendar.self,
            AlephBridge.Reminders.self,
            AlephBridge.Contacts.self,
            AlephBridge.System.self,
        ]
    )
}

// MARK: - Dual-mode entry point
//
// If any CLI argument is given (domain + action + flags), run the legacy
// ArgumentParser subcommand flow invoked by desktop/macos/src/pim.rs via
// the deprecated spawn-per-call `SwiftBridge` in desktop/shared/src/bridge/mod.rs.
//
// With zero arguments, enter JSON-RPC mode: bridge.{ping,handshake,shutdown}
// over stdin/stdout. This is what the new long-lived `SwiftBridge` client in
// desktop/shared/src/bridge/client.rs spawns.
//
// The CLI mode is scheduled for removal once every caller has migrated to
// JSON-RPC (tracked in Stage 6 cleanup of the codex-desktop plan).

if CommandLine.arguments.count > 1 {
    AlephBridge.main()
} else {
    // Top-level `await` requires an `@main` entry point in Swift, so run an
    // unstructured Task and block the main thread on a dispatch semaphore.
    let done = DispatchSemaphore(value: 0)
    Task {
        let router = Router()
        await registerBridgeHandlers(router)
        await registerCameraHandlers(router)
        await registerAudioHandlers(router)
        await registerSpeechHandlers(router)
        await ParentWatch().start()
        await Server(router: router).run()
        done.signal()
    }
    done.wait()
}
