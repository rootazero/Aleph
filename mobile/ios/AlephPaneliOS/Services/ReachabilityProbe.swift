import Foundation
import Network

/// Whether a Gateway endpoint is currently accepting TCP connections. True
/// reachability, auth, and TLS are the webview's concern; this only answers
/// "is the port open" before we commit a navigation — mirrors the desktop lite
/// shell's pre-navigation probe (`connect_setup.rs::probe_reachable`).
protocol ReachabilityProbing {
    func probe(host: String, port: UInt16) async -> Bool
}

struct NWReachabilityProbe: ReachabilityProbing {
    let timeout: TimeInterval

    init(timeout: TimeInterval = 2.0) {
        self.timeout = timeout
    }

    func probe(host: String, port: UInt16) async -> Bool {
        guard let nwPort = NWEndpoint.Port(rawValue: port) else { return false }
        let connection = NWConnection(host: NWEndpoint.Host(host), port: nwPort, using: .tcp)
        let queue = DispatchQueue(label: "ai.aleph.panel.probe")

        return await withCheckedContinuation { continuation in
            // A small actor-free latch so we resume exactly once whether the
            // connection becomes ready, fails, or the timeout fires first.
            let resumed = ResumeOnce(continuation: continuation) {
                connection.cancel()
            }

            connection.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    resumed.fire(true)
                case .failed, .cancelled:
                    resumed.fire(false)
                default:
                    break
                }
            }
            connection.start(queue: queue)
            queue.asyncAfter(deadline: .now() + timeout) {
                resumed.fire(false)
            }
        }
    }
}

/// Resumes a continuation at most once and cancels the connection on first fire.
private final class ResumeOnce {
    private var done = false
    private let lock = NSLock()
    private let continuation: CheckedContinuation<Bool, Never>
    private let onResume: () -> Void

    init(continuation: CheckedContinuation<Bool, Never>, onResume: @escaping () -> Void) {
        self.continuation = continuation
        self.onResume = onResume
    }

    func fire(_ value: Bool) {
        lock.lock()
        defer { lock.unlock() }
        guard !done else { return }
        done = true
        onResume()
        continuation.resume(returning: value)
    }
}
