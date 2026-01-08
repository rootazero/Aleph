//
//  SettingsView.swift
//  Aether
//
//  Shared settings types and views used by RootContentView.
//

import SwiftUI
import AppKit
import UniformTypeIdentifiers

// MARK: - Settings Tab Enum

enum SettingsTab: Hashable {
    case general
    case providers
    case routing
    case shortcuts
    case behavior
    case memory
    case search
    case skills
}

// MARK: - UTType Extension

extension UTType {
    static var toml: UTType {
        UTType(filenameExtension: "toml") ?? .plainText
    }
}

// MARK: - General Settings View

struct GeneralSettingsView: View {
    let core: AetherCore?
    @ObservedObject var saveBarState: SettingsSaveBarState
    @ObservedObject var themeEngine: ThemeEngine

    @State private var soundEnabled = false
    @State private var showingLogViewer = false
    @State private var selectedLanguage: String? = nil
    @State private var showCommandHints = true

    // Launch at login manager (via DependencyContainer)
    @ObservedObject private var launchAtLoginManager = DependencyContainer.shared.launchAtLoginManagerConcrete

    private var appVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "Unknown"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "Unknown"
        return "\(version) (Build \(build))"
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Form {
                    Section(header: Text(L("settings.general.theme"))) {
                        ThemePickerView(themeEngine: themeEngine)
                    }

                    Section(header: Text(L("settings.general.sound"))) {
                        Toggle(L("settings.general.sound_effects"), isOn: $soundEnabled)
                            .onChange(of: soundEnabled) { _, newValue in
                                showComingSoonAlert(feature: L("settings.general.sound_effects"))
                            }
                    }

                    Section(header: Text(L("settings.general.startup"))) {
                        Toggle(L("settings.general.launch_at_login"), isOn: $launchAtLoginManager.isEnabled)
                        Text(L("settings.general.launch_at_login_description"))
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    Section(header: Text(L("settings.general.command_completion"))) {
                        Toggle(L("settings.general.show_command_hints"), isOn: $showCommandHints)
                            .onChange(of: showCommandHints) { _, newValue in
                                saveShowCommandHints(newValue)
                            }
                        Text(L("settings.general.show_command_hints_description"))
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    Section(header: Text(L("settings.general.language"))) {
                        Picker(L("settings.general.language_preference"), selection: $selectedLanguage) {
                            Text(L("settings.general.language_system_default")).tag(nil as String?)
                            Text("English").tag("en" as String?)
                            Text("简体中文").tag("zh-Hans" as String?)
                        }
                        .onChange(of: selectedLanguage) { oldValue, newValue in
                            saveLanguagePreference(newValue)
                        }
                    }

                    Section(header: Text(L("settings.general.updates"))) {
                        Button(L("settings.general.check_updates")) {
                            checkForUpdates()
                        }
                        .help(L("settings.general.check_updates_help"))
                    }

                    Section(header: Text(L("settings.general.logs"))) {
                        Button(L("settings.general.view_logs")) {
                            showingLogViewer = true
                        }
                        .help(L("settings.general.view_logs_help"))
                        .disabled(core == nil)
                    }

                    Section(header: Text(L("settings.general.about"))) {
                        HStack {
                            Text(L("settings.general.version"))
                            Spacer()
                            Text(appVersion)
                                .foregroundColor(.secondary)
                        }
                    }
                }
                .formStyle(.grouped)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .padding(20)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .sheet(isPresented: $showingLogViewer) {
            if let core = core {
                LogViewerView(core: core)
            }
        }
        .onAppear {
            // Set save bar to disabled state for instant-save view
            saveBarState.update(
                hasUnsavedChanges: false,
                isSaving: false,
                statusMessage: nil,
                onSave: nil,
                onCancel: nil
            )

            // Load current language setting
            loadLanguagePreference()

            // Load command hints setting
            loadShowCommandHints()
        }
    }

    private func showComingSoonAlert(feature: String) {
        let alert = NSAlert()
        alert.messageText = L("settings.general.coming_soon")
        alert.informativeText = L("settings.general.coming_soon_message", feature)
        alert.alertStyle = .informational
        alert.addButton(withTitle: L("common.ok"))
        alert.runModal()
    }

    private func checkForUpdates() {
        let alert = NSAlert()
        alert.messageText = L("settings.general.check_updates")
        alert.informativeText = """
        Current Version: \(appVersion)

        To check for updates, please visit:
        https://github.com/yourusername/aether/releases

        Automatic updates will be available in a future release.
        """
        alert.alertStyle = .informational
        alert.addButton(withTitle: L("common.ok"))
        alert.addButton(withTitle: "Visit GitHub")

        let response = alert.runModal()
        if response == .alertSecondButtonReturn {
            if let url = URL(string: "https://github.com/yourusername/aether/releases") {
                NSWorkspace.shared.open(url)
            }
        }
    }

    private func loadLanguagePreference() {
        guard let core = core else { return }
        do {
            let config = try core.loadConfig()
            selectedLanguage = config.general.language
        } catch {
            print("Failed to load language preference: \(error)")
        }
    }

    private func loadShowCommandHints() {
        guard let core = core else { return }
        do {
            let config = try core.loadConfig()
            showCommandHints = config.general.showCommandHints ?? true
        } catch {
            print("Failed to load showCommandHints: \(error)")
        }
    }

    private func saveShowCommandHints(_ enabled: Bool) {
        guard let core = core else { return }

        do {
            // Load current config
            var config = try core.loadConfig()

            // Update showCommandHints field
            config.general.showCommandHints = enabled

            // Save config using update_general_config
            try core.updateGeneralConfig(config: config.general)

            print("[Settings] showCommandHints saved: \(enabled)")
        } catch {
            print("Failed to save showCommandHints: \(error)")
            let alert = NSAlert()
            alert.messageText = L("common.error")
            alert.informativeText = "Failed to save command hints setting: \(error.localizedDescription)"
            alert.alertStyle = .warning
            alert.addButton(withTitle: L("common.ok"))
            alert.runModal()
        }
    }

    private func saveLanguagePreference(_ language: String?) {
        guard let core = core else { return }

        do {
            // Load current config
            var config = try core.loadConfig()

            // Update language field
            config.general.language = language

            // Save config using update_general_config
            try core.updateGeneralConfig(config: config.general)

            // Show restart alert
            showRestartAlert()
        } catch {
            print("Failed to save language preference: \(error)")
            let alert = NSAlert()
            alert.messageText = L("common.error")
            alert.informativeText = "Failed to save language preference: \(error.localizedDescription)"
            alert.alertStyle = .warning
            alert.addButton(withTitle: L("common.ok"))
            alert.runModal()
        }
    }

    private func showRestartAlert() {
        let alert = NSAlert()
        alert.messageText = L("settings.general.language_restart_title")
        alert.informativeText = L("settings.general.language_restart_message")
        alert.alertStyle = .informational
        alert.addButton(withTitle: L("settings.general.language_restart_now"))
        alert.addButton(withTitle: L("settings.general.language_restart_later"))

        let response = alert.runModal()
        if response == .alertFirstButtonReturn {
            // User chose "Restart Now" - restart the application
            restartApplication()
        }
    }

    /// Restart the application after language change
    /// Uses NSWorkspace to launch a new instance before terminating current instance
    private func restartApplication() {
        print("[SettingsView] Restarting application for language change")

        let url = URL(fileURLWithPath: Bundle.main.bundlePath)
        let config = NSWorkspace.OpenConfiguration()
        config.createsNewApplicationInstance = true

        NSWorkspace.shared.openApplication(at: url, configuration: config) { _, error in
            if let error = error {
                print("[SettingsView] ❌ Error restarting application: \(error)")
                // Show error alert if restart fails
                DispatchQueue.main.async {
                    let errorAlert = NSAlert()
                    errorAlert.messageText = L("alert.restart.failed_title")
                    errorAlert.informativeText = L("alert.restart.failed_message", error.localizedDescription)
                    errorAlert.alertStyle = .warning
                    errorAlert.addButton(withTitle: L("common.ok"))
                    errorAlert.runModal()
                }
            }

            // Terminate current instance after new instance starts
            DispatchQueue.main.async {
                NSApp.terminate(nil)
            }
        }
    }
}

// MARK: - Theme Picker View

struct ThemePickerView: View {
    @ObservedObject var themeEngine: ThemeEngine
    @State private var hoveredTheme: Theme?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(L("settings.general.theme_description"))
                .font(.caption)
                .foregroundColor(.secondary)

