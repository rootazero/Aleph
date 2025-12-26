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
    let core: AetherCore?
    let keychainManager: KeychainManagerImpl
    @State private var providers: [ProviderConfigEntry] = []
    @State private var configReloadTrigger: Int = 0 // Trigger to force UI refresh

    init(core: AetherCore? = nil, keychainManager: KeychainManagerImpl? = nil) {
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
                    GeneralSettingsView(core: core)
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
    @State private var soundEnabled = false
    @State private var showingLogViewer = false
    let core: AetherCore?

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

// MARK: - Preview

struct SettingsView_Previews: PreviewProvider {
    static var previews: some View {
        SettingsView()
    }
}
