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
    case generation  // Image/Video/Audio generation providers
    case routing
    case shortcuts
    case behavior
    case memory
    case search
    case mcp
    case skills
    case cowork
    case policies
    case runtimes  // External runtime management (uv, fnm, yt-dlp)
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

    @State private var soundEnabled = false
    @State private var showingLogViewer = false
    @State private var selectedLanguage: String? = nil

    // Launch at login manager
    @ObservedObject private var launchAtLoginManager = LaunchAtLoginManager.shared

    private var appVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "Unknown"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "Unknown"
        return "\(version) (Build \(build))"
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Form {
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
            // Load current language setting
            loadLanguagePreference()
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

// Theme-related views removed - using unified visual style