            HStack(spacing: 16) {
                ForEach(Theme.allCases, id: \.self) { theme in
                    ThemeCard(
                        theme: theme,
                        isSelected: themeEngine.selectedTheme == theme,
                        isHovered: hoveredTheme == theme
                    ) {
                        withAnimation(.easeInOut(duration: 0.2)) {
                            themeEngine.setTheme(theme)
                        }
                    }
                    .onHover { isHovered in
                        hoveredTheme = isHovered ? theme : nil
                    }
                }
            }
        }
    }
}

// MARK: - Theme Card

private struct ThemeCard: View {
    let theme: Theme
    let isSelected: Bool
    let isHovered: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 8) {
                // Theme preview
                ZStack {
                    RoundedRectangle(cornerRadius: 12)
                        .fill(theme.previewBackground)
                        .frame(width: 80, height: 60)

                    // Theme-specific preview element
                    theme.previewIcon
                        .frame(width: 30, height: 30)
                }
                .overlay(
                    RoundedRectangle(cornerRadius: 12)
                        .stroke(isSelected ? Color.accentColor : Color.clear, lineWidth: 2)
                )

                // Theme name
                Text(theme.localizedName)
                    .font(.system(size: 11, weight: isSelected ? .semibold : .regular))
                    .foregroundColor(isSelected ? .primary : .secondary)

                // Selection indicator
                if isSelected {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 12))
                        .foregroundColor(.accentColor)
                } else {
                    Circle()
                        .stroke(Color.secondary.opacity(0.3), lineWidth: 1)
                        .frame(width: 12, height: 12)
                }
            }
            .padding(8)
            .background(
                RoundedRectangle(cornerRadius: 16)
                    .fill(isHovered ? Color.primary.opacity(0.05) : Color.clear)
            )
        }
        .buttonStyle(.plain)
        .scaleEffect(isHovered ? 1.02 : 1.0)
        .animation(.easeInOut(duration: 0.15), value: isHovered)
    }
}

