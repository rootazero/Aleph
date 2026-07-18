import Testing
import Foundation
@testable import AlephPaneliOS

@Suite struct CertTrustStoreDecisionTests {
    @Test("unknown host prompts")
    func unknownHostPrompts() {
        let store = InMemoryCertStore()
        if case .promptUnknown = certDecision(host: "h:1", presentedFP: "AA:BB", store: store) {
        } else {
            Issue.record("expected prompt")
        }
    }

    @Test("matching fingerprint allows")
    func matchingFPAllows() {
        let store = InMemoryCertStore()
        store.pin("h:1", "AA:BB")
        #expect(certDecision(host: "h:1", presentedFP: "AA:BB", store: store) == .allow)
    }

    @Test("changed fingerprint warns")
    func changedFPWarns() {
        let store = InMemoryCertStore()
        store.pin("h:1", "AA:BB")
        if case .warnChanged = certDecision(host: "h:1", presentedFP: "CC:DD", store: store) {
        } else {
            Issue.record("expected warn")
        }
    }
}

// Serialized: all tests share one underlying keychain item (the whole
// host->fp map lives in ONE generic-password entry), so they must not run in
// parallel (Swift Testing's default) or they race. Each test uses its own
// host key so they don't need explicit isolation between runs.
@Suite(.serialized) struct KeychainCertStoreTests {
    let store = KeychainCertStore()

    @Test("pin then lookup round-trips the fingerprint")
    func roundTrip() throws {
        let host = "172.245.43.211:18790"
        store.pin(host, "49:3D:51:AA")
        #expect(store.lookup(host) == "49:3D:51:AA")
    }

    @Test("pin overwrites a previous fingerprint for the same host")
    func overwrite() throws {
        let host = "overwrite-host:1"
        store.pin(host, "OLD")
        store.pin(host, "NEW")
        #expect(store.lookup(host) == "NEW")
    }

    @Test("lookup on an unknown host returns nil")
    func unknownHostReturnsNil() throws {
        #expect(store.lookup("definitely-not-pinned:1") == nil)
    }
}

/// Mirrors `InMemoryConnectionStore` — a plain-dict conformer for `CertStore`
/// used in decision-logic tests so they never touch the Keychain.
final class InMemoryCertStore: CertStore {
    private var pinned: [String: String] = [:]
    func lookup(_ host: String) -> String? { pinned[host] }
    func pin(_ host: String, _ fp: String) { pinned[host] = fp }
}
