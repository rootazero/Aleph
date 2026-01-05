//
//  ThemeEngine.swift
//  Aether
//
//  Theme management - Supports user-selected themes with persistence
//

import SwiftUI
import Combine

/// Manages theme rendering with user preference persistence
class ThemeEngine: ObservableObject {
    // MARK: - Published Properties

    /// Currently selected theme type
    @Published var selectedTheme: Theme {
        didSet {
            saveThemePreference()
        }
    }

    /// Currently active theme instance
    var activeTheme: any HaloTheme {
        selectedTheme.makeTheme()
    }

    // MARK: - Private Properties

    private let userDefaultsKey = "aether.halo.theme"

    // MARK: - Initialization

    /// Initialize theme engine with saved preference or default
    init() {
        // Load saved theme preference
        if let savedTheme = UserDefaults.standard.string(forKey: userDefaultsKey),
           let theme = Theme(rawValue: savedTheme) {
            self.selectedTheme = theme
        } else {
            // Default to Zen theme (circular halo with 3 rotating arcs)
            self.selectedTheme = .zen
        }
    }

    // MARK: - Public Methods

    /// Set the active theme
    func setTheme(_ theme: Theme) {
        selectedTheme = theme
    }

    // MARK: - Private Methods

    /// Save theme preference to UserDefaults
    private func saveThemePreference() {
        UserDefaults.standard.set(selectedTheme.rawValue, forKey: userDefaultsKey)
    }
}
