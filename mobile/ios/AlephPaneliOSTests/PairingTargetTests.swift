import Testing
import Foundation
@testable import AlephPaneliOS

@Suite struct PairingTargetTests {
    @Test("empty and whitespace reject")
    func emptyRejected() {
        #expect(PairingTarget.parse("") == .failure(.empty))
        #expect(PairingTarget.parse("   ") == .failure(.empty))
    }

    /// A bare host means "an aleph-server lives there": the gateway ships with
    /// TLS off, so the assumed scheme is `http` and the port is its own default
    /// listener. This matches the desktop shell's `ConnectionTarget::parse`,
    /// which the two shells promise to share — they did not, and the phone's
    /// `https` assumption combined with the forced 18790 into an origin no
    /// deployment serves.
    @Test("bare host gets http and the Aleph listener port")
    func bareHost() throws {
        let t = try PairingTarget.parse("192.168.1.5").get()
        #expect(t.url.absoluteString == "http://192.168.1.5:18790")
        #expect(t.host == "192.168.1.5")
        #expect(t.port == 18790)
        #expect(t.scheme == "http")
    }

    @Test("host:port keeps user port, adds http")
    func hostPort() throws {
        let t = try PairingTarget.parse("box.lan:9000").get()
        #expect(t.url.absoluteString == "http://box.lan:9000")
        #expect(t.port == 9000)
    }

