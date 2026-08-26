import Testing
import Foundation
import WebKit
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
/// The TOFU decision above only ever runs if WebKit actually dispatches to the
/// coordinator's challenge hook — and that dispatch is by ObjC selector, not by
/// the compiler. `WKNavigationDelegate` declares the requirement `@optional`,
/// so a Swift method whose signature drifts from the imported one does not fail
/// to build: it simply stops being a witness, loses its `@objc` export, and is
/// never called. WebKit then takes the documented no-implementation path
/// (`NSURLSessionAuthChallengeRejectProtectionSpace`), so every self-signed LAN
/// gateway — the documented default, and the only reason this whole file
/// exists — becomes unreachable, with no error anywhere.
///
/// This happened once, for a reason nobody would grep for: the header annotates
/// the completion handler `WK_SWIFT_UI_ACTOR`, so it imports as
/// `@MainActor @Sendable`. Swift 5 accepted the un-annotated spelling as a
/// witness; Swift 6 does not, and only emits `nearly matches optional
/// requirement` — a *warning*, in a build that otherwise passes with 42 green
/// tests, because nothing here needs a real TLS challenge to go green.
///
/// So assert the effect the runtime depends on, not the presence of the source:
/// ask the ObjC runtime whether the selector is exported at all.
@Suite struct CertChallengeHookIsWiredTests {
    @Test("WebKit's challenge selector is actually exported by the coordinator")
    func challengeSelectorIsExported() {
        // Derived from WebKit's own declaration rather than typed as a string:
        // a hand-written selector that no longer matches anything is a guard
        // that passes for the wrong reason, and renames happen upstream.
        let selector = #selector(
            WKNavigationDelegate.webView(_:didReceive:completionHandler:)
        )
        #expect(
            PanelWebView.Coordinator.instancesRespond(to: selector),
            """
            PanelWebView.Coordinator no longer witnesses \
            webView(_:didReceive:completionHandler:). Its signature has drifted \
            from WKNavigationDelegate's imported one (check the completion \
            handler's @MainActor @Sendable annotations) — WebKit will now reject \
            every server-trust challenge and no self-signed gateway can be \
            reached.
            """
        )
    }
}

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
