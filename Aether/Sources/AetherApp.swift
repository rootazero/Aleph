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
            .frame(minWidth: 980, minHeight: 750)  // Set minimum size to prevent layout distortion
            .environmentObject(appDelegate)  // Provide appDelegate as environment object
        }
        .windowStyle(.hiddenTitleBar)
        .windowToolbarStyle(.unifiedCompact)
        .defaultSize(width: 980, height: 750)  // Initial window size
        .commands {
            // Remove default "New Window" command
            CommandGroup(replacing: .newItem) {}
        }
    }
}
