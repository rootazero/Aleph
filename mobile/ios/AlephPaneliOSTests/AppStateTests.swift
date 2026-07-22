import Testing
import Foundation
@testable import AlephPaneliOS

private struct StubProbe: ReachabilityProbing {
    let reachable: Bool
    func probe(host: String, port: UInt16) async -> Bool { reachable }
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

    @Test("saved + unreachable falls to pairing with message")
    func savedUnreachable() async {
        let store = InMemoryConnectionStore(URL(string: "http://box.lan:9000")!)
        let state = AppState(store: store, probe: StubProbe(reachable: false), envURL: { nil })
        await state.resolve()
        #expect(state.screen == .pairing(message: "Last server unreachable"))
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
        #expect(state.screen == .connected(URL(string: "https://192.168.1.5:18790")!))
        #expect(store.load() == URL(string: "https://192.168.1.5:18790")!)
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

    @Test("submit valid + unreachable shows not-reachable")
    func submitUnreachable() async {
        let state = AppState(store: InMemoryConnectionStore(), probe: StubProbe(reachable: false), envURL: { nil })
        await state.submit("box.lan:9000")
        #expect(state.screen == .pairing(message: "box.lan:9000 is not reachable"))
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
