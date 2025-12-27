//
//  AetherApp.swift
//  Aether
//
//  Main entry point for Aether macOS application.
//  This is a menu bar-only app (no Dock icon) that integrates with Rust core.
//

import SwiftUI

@main
struct AetherApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        #if DEBUG
        // MARK: - New Window Design (macOS 26 Style)
        // Debug mode: Use new WindowGroup with custom traffic lights
        WindowGroup {
            RootContentView(
                core: appDelegate.core,
                keychainManager: appDelegate.keychainManager
            )
            .frame(minWidth: 800, minHeight: 500)
        }
        .windowStyle(.hiddenTitleBar)
        .windowToolbarStyle(.unifiedCompact)
        .defaultSize(width: 1200, height: 800)
        .commands {
            // Remove default "New Window" command
            CommandGroup(replacing: .newItem) {}
        }
        #else
        // MARK: - Legacy Settings Window
        // Release mode: Use traditional Settings scene (fallback)
        Settings {
            SettingsView()
        }
        #endif
    }
}
