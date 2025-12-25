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
    case memory
}

struct SettingsView: View {
    @State private var selectedTab: SettingsTab = .general
    @ObservedObject var themeEngine: ThemeEngine
    let core: AetherCore?
    let keychainManager: KeychainManagerImpl

    init(themeEngine: ThemeEngine, core: AetherCore? = nil, keychainManager: KeychainManagerImpl? = nil) {
        self.themeEngine = themeEngine
        self.core = core
        self.keychainManager = keychainManager ?? KeychainManagerImpl()
    }

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

                Label("Memory", systemImage: "brain")
                    .tag(SettingsTab.memory)
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 200, max: 250)
            .listStyle(.sidebar)
        } detail: {
            // Detail view based on selection
            Group {
                switch selectedTab {
                case .general:
                    GeneralSettingsView(themeEngine: themeEngine)
                case .providers:
                    if let core = core {
                        ProvidersView(core: core, keychainManager: keychainManager)
                    } else {
                        Text("Provider management requires AetherCore initialization")
                            .foregroundColor(.secondary)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                case .routing:
                    RoutingView()
                case .shortcuts:
                    ShortcutsView()
                case .memory:
                    if let core = core {
                        MemoryView(core: core)
                    } else {
                        Text("Memory management requires AetherCore initialization")
                            .foregroundColor(.secondary)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

// MARK: - General Settings Tab

struct GeneralSettingsView: View {
    @ObservedObject var themeEngine: ThemeEngine
    @State private var soundEnabled = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Form {
                    Section(header: Text("Appearance")) {
                        VStack(alignment: .leading, spacing: 16) {
                            HStack {
                                Text("Theme:")
                                    .frame(width: 100, alignment: .leading)
                                Picker("", selection: $themeEngine.currentTheme) {
                                    ForEach(Theme.allCases, id: \.self) { theme in
                                        Text(theme.displayName).tag(theme)
                                    }
                                }
                                .pickerStyle(.segmented)
                                .frame(width: 300)
                                Spacer()
                            }

                            // Theme preview cards
                            HStack(spacing: 16) {
                                ForEach(Theme.allCases, id: \.self) { theme in
                                    ThemePreviewCard(
                                        theme: theme,
                                        isSelected: themeEngine.currentTheme == theme
                                    )
                                    .onTapGesture {
                                        themeEngine.setTheme(theme)
                                    }
                                }
                            }
                            .padding(.top, 8)
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
                            Text("0.1.0 (Phase 3)")
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

// MARK: - Theme Preview Card

struct ThemePreviewCard: View {
    let theme: Theme
    let isSelected: Bool

    var body: some View {
        VStack(spacing: 8) {
            // Preview image placeholder
            ZStack {
                RoundedRectangle(cornerRadius: 8)
                    .fill(previewBackgroundColor)
                    .frame(width: 120, height: 80)
                    .overlay(
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(isSelected ? Color.accentColor : Color.gray.opacity(0.3), lineWidth: isSelected ? 3 : 1)
                    )

                // Mini theme preview
                Group {
                    switch theme {
                    case .zen:
                        Circle()
                            .stroke(Color.white.opacity(0.8), lineWidth: 2)
                            .frame(width: 30, height: 30)
                    case .cyberpunk:
                        // Hexagon outline
                        Text("⬡")
                            .font(.system(size: 40))
                            .foregroundColor(Color.cyan)
                    case .jarvis:
                        // Arc reactor style
                        ZStack {
                            ForEach(0..<6) { i in
                                Rectangle()
                                    .fill(Color(red: 0.0, green: 0.83, blue: 1.0))
                                    .frame(width: 2, height: 15)
                                    .offset(y: -15)
                                    .rotationEffect(.degrees(Double(i) * 60))
                            }
                            Circle()
                                .fill(Color(red: 0.0, green: 0.83, blue: 1.0))
                                .frame(width: 12, height: 12)
                        }
                    }
                }
            }

            Text(theme.displayName)
                .font(.caption)
                .foregroundColor(isSelected ? .primary : .secondary)
        }
    }

    private var previewBackgroundColor: Color {
        switch theme {
        case .zen:
            return Color(red: 0.95, green: 0.95, blue: 0.97)
        case .cyberpunk:
            return Color(red: 0.1, green: 0.1, blue: 0.15)
        case .jarvis:
            return Color(red: 0.05, green: 0.08, blue: 0.12)
        }
    }
}

// MARK: - Preview

struct SettingsView_Previews: PreviewProvider {
    static var previews: some View {
        SettingsView(themeEngine: ThemeEngine())
    }
}
