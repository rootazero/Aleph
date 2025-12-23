//
//  SettingsView.swift
//  Aether
//
//  Main settings interface with tabs for General, Providers, Routing, and Shortcuts.
//

import SwiftUI

enum SettingsTab {
    case general
    case providers
    case routing
    case shortcuts
}

struct SettingsView: View {
    @State private var selectedTab: SettingsTab = .general

    var body: some View {
        NavigationSplitView {
            // Sidebar with navigation list
            List(selection: $selectedTab) {
                Label("General", systemImage: "gear")
                    .tag(SettingsTab.general)

                Label("Providers", systemImage: "brain.head.profile")
                    .tag(SettingsTab.providers)

                Label("Routing", systemImage: "arrow.triangle.branch")
                    .tag(SettingsTab.routing)

                Label("Shortcuts", systemImage: "command")
                    .tag(SettingsTab.shortcuts)
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 200, max: 250)
            .listStyle(.sidebar)
        } detail: {
            // Detail view based on selection
            Group {
                switch selectedTab {
                case .general:
                    GeneralSettingsView()
                case .providers:
                    ProvidersView()
                case .routing:
                    RoutingView()
                case .shortcuts:
                    ShortcutsView()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

// MARK: - General Settings Tab

struct GeneralSettingsView: View {
    @State private var selectedTheme = "Cyberpunk"
    @State private var soundEnabled = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Form {
                    Section(header: Text("Appearance")) {
                        HStack {
                            Text("Theme:")
                                .frame(width: 100, alignment: .leading)
                            Picker("", selection: $selectedTheme) {
                                Text("Cyberpunk").tag("Cyberpunk")
                                Text("Zen").tag("Zen")
                                Text("Jarvis").tag("Jarvis")
                            }
                            .pickerStyle(.menu)
                            .frame(width: 150)
                            .onChange(of: selectedTheme) { newValue in
                                showComingSoonAlert(feature: "Theme customization")
                            }
                            Spacer()
                        }
                    }

                    Section(header: Text("Sound")) {
                        Toggle("Sound Effects", isOn: $soundEnabled)
                            .onChange(of: soundEnabled) { newValue in
                                showComingSoonAlert(feature: "Sound effects")
                            }
                    }

                    Section(header: Text("Updates")) {
                        Button("Check for Updates") {
                            showComingSoonAlert(feature: "Auto-update")
                        }
                    }

                    Section(header: Text("About")) {
                        HStack {
                            Text("Version:")
                            Spacer()
                            Text("0.1.0 (Phase 2)")
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
    }

    private func showComingSoonAlert(feature: String) {
        let alert = NSAlert()
        alert.messageText = "Coming Soon"
        alert.informativeText = "\(feature) will be available in a future update."
        alert.alertStyle = .informational
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }
}

// MARK: - Preview

struct SettingsView_Previews: PreviewProvider {
    static var previews: some View {
        SettingsView()
    }
}
