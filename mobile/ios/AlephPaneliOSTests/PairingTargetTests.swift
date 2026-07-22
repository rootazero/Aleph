import Testing
import Foundation
@testable import AlephPaneliOS

@Suite struct PairingTargetTests {
    @Test("empty and whitespace reject")
    func emptyRejected() {
        #expect(PairingTarget.parse("") == .failure(.empty))
        #expect(PairingTarget.parse("   ") == .failure(.empty))
    }

    @Test("bare host gets https and default port")
    func bareHost() throws {
        let t = try PairingTarget.parse("192.168.1.5").get()
        #expect(t.url.absoluteString == "https://192.168.1.5:18790")
        #expect(t.host == "192.168.1.5")
        #expect(t.port == 18790)
    }

    @Test("host:port keeps user port, adds https")
    func hostPort() throws {
        let t = try PairingTarget.parse("box.lan:9000").get()
        #expect(t.url.absoluteString == "https://box.lan:9000")
        #expect(t.port == 9000)
    }

    @Test("explicit scheme preserved, default port added")
    func explicitScheme() throws {
        let t = try PairingTarget.parse("https://gw.example.com").get()
        #expect(t.url.absoluteString == "https://gw.example.com:18790")
    }

    @Test("explicit port preserved with scheme")
    func explicitPort() throws {
        let t = try PairingTarget.parse("https://gw.example.com:443").get()
        #expect(t.port == 443)
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
        let noPort = try PairingTarget.parse("http://[::1]").get()
        #expect(noPort.port == 18790)
    }

    @Test("out-of-range port rejected, not crashing")
    func outOfRangePortRejected() {
        #expect(PairingTarget.parse("http://host:99999") == .failure(.invalidURL))
    }
}