    /// The reported failure, pinned. A typed `https://` with no port names a
    /// reverse proxy or CDN, whose port is 443 — injecting the Aleph listener
    /// port rewrote a working address into one that nothing answers, and the
    /// user could not see it because the field only shows what they typed.
    /// Asserted on `port` / `origin` rather than `url.absoluteString`: whether
    /// Foundation keeps a *scheme-default* port in the serialised URL is its
    /// business, and pinning it here would test Foundation instead of the rule.
    /// What must hold is which endpoint we resolve to and report.
    @Test("a typed https URL keeps the scheme default port, not 18790")
    func typedHttpsGetsSchemeDefault() throws {
        let t = try PairingTarget.parse("https://gw.example.com").get()
        #expect(t.port == 443)
        #expect(t.scheme == "https")
        #expect(
            t.origin == "https://gw.example.com:443",
            "the origin must be the one that will actually be dialled"
        )
    }

    @Test("a typed http URL keeps the scheme default port")
    func typedHttpGetsSchemeDefault() throws {
        let t = try PairingTarget.parse("http://gw.example.com").get()
        #expect(t.port == 80)
        #expect(t.origin == "http://gw.example.com:80")
    }

    /// Foundation drops the brackets off an IPv6 host, so the origin has to put
    /// them back — otherwise our own failure message reads `http://::1:18790`.
    @Test("an ipv6 origin stays bracketed")
    func ipv6OriginIsBracketed() throws {
        let t = try PairingTarget.parse("[::1]:9000").get()
        #expect(t.origin == "http://[::1]:9000")
    }

    @Test("explicit port preserved with scheme")
    func explicitPort() throws {
        let t = try PairingTarget.parse("https://gw.example.com:8443").get()
        #expect(t.port == 8443)
        #expect(t.origin == "https://gw.example.com:8443")
    }

    @Test("token query is preserved")
    func tokenPreserved() throws {
        let t = try PairingTarget.parse("http://127.0.0.1:18790/?token=aleph-abc123").get()
        #expect(t.url.query?.contains("token=aleph-abc123") == true)
        #expect(t.port == 18790)
    }

    @Test("unsupported schemes rejected")
    func unsupportedScheme() {
        #expect(PairingTarget.parse("ftp://host") == .failure(.unsupportedScheme("ftp")))
        #expect(PairingTarget.parse("ws://host") == .failure(.unsupportedScheme("ws")))
    }

    @Test("ipv6 with and without port")
    func ipv6() throws {
        let withPort = try PairingTarget.parse("http://[::1]:9000").get()
        #expect(withPort.port == 9000)
        // A typed scheme, so the scheme's own default applies here too.
        let noPort = try PairingTarget.parse("http://[::1]").get()
        #expect(noPort.port == 80)
        // …whereas a bare IPv6 literal is the LAN case and keeps 18790.
        let bare = try PairingTarget.parse("[::1]").get()
        #expect(bare.port == 18790)
    }

    /// The bracket rule, pinned as an *invariant* rather than as Foundation's
    /// current behaviour. `URLComponents.host` has returned both `::1` and
    /// `[::1]` across Foundation versions; the previous code encoded one of
    /// them in a comment and re-added brackets unconditionally, which emitted
    /// `http://[[::1]]:9000` once Foundation started supplying them itself
    /// (measured on the iOS 27 SDK: `.host` == `[::1]`). Asserting "exactly one
    /// bracket pair" holds under either behaviour, so this test cannot rot the
    /// same way the comment did.
    @Test("the ipv6 bracket rule is answered exactly once")
    func ipv6BracketRuleIsIdempotent() throws {
        let t = try PairingTarget.parse("[::1]:9000").get()

        #expect(t.host == "::1", "host is the bare literal; got \(t.host)")
        #expect(!t.host.contains("["), "host must never carry brackets")
        #expect(t.hostLiteral == "[::1]", "got \(t.hostLiteral)")
        #expect(t.origin.filter { $0 == "[" }.count == 1, "got \(t.origin)")
        #expect(t.origin.filter { $0 == "]" }.count == 1, "got \(t.origin)")

        // A name is not an IPv6 literal and must stay unbracketed.
        let named = try PairingTarget.parse("box.lan:9000").get()
        #expect(named.hostLiteral == "box.lan")
        #expect(!named.origin.contains("["))
    }

    /// The second half of the same defect. `origin` and `unreachableMessage`
    /// each decided the bracket question separately and in opposite directions,
    /// so one of them was always wrong — the message happened to be the right
    /// one here, which is why only `origin` failed. Both now read the same
    /// accessor, and this pins the message so they cannot drift apart again.
    @Test("an ipv6 failure message stays bracketed too")
    func ipv6UnreachableMessageIsBracketed() throws {
        let t = try PairingTarget.parse("[::1]").get()
        let msg = t.unreachableMessage
        #expect(msg.contains("http://[::1]:18790"), "got: \(msg)")
        #expect(!msg.contains("[[") && !msg.contains("]]"), "got: \(msg)")
        // The bare `::1:18790` spelling parses as nothing and reads as a typo
        // in our own error text.
        #expect(!msg.contains("://::1"), "got: \(msg)")
    }

    @Test("out-of-range port rejected, not crashing")
    func outOfRangePortRejected() {
        #expect(PairingTarget.parse("http://host:99999") == .failure(.invalidURL))
    }

    /// The port rule is the desktop's `connection::default_port_for`, and it is
    /// tested directly so a future "simplification" back to a single constant
    /// has to walk past a named assertion.
    @Test("the port rule answers two different questions")
    func portRuleMirrorsTheDesktop() {
        #expect(PairingTarget.defaultPort(hasTypedScheme: false, scheme: "http") == 18790)
        #expect(PairingTarget.defaultPort(hasTypedScheme: false, scheme: "https") == 18790)
        #expect(PairingTarget.defaultPort(hasTypedScheme: true, scheme: "https") == 443)
        #expect(PairingTarget.defaultPort(hasTypedScheme: true, scheme: "http") == 80)
    }

    /// The failure sentence must name the resolved origin and point at the fix.
    /// An `https` target on a non-default port is the reverse-proxy case, where
    /// the way out is to *remove* the port — "try another port" would send the
    /// user the wrong way, and every install upgrading from the build that
    /// force-injected 18790 lands exactly there.
    @Test("the unreachable message names the origin and the way out")
    func unreachableMessageGuides() throws {
        let stale = try PairingTarget.parse("https://aleph.example.com:18790").get()
        let msg = stale.unreachableMessage
        #expect(msg.contains("https://aleph.example.com:18790"), "got: \(msg)")
        #expect(msg.contains("remove the port"), "got: \(msg)")
        #expect(msg.contains("(https://aleph.example.com)"), "got: \(msg)")

        // Nothing to remove on the default port — that advice would be a no-op
        // instruction there.
        let on443 = try PairingTarget.parse("https://aleph.example.com").get()
        #expect(!on443.unreachableMessage.contains("remove the port"))
        #expect(on443.unreachableMessage.contains("aleph.example.com:8443"))

        // The cleartext case points at the https form, which is both the
        // reverse-proxy answer and the way past an ATS refusal.
        let plain = try PairingTarget.parse("aleph.example.com").get()
        #expect(plain.unreachableMessage.contains("https://aleph.example.com"))
    }
}
