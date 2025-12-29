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
    @State private var soundEnabled = false
    @State private var showingLogViewer = false
    let core: AetherCore?

    private var appVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "Unknown"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "Unknown"
        return "\(version) (Build \(build))"
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Form {
                    Section(header: Text("settings.general.sound")) {
                        Toggle("settings.general.sound_effects", isOn: $soundEnabled)
                            .onChange(of: soundEnabled) { _, newValue in
                                showComingSoonAlert(feature: NSLocalizedString("settings.general.sound_effects", comment: "Sound effects feature"))
                            }
                    }

                    Section(header: Text("settings.general.updates")) {
                        Button("settings.general.check_updates") {
                            checkForUpdates()
                        }
                        .help(NSLocalizedString("settings.general.check_updates_help", comment: ""))
                    }

                    Section(header: Text("settings.general.logs")) {
                        Button("settings.general.view_logs") {
                            showingLogViewer = true
                        }
                        .help(NSLocalizedString("settings.general.view_logs_help", comment: ""))
                        .disabled(core == nil)
                    }

                    Section(header: Text("settings.general.about")) {
                        HStack {
                            Text("settings.general.version")
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
}
