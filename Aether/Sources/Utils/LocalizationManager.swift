//
//  LocalizationManager.swift
//  Aether
//
//  Centralized localization manager that properly handles runtime language switching.
//  This solves the issue where Bundle.main caches its localization at load time.
//

import Foundation

/// Centralized localization manager for Aether
///
/// This manager solves the critical issue where `NSLocalizedString` uses `Bundle.main`,
/// which determines its localization at bundle load time, not at runtime.
/// Setting `AppleLanguages` after the app launches doesn't affect the current session.
///
/// Usage:
/// ```swift
/// // Instead of: NSLocalizedString("menu.about", comment: "")
/// // Use: L10n.string("menu.about")
/// ```
final class LocalizationManager {

    // MARK: - Singleton

    static let shared = LocalizationManager()

    // MARK: - Properties

    /// Cached bundle for the current language
    private var localizedBundle: Bundle?

    /// Current language code (e.g., "en", "zh-Hans")
    private(set) var currentLanguage: String = "en"

    /// Supported languages
    static let supportedLanguages = ["en", "zh-Hans"]

    // MARK: - Initialization

    private init() {
        // Determine language on initialization
        currentLanguage = determineLanguage()
        localizedBundle = loadBundle(for: currentLanguage)

        print("[LocalizationManager] Initialized with language: \(currentLanguage)")
    }

    // MARK: - Public API

    /// Get localized string for the given key
    ///
    /// - Parameters:
    ///   - key: The localization key
    ///   - comment: Optional comment for translators
    /// - Returns: The localized string, or the key if not found
    func string(_ key: String, comment: String = "") -> String {
        guard let bundle = localizedBundle else {
            // Fallback to Bundle.main if custom bundle not available
            return NSLocalizedString(key, bundle: Bundle.main, comment: comment)
        }

        let localizedString = bundle.localizedString(forKey: key, value: nil, table: nil)

        // If the key is returned as-is, it means the translation was not found
        // Try fallback to English
        if localizedString == key && currentLanguage != "en" {
            if let englishBundle = loadBundle(for: "en") {
                let englishString = englishBundle.localizedString(forKey: key, value: nil, table: nil)
                if englishString != key {
                    return englishString
                }
            }
        }

        return localizedString
    }

    /// Get localized string with format arguments
    ///
    /// - Parameters:
    ///   - key: The localization key
    ///   - args: Format arguments
    /// - Returns: The formatted localized string
    func string(_ key: String, _ args: CVarArg...) -> String {
        let format = string(key)
        return String(format: format, arguments: args)
    }

    /// Update the current language and reload bundle
    ///
    /// - Parameter language: The language code (e.g., "en", "zh-Hans", or nil for system default)
    func setLanguage(_ language: String?) {
        let newLanguage: String

        if let lang = language, Self.supportedLanguages.contains(lang) {
            newLanguage = lang
        } else {
            // Use system language
            newLanguage = detectSystemLanguage()
        }

        if newLanguage != currentLanguage {
            currentLanguage = newLanguage
            localizedBundle = loadBundle(for: currentLanguage)
            print("[LocalizationManager] Language changed to: \(currentLanguage)")

            // Post notification for UI refresh
            NotificationCenter.default.post(
                name: .localizationDidChange,
                object: nil
            )
        }
    }

    /// Reload the current bundle (useful after app restart)
    func reload() {
        currentLanguage = determineLanguage()
        localizedBundle = loadBundle(for: currentLanguage)
        print("[LocalizationManager] Reloaded with language: \(currentLanguage)")
    }

    // MARK: - Private Methods

    /// Determine which language to use based on config and system settings
    private func determineLanguage() -> String {
        // Check user config first
        if let configuredLanguage = loadLanguageFromConfig() {
            print("[LocalizationManager] Using configured language: \(configuredLanguage)")
            return configuredLanguage
        }

        // Fall back to system language
        let systemLanguage = detectSystemLanguage()
        print("[LocalizationManager] Using system language: \(systemLanguage)")
        return systemLanguage
    }

    /// Load language preference from config file
    private func loadLanguageFromConfig() -> String? {
        let configPath = NSHomeDirectory() + "/.config/aether/config.toml"

        guard FileManager.default.fileExists(atPath: configPath),
              let content = try? String(contentsOfFile: configPath, encoding: .utf8) else {
            return nil
        }

        // Parse language field from [general] section
        // Use word boundary \b to avoid matching preferred_language or other compound keys
        let pattern = #"\blanguage\s*=\s*"([^"]+)""#
        guard let regex = try? NSRegularExpression(pattern: pattern, options: []),
              let match = regex.firstMatch(in: content, options: [], range: NSRange(content.startIndex..., in: content)),
              let languageRange = Range(match.range(at: 1), in: content) else {
            return nil
        }

        let language = String(content[languageRange])

        // Validate it's a supported language
        guard Self.supportedLanguages.contains(language) else {
            print("[LocalizationManager] Configured language '\(language)' not supported")
            return nil
        }

        return language
    }

    /// Detect system language preference
    ///
    /// Uses CFPreferencesCopyAppValue to get the REAL system language preference,
    /// not the app-level UserDefaults which may have been overridden.
    private func detectSystemLanguage() -> String {
        // Get the REAL system language using CFPreferences
        guard let systemLanguages = CFPreferencesCopyAppValue(
            "AppleLanguages" as CFString,
            kCFPreferencesAnyApplication
        ) as? [String],
        let primaryLanguage = systemLanguages.first else {
            print("[LocalizationManager] No system language found, defaulting to English")
            return "en"
        }

        print("[LocalizationManager] System primary language: \(primaryLanguage)")

        // Map system language to supported language
        if primaryLanguage.hasPrefix("zh") {
            return "zh-Hans"
        }

        // Default to English for unsupported languages
        return "en"
    }

    /// Load the bundle for a specific language
    private func loadBundle(for language: String) -> Bundle? {
        // Try to find the .lproj folder in Bundle.main
        guard let lprojPath = Bundle.main.path(forResource: language, ofType: "lproj"),
              let bundle = Bundle(path: lprojPath) else {
            print("[LocalizationManager] Failed to load bundle for language: \(language)")

            // Fallback: try to find in Resources folder
            let resourcesPath = Bundle.main.bundlePath + "/Contents/Resources/\(language).lproj"
            if FileManager.default.fileExists(atPath: resourcesPath) {
                return Bundle(path: resourcesPath)
            }

            return nil
        }

        print("[LocalizationManager] Loaded bundle for language: \(language)")
        return bundle
    }
}

// MARK: - Convenience Type Alias

/// Shorthand for LocalizationManager
typealias L10n = LocalizationManager

// MARK: - Extension for Convenience

extension LocalizationManager {

    /// Check if current language is Chinese
    var isChinese: Bool {
        currentLanguage.hasPrefix("zh")
    }

    /// Check if current language is English
    var isEnglish: Bool {
        currentLanguage == "en"
    }

    /// Get display name for a language code
    static func displayName(for languageCode: String) -> String {
        switch languageCode {
        case "en":
            return "English"
        case "zh-Hans":
            return "简体中文"
        default:
            return languageCode
        }
    }
}

// MARK: - Global Function for Easy Access

/// Global function for localized strings
///
/// Usage:
/// ```swift
/// let text = L("menu.about")
/// let formatted = L("settings.memory.user_prefix", username)
/// ```
func L(_ key: String, _ args: CVarArg...) -> String {
    if args.isEmpty {
        return LocalizationManager.shared.string(key)
    } else {
        let format = LocalizationManager.shared.string(key)
        return String(format: format, arguments: args)
    }
}
