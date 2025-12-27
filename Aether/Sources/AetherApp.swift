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
        WindowGroup(id: "settings") {
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
        .handlesExternalEvents(matching: Set(arrayLiteral: "settings"))  // Allow reopening via URL
        .commands {
            // Keep the New Window command but redirect it to open settings window
            CommandGroup(replacing: .newItem) {
                Button("New Window") {
                    // Open settings window via URL
                    if let url = URL(string: "aether://settings") {
                        NSWorkspace.shared.open(url)
                    }
                }
                .keyboardShortcut("n", modifiers: .command)
            }
        }
    }
}
