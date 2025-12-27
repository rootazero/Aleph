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
                    Section(header: Text("Sound")) {
                        Toggle("Sound Effects", isOn: $soundEnabled)
                            .onChange(of: soundEnabled) { _, newValue in
                                showComingSoonAlert(feature: "Sound effects")
                            }
                    }

                    Section(header: Text("Updates")) {
                        Button("Check for Updates") {
                            checkForUpdates()
                        }
                        .help("Check for Aether updates")
                    }

                    Section(header: Text("Logs")) {
                        Button("View Logs") {
                            showingLogViewer = true
                        }
                        .help("View application logs")
                        .disabled(core == nil)
                    }

                    Section(header: Text("About")) {
                        HStack {
                            Text("Version:")
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
        alert.messageText = "Coming Soon"
        alert.informativeText = "\(feature) will be available in a future update."
        alert.alertStyle = .informational
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }

    private func checkForUpdates() {
        let alert = NSAlert()
        alert.messageText = "Check for Updates"
        alert.informativeText = """
        Current Version: \(appVersion)

        To check for updates, please visit:
        https://github.com/yourusername/aether/releases

        Automatic updates will be available in a future release.
        """
        alert.alertStyle = .informational
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Visit GitHub")

        let response = alert.runModal()
        if response == .alertSecondButtonReturn {
            if let url = URL(string: "https://github.com/yourusername/aether/releases") {
                NSWorkspace.shared.open(url)
            }
        }
    }
}
