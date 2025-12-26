//
//  ThemeEngine.swift
//  Aether
//
//  Theme management - System appearance only (follows macOS light/dark mode)
//

import SwiftUI
import Combine

/// Manages theme rendering based on system appearance
/// Automatically adapts to macOS light/dark mode changes
class ThemeEngine: ObservableObject {
    // MARK: - Published Properties

    /// Currently active theme instance (always uses system appearance)
    var activeTheme: any HaloTheme {
        ZenTheme()
    }

    // MARK: - Initialization

    /// Initialize theme engine
    init() {
        // Theme is now purely reactive to system appearance
        // No persistence needed
    }
}
