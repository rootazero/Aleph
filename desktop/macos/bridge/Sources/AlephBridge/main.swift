import Foundation

// MARK: - Shared helpers

func iso8601Formatter() -> ISO8601DateFormatter {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime]
    return f
}

/// Parse an RFC 3339 timestamp sent by the Rust side.
///
/// `chrono` serialises `DateTime<Utc>` with 0, 3, 6 or 9 fractional-second
/// digits, but `ISO8601DateFormatter` only accepts 3 — so a sub-millisecond
/// timestamp is retried with the extra digits dropped instead of failing.
func parseISO8601(_ string: String) -> Date? {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let d = f.date(from: string) { return d }
    f.formatOptions = [.withInternetDateTime]
    if let d = f.date(from: string) { return d }

    guard let truncated = truncatingSubMillisecondDigits(string) else { return nil }
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return f.date(from: truncated)
}

/// Keep at most three fractional-second digits; nil when there is nothing to cut.
private func truncatingSubMillisecondDigits(_ string: String) -> String? {
    guard let dot = string.firstIndex(of: ".") else { return nil }
    let digitsStart = string.index(after: dot)
    var digitsEnd = digitsStart
    while digitsEnd < string.endIndex, string[digitsEnd].isNumber {
        digitsEnd = string.index(after: digitsEnd)
    }
    let digits = string[digitsStart..<digitsEnd]
    guard digits.count > 3 else { return nil }
    return String(string[..<digitsStart]) + String(digits.prefix(3)) + String(string[digitsEnd...])
}

func escapeAppleScript(_ s: String) -> String {
    var result = s
    result = result.replacingOccurrences(of: "\\", with: "\\\\")
    result = result.replacingOccurrences(of: "\"", with: "\\\"")
    result = result.replacingOccurrences(of: "\n", with: "\\n")
    result = result.replacingOccurrences(of: "\r", with: "\\r")
    return result
}

// MARK: - Entry point
//
// JSON-RPC over stdin/stdout is the only mode: the Rust client
// (desktop/shared/src/bridge/client.rs) spawns this helper with zero arguments.
// The process exits on stdin EOF or parent death (see `ParentWatch`); there is
// no shutdown RPC.

// Top-level `await` requires an `@main` entry point in Swift, so run an
// unstructured Task and block the main thread on a dispatch semaphore. Nothing
// may be scheduled on the main queue from here on — it never drains again.
let done = DispatchSemaphore(value: 0)
Task {
    let router = Router()
    await registerBridgeHandlers(router)
    await registerCameraHandlers(router)
    await registerAudioHandlers(router)
    await registerSpeechHandlers(router)
    await registerOcrHandlers(router)
    await registerAxHandlers(router)
    await registerPermHandlers(router)
    await registerScreenCaptureHandlers(router)
    await registerPimHandlers(router)
    await ParentWatch().start()
    await Server(router: router).run()
    done.signal()
}
done.wait()
