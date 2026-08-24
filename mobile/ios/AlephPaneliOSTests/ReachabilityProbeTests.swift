import Testing
import Foundation
import Network
@testable import AlephPaneliOS

@Suite struct ReachabilityProbeTests {
    enum ProbeTestError: Error { case noPort }

    @Test("closed port probes false")
    func closedPortFalse() async throws {
        let probe = GatewayReadyProbe(timeout: 0.5)
        let closed = try target(port: 1)
        #expect(await probe.probe(closed) == false)
    }

    /// The regression that stranded the user, on the phone this time: a peer
    /// that completes the TCP handshake and then closes without speaking HTTP
    /// — exactly what a CDN edge, a load balancer, or a port-forward to nothing
    /// does. The old `NWReachabilityProbe` reported this as reachable, so the
    /// shell navigated to a dead origin and `WKWebView` showed its native
    /// "closed the connection" page with only the shake gesture as a way out.
    @Test("a socket that accepts but serves no HTTP is not reachable")
    func acceptThenCloseIsNotReachable() async throws {
        let (listener, port) = try await startStub(statusLine: nil)
        defer { listener.cancel() }
        let probe = GatewayReadyProbe(timeout: 2.0)
        let dead = try target(port: port)
        #expect(
            await probe.probe(dead) == false,
            "a socket that speaks no HTTP must not be reported as a live Gateway"
        )
    }

    /// The two statuses `aleph-server` itself answers with. 503 counts: it means
    /// the Gateway has bound the port and is still booting, and bouncing the
    /// user to the pairing screen during a normal restart would be wrong.
    @Test("a Gateway answering /ready is reachable")
    func gatewayIsReachable() async throws {
        for line in ["HTTP/1.1 200 OK", "HTTP/1.1 503 Service Unavailable"] {
            let (listener, port) = try await startStub(statusLine: line)
            defer { listener.cancel() }
            let probe = GatewayReadyProbe(timeout: 2.0)
            let live = try target(port: port)
            #expect(
                await probe.probe(live) == true,
                "\(line) at /ready must count as a live Gateway"
            )
        }
    }

    /// A live HTTP server that is not an Aleph Gateway (a stray web server, a
    /// proxy's own error page) is not a target worth navigating to.
    @Test("a non-Gateway HTTP responder is not reachable")
    func nonGatewayIsNotReachable() async throws {
        let (listener, port) = try await startStub(statusLine: "HTTP/1.1 404 Not Found")
        defer { listener.cancel() }
        let probe = GatewayReadyProbe(timeout: 2.0)
        let stray = try target(port: port)
        #expect(await probe.probe(stray) == false)
    }

    /// `/ready` is unauthenticated, so the probe must not carry the stored
    /// token — and it must reach `/ready` rather than whatever route the
    /// persisted target happens to point at.
    @Test("the probe strips the route and the token")
    func probeStripsRouteAndToken() async throws {
        let (listener, port, seen) = try await startRecordingStub()
        defer { listener.cancel() }
        let raw = "http://127.0.0.1:\(port)/settings?token=aleph-secret"
        let parsed = try PairingTarget.parse(raw).get()
        _ = await GatewayReadyProbe(timeout: 2.0).probe(parsed)

        let request = await seen.value()
        #expect(request?.contains("GET /ready") == true, "got: \(request ?? "<nothing>")")
        #expect(
            request?.contains("aleph-secret") == false,
            "the stored token must never ride along to an unauthenticated endpoint"
        )
    }

    // MARK: - helpers

    private func target(port: UInt16) throws -> PairingTarget {
        try PairingTarget.parse("http://127.0.0.1:\(port)").get()
    }

    /// Bind an ephemeral loopback listener. `statusLine == nil` accepts each
    /// connection and closes it without writing a byte.
    private func startStub(statusLine: String?) async throws -> (NWListener, UInt16) {
        let listener = try NWListener(using: .tcp)
        listener.newConnectionHandler = { connection in
            connection.start(queue: .global())
            guard let statusLine else {
                connection.cancel()
                return
            }
            // Drain the request before replying: closing on an unread receive
            // buffer surfaces as a reset on some stacks, which would make this
            // stub look like the dead peer it is meant to contrast with.
            connection.receive(minimumIncompleteLength: 1, maximumLength: 8192) { _, _, _, _ in
                let head = "\(statusLine)\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                connection.send(
                    content: Data(head.utf8),
                    completion: .contentProcessed { _ in connection.cancel() }
                )
            }
        }
        return (listener, try await ready(listener))
    }

    /// Like ``startStub(statusLine:)`` with a 200, but keeps the request bytes
    /// so a test can assert what was actually sent.
    private func startRecordingStub() async throws -> (NWListener, UInt16, Recorded) {
        let recorded = Recorded()
        let listener = try NWListener(using: .tcp)
        listener.newConnectionHandler = { connection in
            connection.start(queue: .global())
            connection.receive(minimumIncompleteLength: 1, maximumLength: 8192) { data, _, _, _ in
                if let data { recorded.set(String(decoding: data, as: UTF8.self)) }
                let head = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                connection.send(
                    content: Data(head.utf8),
                    completion: .contentProcessed { _ in connection.cancel() }
                )
            }
        }
        return (listener, try await ready(listener), recorded)
    }

    /// Start `listener` and resume once it is bound, yielding its port.
    /// The one-shot latch matters: `.failed` arriving after `.ready` would
    /// otherwise resume the continuation twice, which traps — a crashed suite
    /// rather than a failed assertion.
    private func ready(_ listener: NWListener) async throws -> UInt16 {
        try await withCheckedThrowingContinuation { continuation in
            let once = OnceFlag()
            listener.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    guard once.take() else { return }
                    if let port = listener.port?.rawValue {
                        continuation.resume(returning: port)
                    } else {
                        continuation.resume(throwing: ProbeTestError.noPort)
                    }
                case .failed(let error):
                    guard once.take() else { return }
                    continuation.resume(throwing: error)
                default:
                    break
                }
            }
            listener.start(queue: .global())
        }
    }
}

/// Thread-safe "first caller wins".
private final class OnceFlag: @unchecked Sendable {
    private var taken = false
    private let lock = NSLock()

    func take() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if taken { return false }
        taken = true
        return true
    }
}

/// Thread-safe single-slot recorder for bytes seen by a stub connection.
final class Recorded: @unchecked Sendable {
    private var stored: String?
    private let lock = NSLock()

    func set(_ value: String) {
        lock.lock()
        defer { lock.unlock() }
        stored = value
    }

    /// Polls briefly — the probe's `await` returns once the *response* is read,
    /// which can race the stub's receive callback finishing.
    func value() async -> String? {
        for _ in 0..<40 {
            lock.lock()
            let current = stored
            lock.unlock()
            if current != nil { return current }
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
        return nil
    }
}
