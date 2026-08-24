import Testing
import Foundation
@testable import AlephPaneliOS

private struct StubProbe: ReachabilityProbing {
    let reachable: Bool
    func probe(_ target: PairingTarget) async -> Bool { reachable }
}

@MainActor
@Suite struct AppStateTests {
    @Test("env URL wins and is persisted")
    func envWins() async {
        let store = InMemoryConnectionStore()
        let state = AppState(store: store, probe: StubProbe(reachable: true),
                             envURL: { "http://127.0.0.1:18790/?token=aleph-env" })
        await state.resolve()
        #expect(state.screen == .connected(URL(string: "http://127.0.0.1:18790/?token=aleph-env")!))
        #expect(store.load() == URL(string: "http://127.0.0.1:18790/?token=aleph-env")!)
    }

    @Test("saved + reachable connects")
    func savedReachable() async {
        let store = InMemoryConnectionStore(URL(string: "http://box.lan:9000")!)
        let state = AppState(store: store, probe: StubProbe(reachable: true), envURL: { nil })
        await state.resolve()
        #expect(state.screen == .connected(URL(string: "http://box.lan:9000")!))
    }

    /// A stored target that no longer answers must land on the pairing screen
    /// with the origin spelled out. The old copy was "Last server unreachable",
    /// which named nothing at all — and the one fact the user needed was the
    /// port the shell had filled in, which the address field cannot show.
    @Test("saved + unreachable falls to pairing naming the origin")
    func savedUnreachable() async {
        let store = InMemoryConnectionStore(URL(string: "http://box.lan:9000")!)
        let state = AppState(store: store, probe: StubProbe(reachable: false), envURL: { nil })
        await state.resolve()
        guard case .pairing(let message) = state.screen, let message else {
            Issue.record("expected a pairing screen carrying a reason")
            return
        }
        #expect(message.contains("http://box.lan:9000"), "got: \(message)")
    }

    @Test("no env, empty store → pairing(nil)")
    func emptyStore() async {
        let state = AppState(store: InMemoryConnectionStore(), probe: StubProbe(reachable: true), envURL: { nil })
        await state.resolve()
        #expect(state.screen == .pairing(message: nil))
    }

    @Test("submit valid + reachable connects and persists")
    func submitReachable() async {
        let store = InMemoryConnectionStore()
        let state = AppState(store: store, probe: StubProbe(reachable: true), envURL: { nil })
        await state.submit("192.168.1.5")
        #expect(state.screen == .connected(URL(string: "http://192.168.1.5:18790")!))
        #expect(store.load() == URL(string: "http://192.168.1.5:18790")!)
    }

    @Test("submit invalid stays on pairing with message")
    func submitInvalid() async {
        let state = AppState(store: InMemoryConnectionStore(), probe: StubProbe(reachable: true), envURL: { nil })
        await state.submit("")
        if case .pairing(let message) = state.screen {
            #expect(message != nil)
        } else {
            Issue.record("expected pairing screen")
        }
    }

    @Test("submit valid + unreachable names the origin it dialled")
    func submitUnreachable() async {
        let state = AppState(store: InMemoryConnectionStore(), probe: StubProbe(reachable: false), envURL: { nil })
        await state.submit("box.lan:9000")
        guard case .pairing(let message) = state.screen, let message else {
            Issue.record("expected a pairing screen carrying a reason")
            return
        }
        #expect(message.contains("http://box.lan:9000"), "got: \(message)")
    }

    /// The exact reported failure, end to end through `AppState`: a user types a
    /// bare `https://` domain fronted by a CDN. The shell must dial :443 and,
    /// when that does not answer, tell them so — naming the origin rather than
    /// a port they never wrote.
    @Test("a typed https domain is dialled on 443, and said so when it fails")
    func typedHttpsDomainReportsPort443() async {
        let state = AppState(store: InMemoryConnectionStore(), probe: StubProbe(reachable: false), envURL: { nil })
        await state.submit("https://aleph.example.com")
        guard case .pairing(let message) = state.screen, let message else {
            Issue.record("expected a pairing screen carrying a reason")
            return
        }
        #expect(message.contains("https://aleph.example.com:443"), "got: \(message)")
        #expect(!message.contains("18790"), "the listener port must not be dialled here: \(message)")
    }

    @Test("requestReconfigure switches to pairing")
    func reconfigure() async {
        let state = AppState(store: InMemoryConnectionStore(URL(string: "http://a.lan:18790")!),
                             probe: StubProbe(reachable: true), envURL: { nil })
        await state.resolve()
        state.requestReconfigure()
        #expect(state.screen == .pairing(message: nil))
    }
}
