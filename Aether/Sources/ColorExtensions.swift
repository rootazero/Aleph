//
//  ColorExtensions.swift
//  Aether
//
//  Shared Color extensions for the entire project
//

import SwiftUI

// MARK: - Color Extension

extension Color {
    /// Initialize Color from hex string (e.g., "#10a37f")
    init?(hex: String) {
        let hexString = hex.trimmingCharacters(in: .whitespacesAndNewlines)
        let scanner = Scanner(string: hexString)

        if hexString.hasPrefix("#") {
            scanner.currentIndex = hexString.index(after: hexString.startIndex)
        }

        var rgbValue: UInt64 = 0
        guard scanner.scanHexInt64(&rgbValue) else {
            return nil
        }

        let r = Double((rgbValue & 0xFF0000) >> 16) / 255.0
        let g = Double((rgbValue & 0x00FF00) >> 8) / 255.0
        let b = Double(rgbValue & 0x0000FF) / 255.0

        self.init(red: r, green: g, blue: b)
    }

    /// Convert Color to hex string (e.g., "#10A37F")
    func toHex() -> String {
        #if os(macOS)
        guard let components = NSColor(self).cgColor.components else {
            return "#808080"
        }
        #else
        guard let components = UIColor(self).cgColor.components else {
            return "#808080"
        }
        #endif

        let r = Int(components[0] * 255.0)
        let g = Int(components[1] * 255.0)
        let b = Int(components[2] * 255.0)

        return String(format: "#%02X%02X%02X", r, g, b)
    }
}
