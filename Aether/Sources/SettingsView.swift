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
    case behavior
    case memory
}

struct SettingsView: View {
    @State private var selectedTab: SettingsTab = .general
    @ObservedObject var themeEngine: ThemeEngine
    let core: AetherCore?
    let keychainManager: KeychainManagerImpl
    @State private var providers: [ProviderConfigEntry] = []
    @State private var configReloadTrigger: Int = 0 // Trigger to force UI refresh

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

                Label("Behavior", systemImage: "slider.horizontal.3")
                    .tag(SettingsTab.behavior)

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
                            .id(configReloadTrigger) // Force re-render on config change
                    } else {
                        Text("Provider management requires AetherCore initialization")
                            .foregroundColor(.secondary)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                case .routing:
                    if let core = core {
                        RoutingView(core: core, providers: providers)
                            .id(configReloadTrigger) // Force re-render on config change
                    } else {
                        Text("Routing management requires AetherCore initialization")
                            .foregroundColor(.secondary)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                case .shortcuts:
                    ShortcutsView()
                case .behavior:
                    BehaviorSettingsView(core: core)
                        .id(configReloadTrigger) // Force re-render on config change
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
        .onAppear {
            loadProviders()
        }
        .onReceive(NotificationCenter.default.publisher(for: NSNotification.Name("AetherConfigDidChange"))) { _ in
            // Config file changed externally, reload configuration
            print("[SettingsView] Config change notification received, reloading...")
            handleConfigChange()
        }
    }

    private func loadProviders() {
        guard let core = core else { return }

        Task {
            do {
                let config = try core.loadConfig()
                await MainActor.run {
                    providers = config.providers
                }
            } catch {
                print("Failed to load providers: \(error)")
            }
        }
    }

    private func handleConfigChange() {
        // Reload providers
        loadProviders()

        // Increment trigger to force UI refresh
        configReloadTrigger += 1

        // Optional: Show visual feedback
        print("[SettingsView] Configuration reloaded from file")
    }
}

// MARK: - General Settings Tab

struct GeneralSettingsView: View {
    @ObservedObject var themeEngine: ThemeEngine
    @State private var soundEnabled = false

    // Dynamic version from Info.plist
    private var appVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "Unknown"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "Unknown"
        return "\(version) (Build \(build))"
    }

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
                            checkForUpdates()
                        }
                        .help("Check for Aether updates")
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
        // TODO: Integrate Sparkle auto-update framework
        // This is a placeholder implementation for Phase 6.1
        //
        // Full Sparkle integration requires:
        // 1. Add Sparkle dependency to project.yml (SPM or framework)
        // 2. Initialize SPUUpdater in AppDelegate
        // 3. Set up appcast.xml feed (update manifest)
        // 4. Configure code signing for updates
        // 5. Set up update server infrastructure
        //
        // For now, show a simple alert with manual update instructions

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
            // Open GitHub releases page
            if let url = URL(string: "https://github.com/yourusername/aether/releases") {
                NSWorkspace.shared.open(url)
            }
        }
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
