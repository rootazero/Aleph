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
        // Settings window with macOS 26 design language
        WindowGroup {
            RootContentView(
                core: appDelegate.core,
                keychainManager: appDelegate.keychainManager
            )
            .frame(minWidth: 800, minHeight: 500)
            .environmentObject(appDelegate)  // Provide appDelegate as environment object
        }
        .windowStyle(.hiddenTitleBar)
        .windowToolbarStyle(.unifiedCompact)
        .defaultSize(width: 1200, height: 800)
        .commands {
            // Remove default "New Window" command
            CommandGroup(replacing: .newItem) {}
        }
    }
}
