import SwiftUI

extension Color {
    /// Parse a 6- or 8-digit hex string (optional leading `#`, surrounding
    /// whitespace tolerated) into sRGB components in `0...1`. Returns `nil` on
    /// malformed input. Pure and unit-tested — the one bit of real logic in the
    /// iPad styling work.
    static func rgba(fromHex hex: String) -> (red: Double, green: Double, blue: Double, alpha: Double)? {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s = String(s.dropFirst()) }
        guard s.count == 6 || s.count == 8, let value = UInt64(s, radix: 16) else {
            return nil
        }
        if s.count == 8 {
            return (
                red: Double((value >> 24) & 0xff) / 255,
                green: Double((value >> 16) & 0xff) / 255,
                blue: Double((value >> 8) & 0xff) / 255,
                alpha: Double(value & 0xff) / 255
            )
        }
        return (
            red: Double((value >> 16) & 0xff) / 255,
            green: Double((value >> 8) & 0xff) / 255,
            blue: Double(value & 0xff) / 255,
            alpha: 1.0
        )
    }

    /// SwiftUI `Color` from a hex string. Falls back to `.clear` on malformed
    /// input so a typo in one of our own compile-time literals is visible at QA
    /// instead of crashing the view (P7 — no reachable trap).
    init(hex: String) {
        guard let c = Color.rgba(fromHex: hex) else {
            self = .clear
            return
        }
        self = Color(.sRGB, red: c.red, green: c.green, blue: c.blue, opacity: c.alpha)
    }
}
