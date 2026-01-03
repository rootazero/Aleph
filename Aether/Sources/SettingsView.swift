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

    @State private var soundEnabled = false
    @State private var showingLogViewer = false
    @State private var selectedLanguage: String? = nil

    private var appVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "Unknown"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "Unknown"
        return "\(version) (Build \(build))"
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Form {
                    Section(header: Text(LocalizedStringKey("settings.general.sound"))) {
                        Toggle(LocalizedStringKey("settings.general.sound_effects"), isOn: $soundEnabled)
                            .onChange(of: soundEnabled) { _, newValue in
                                showComingSoonAlert(feature: NSLocalizedString("settings.general.sound_effects", comment: "Sound effects feature"))
                            }
                    }

                    Section(header: Text(LocalizedStringKey("settings.general.language"))) {
                        Picker(LocalizedStringKey("settings.general.language_preference"), selection: $selectedLanguage) {
                            Text(LocalizedStringKey("settings.general.language_system_default")).tag(nil as String?)
                            Text("English").tag("en" as String?)
                            Text("简体中文").tag("zh-Hans" as String?)
                        }
                        .onChange(of: selectedLanguage) { oldValue, newValue in
                            saveLanguagePreference(newValue)
                        }
                    }

                    Section(header: Text(LocalizedStringKey("settings.general.updates"))) {
                        Button(LocalizedStringKey("settings.general.check_updates")) {
                            checkForUpdates()
                        }
                        .help(NSLocalizedString("settings.general.check_updates_help", comment: ""))
                    }

                    Section(header: Text(LocalizedStringKey("settings.general.logs"))) {
                        Button(LocalizedStringKey("settings.general.view_logs")) {
                            showingLogViewer = true
                        }
                        .help(NSLocalizedString("settings.general.view_logs_help", comment: ""))
                        .disabled(core == nil)
                    }

                    Section(header: Text(LocalizedStringKey("settings.general.about"))) {
                        HStack {
                            Text(LocalizedStringKey("settings.general.version"))
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
        }
    }

    private func showComingSoonAlert(feature: String) {
        let alert = NSAlert()
        alert.messageText = NSLocalizedString("settings.general.coming_soon", comment: "Coming soon alert title")
        alert.informativeText = String.localizedStringWithFormat(
            NSLocalizedString("settings.general.coming_soon_message", comment: "Coming soon message"),
            feature
        )
        alert.alertStyle = .informational
        alert.addButton(withTitle: NSLocalizedString("common.ok", comment: "OK button"))
        alert.runModal()
    }

    private func checkForUpdates() {
        let alert = NSAlert()
        alert.messageText = NSLocalizedString("settings.general.check_updates", comment: "Check updates alert title")
        alert.informativeText = """
        Current Version: \(appVersion)

        To check for updates, please visit:
        https://github.com/yourusername/aether/releases

        Automatic updates will be available in a future release.
        """
        alert.alertStyle = .informational
        alert.addButton(withTitle: NSLocalizedString("common.ok", comment: "OK button"))
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
            alert.messageText = NSLocalizedString("common.error", comment: "")
            alert.informativeText = "Failed to save language preference: \(error.localizedDescription)"
            alert.alertStyle = .warning
            alert.addButton(withTitle: NSLocalizedString("common.ok", comment: ""))
            alert.runModal()
        }
    }

    private func showRestartAlert() {
        let alert = NSAlert()
        alert.messageText = NSLocalizedString("settings.general.language_restart_title", comment: "Restart required")
        alert.informativeText = NSLocalizedString("settings.general.language_restart_message", comment: "Language will change after restart")
        alert.alertStyle = .informational
        alert.addButton(withTitle: NSLocalizedString("settings.general.language_restart_now", comment: "Restart Now"))
        alert.addButton(withTitle: NSLocalizedString("settings.general.language_restart_later", comment: "Later"))

        let response = alert.runModal()
        if response == .alertFirstButtonReturn {
            // User chose "Restart Now"
            NSApp.terminate(nil)
        }
    }
}
