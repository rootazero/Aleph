import Testing
import Foundation
@testable import AlephPaneliOS

// Serialized: all three tests share one keychain entry (same service+account),
// so they must not run in parallel (Swift Testing's default) or they race.
@Suite(.serialized) struct KeychainConnectionStoreTests {
    let store = KeychainConnectionStore()

    init() {
        // isolate each test from prior keychain state
        try? store.save(URL(string: "http://invalid.local/.test.placeholder")!)
        _ = store.load()
    }

    @Test("save then load round-trips the full URL")
    func roundTrip() throws {
        let url = URL(string: "http://127.0.0.1:18790/?token=aleph-xyz")!
        try store.save(url)
        #expect(store.load() == url)
    }

    @Test("save overwrites a previous value")
    func overwrite() throws {
        try store.save(URL(string: "http://a.lan:18790")!)
        try store.save(URL(string: "http://b.lan:9000")!)
        #expect(store.load() == URL(string: "http://b.lan:9000")!)
    }
}
