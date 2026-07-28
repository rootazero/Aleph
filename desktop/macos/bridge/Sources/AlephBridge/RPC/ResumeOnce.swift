import Foundation

/// A `CheckedContinuation` that can be resumed at most once, from anywhere.
///
/// Resuming a checked continuation twice is not an error you catch — it is
/// `Fatal error: SWIFT TASK CONTINUATION MISUSE`, and it takes the **whole
/// helper process** with it. Every in-flight `desktop.*` RPC dies with it and the
/// crash counts against the supervisor's restart window, so a single malformed
/// argument on one call can disable the bridge.
///
/// That is not hypothetical. `screen.ocr` bridged Vision's completion handler
/// straight to a raw continuation and *also* resumed it from the `catch` around
/// `VNImageRequestHandler.perform`. Vision calls the request's completion handler
/// with the error before `perform` rethrows it, so **every** `perform` failure
/// resumed twice and killed the helper — reachable from tool input, since the
/// image can come from the caller.
///
/// The safe shape already existed in `SpeechSession` as a local `lock` +
/// `resumed` flag. This is that, once, so a third site cannot be written without
/// it. Taking the continuation *out* under the lock rather than setting a flag
/// beside it also drops the reference at the moment it becomes unusable.
final class ResumeOnce<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<T, Error>?

    init(_ continuation: CheckedContinuation<T, Error>) {
        self.continuation = continuation
    }

    /// Deliver a value. Returns `true` if this call was the one that resumed.
    @discardableResult
    func resume(returning value: T) -> Bool {
        guard let cont = claim() else { return false }
        cont.resume(returning: value)
        return true
    }

    /// Deliver an error. Returns `true` if this call was the one that resumed.
    @discardableResult
    func resume(throwing error: Error) -> Bool {
        guard let cont = claim() else { return false }
        cont.resume(throwing: error)
        return true
    }

    private func claim() -> CheckedContinuation<T, Error>? {
        lock.lock()
        defer { lock.unlock() }
        defer { continuation = nil }
        return continuation
    }
}
