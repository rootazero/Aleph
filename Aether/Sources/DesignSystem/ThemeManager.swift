import SwiftUI
import AppKit
import Combine

/// Theme mode options for the application
enum ThemeMode: String, CaseIterable {
    case light
    case dark
    case auto

    /// Display name for each mode
    var displayName: String {
        switch self {
        case .light:
            return "Light"
        case .dark:
            return "Dark"
        case .auto:
            return "Auto"
        }
    }

    /// SF Symbol icon name for each mode
    var iconName: String {
        switch self {
        case .light:
            return "sun.max.fill"
        case .dark:
            return "moon.fill"
        case .auto:
            return "circle.lefthalf.filled"
        }
    }

    /// Convert to NSAppearance
    var appearance: NSAppearance? {
        switch self {
        case .light:
            return NSAppearance(named: .aqua)
        case .dark:
            return NSAppearance(named: .darkAqua)
        case .auto:
            return nil // Use system default
        }
    }
}

/// Manages application theme and persists user preference
final class ThemeManager: ObservableObject {
    // MARK: - Properties

    /// Current theme mode
    @Published var currentTheme: ThemeMode {
        didSet {
            saveThemePreference()
            applyTheme()
        }
    }

    /// UserDefaults key for storing theme preference
    private let themeKey = "app.theme.mode"

    /// System appearance change observer
    private var appearanceObserver: AnyCancellable?

    // MARK: - Initialization

    init() {
        // Load saved theme preference or default to auto
        if let savedTheme = UserDefaults.standard.string(forKey: themeKey),
           let theme = ThemeMode(rawValue: savedTheme) {
            self.currentTheme = theme
        } else {
            self.currentTheme = .auto
        }

        // Apply the loaded theme
        applyTheme()

        // Observe system appearance changes (for auto mode)
        setupAppearanceObserver()
    }

    // MARK: - Public Methods

    /// Apply the current theme to the application
    func applyTheme() {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            // Set application-wide appearance first
            if self.currentTheme == .auto {
                NSApp.appearance = nil
            } else {
                NSApp.appearance = self.currentTheme.appearance
            }

            // Get all windows and apply appearance
            let windows = NSApplication.shared.windows

            // Apply appearance to each window
            for window in windows {
                if self.currentTheme == .auto {
                    // Remove custom appearance to follow system
                    window.appearance = nil
                } else {
                    // Set specific appearance
                    window.appearance = self.currentTheme.appearance
                }

                // Force window to update its appearance
                window.invalidateShadow()
                window.displayIfNeeded()
            }

            // Notify that the theme has changed
            self.objectWillChange.send()
        }
    }

    /// Switch to the next theme mode (for keyboard shortcut support)
    func cycleTheme() {
        let allModes = ThemeMode.allCases
        if let currentIndex = allModes.firstIndex(of: currentTheme) {
            let nextIndex = (currentIndex + 1) % allModes.count
            currentTheme = allModes[nextIndex]
        }
    }

    // MARK: - Private Methods

    /// Save theme preference to UserDefaults
    private func saveThemePreference() {
        UserDefaults.standard.set(currentTheme.rawValue, forKey: themeKey)
    }

    /// Setup observer for system appearance changes
    private func setupAppearanceObserver() {
        // Observe effective appearance changes on main windows
        appearanceObserver = NotificationCenter.default
            .publisher(for: NSApplication.didChangeScreenParametersNotification)
            .sink { [weak self] _ in
                guard let self = self else { return }
                // Re-apply theme if in auto mode
                if self.currentTheme == .auto {
                    self.applyTheme()
                }
            }
    }
}

// MARK: - View Extension for Theme Management

extension View {
    /// Apply theme manager to the view hierarchy
    func applyThemeManager(_ themeManager: ThemeManager) -> some View {
        self.onAppear {
            themeManager.applyTheme()
        }
    }
}
