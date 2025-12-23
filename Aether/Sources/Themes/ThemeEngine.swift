//
//  ThemeEngine.swift
//  Aether
//
//  Theme management and persistence
//

import SwiftUI
import Combine

/// Manages current theme selection and persistence
class ThemeEngine: ObservableObject {
    // MARK: - Published Properties

    /// Currently selected theme
    @Published var currentTheme: Theme {
        didSet {
            saveTheme()
        }
    }

    /// Currently active theme instance
    var activeTheme: any HaloTheme {
        currentTheme.makeTheme()
    }

    // MARK: - Constants

    private let themeDefaultsKey = "selectedTheme"

    // MARK: - Initialization

    /// Initialize theme engine with persisted or default theme
    init() {
        // Load saved theme from UserDefaults, default to zen
        if let savedTheme = UserDefaults.standard.string(forKey: themeDefaultsKey),
           let theme = Theme(rawValue: savedTheme) {
            currentTheme = theme
        } else {
            currentTheme = .zen
            // Save default theme
            saveTheme()
        }
    }

    // MARK: - Public Methods

    /// Change current theme with optional animation
    /// - Parameter theme: New theme to apply
    func setTheme(_ theme: Theme) {
        withAnimation(.easeInOut(duration: 0.5)) {
            currentTheme = theme
        }
    }

    // MARK: - Private Methods

    /// Persist current theme to UserDefaults
    private func saveTheme() {
        UserDefaults.standard.set(currentTheme.rawValue, forKey: themeDefaultsKey)
    }
}
