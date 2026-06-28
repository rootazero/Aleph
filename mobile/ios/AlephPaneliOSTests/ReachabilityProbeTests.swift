import Testing
import Network
@testable import AlephPaneliOS

@Suite struct ReachabilityProbeTests {
    enum ProbeTestError: Error { case noPort }

    @Test("closed port probes false")
    func closedPortFalse() async {
        let probe = NWReachabilityProbe(timeout: 0.3)
        let ok = await probe.probe(host: "127.0.0.1", port: 1)
        #expect(ok == false)
    }

    @Test("open port probes true")
    func openPortTrue() async throws {
        let listener = try NWListener(using: .tcp)
        listener.newConnectionHandler = { $0.cancel() }
        let port: UInt16 = try await withCheckedThrowingContinuation { cont in
            listener.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    if let p = listener.port?.rawValue {
                        cont.resume(returning: p)
                    } else {
                        cont.resume(throwing: ProbeTestError.noPort)
                    }
                case .failed(let e):
                    cont.resume(throwing: e)
                default:
                    break
                }
            }
            listener.start(queue: .global())
        }
        let probe = NWReachabilityProbe(timeout: 1.0)
        let ok = await probe.probe(host: "127.0.0.1", port: port)
        listener.cancel()
        #expect(ok == true)
    }
}
