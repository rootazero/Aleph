import Testing
import SwiftUI
@testable import AlephPaneliOS

@Suite struct ColorHexTests {
    @Test("parses 6-digit hex")
    func sixDigit() throws {
        let c = try #require(Color.rgba(fromHex: "0d0d10"))
        #expect(abs(c.red - 13.0 / 255) < 0.0001)
        #expect(abs(c.green - 13.0 / 255) < 0.0001)
        #expect(abs(c.blue - 16.0 / 255) < 0.0001)
        #expect(c.alpha == 1.0)
    }

    @Test("accepts a leading hash")
    func leadingHash() throws {
        let c = try #require(Color.rgba(fromHex: "#4f46e5"))
        #expect(abs(c.red - 79.0 / 255) < 0.0001)
        #expect(abs(c.green - 70.0 / 255) < 0.0001)
        #expect(abs(c.blue - 229.0 / 255) < 0.0001)
        #expect(c.alpha == 1.0)
    }

    @Test("parses 8-digit hex with alpha")
    func eightDigitAlpha() throws {
        let c = try #require(Color.rgba(fromHex: "ff000080"))
        #expect(abs(c.red - 1.0) < 0.0001)
        #expect(abs(c.green - 0.0) < 0.0001)
        #expect(abs(c.blue - 0.0) < 0.0001)
        #expect(abs(c.alpha - 128.0 / 255) < 0.0001)
    }

    @Test("malformed input returns nil")
    func malformed() {
        #expect(Color.rgba(fromHex: "xyz") == nil)        // non-hex
        #expect(Color.rgba(fromHex: "12345") == nil)      // wrong length
        #expect(Color.rgba(fromHex: "") == nil)           // empty
        #expect(Color.rgba(fromHex: "gggggg") == nil)     // non-hex digits, right length
    }
}