// MARK: - Theme Extension for UI

extension Theme {
    /// Localized display name
    var localizedName: String {
        switch self {
        case .cyberpunk:
            return L("settings.general.theme_cyberpunk")
        case .zen:
            return L("settings.general.theme_zen")
        case .jarvis:
            return L("settings.general.theme_jarvis")
        }
    }

    /// Preview background color
    var previewBackground: Color {
        switch self {
        case .cyberpunk:
            return Color(red: 0.1, green: 0.05, blue: 0.15) // Dark purple
        case .zen:
            return Color(white: 0.95) // Light gray
        case .jarvis:
            return Color(red: 0.05, green: 0.1, blue: 0.15) // Dark blue
        }
    }

    /// Preview icon view
    @ViewBuilder
    var previewIcon: some View {
        switch self {
        case .cyberpunk:
            CyberpunkPreviewIcon()
        case .zen:
            ZenPreviewIcon()
        case .jarvis:
            JarvisPreviewIcon()
        }
    }
}

// MARK: - Theme Preview Icons

private struct CyberpunkPreviewIcon: View {
    @State private var rotation: Double = 0

    var body: some View {
        HexagonPreview()
            .stroke(Color.cyan, lineWidth: 2)
            .shadow(color: .cyan, radius: 3)
            .rotationEffect(.degrees(rotation))
            .onAppear {
                withAnimation(.linear(duration: 4).repeatForever(autoreverses: false)) {
                    rotation = 360
                }
            }
    }
}

private struct ZenPreviewIcon: View {
    @State private var scale: CGFloat = 1.0

    var body: some View {
        Circle()
            .stroke(Color.gray.opacity(0.6), lineWidth: 2)
            .scaleEffect(scale)
            .onAppear {
                withAnimation(.easeInOut(duration: 1.5).repeatForever(autoreverses: true)) {
                    scale = 1.1
                }
            }
    }
}

private struct JarvisPreviewIcon: View {
    @State private var glow: Double = 0.5

    var body: some View {
        Circle()
            .fill(
                RadialGradient(
                    colors: [Color(red: 0.0, green: 0.83, blue: 1.0), Color.clear],
                    center: .center,
                    startRadius: 0,
                    endRadius: 15
                )
            )
            .shadow(color: Color(red: 0.0, green: 0.83, blue: 1.0), radius: 5 * glow)
            .onAppear {
                withAnimation(.easeInOut(duration: 1.0).repeatForever(autoreverses: true)) {
                    glow = 1.5
                }
            }
    }
}

private struct HexagonPreview: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        let center = CGPoint(x: rect.midX, y: rect.midY)
        let radius = min(rect.width, rect.height) / 2

        for i in 0..<6 {
            let angle = CGFloat(i) * .pi / 3.0 - .pi / 2.0
            let point = CGPoint(
                x: center.x + radius * cos(angle),
                y: center.y + radius * sin(angle)
            )

            if i == 0 {
                path.move(to: point)
            } else {
                path.addLine(to: point)
            }
        }
        path.closeSubpath()
        return path
    }
}
