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
        // Settings scene that opens when using Cmd+,
        // Creates RootContentView with proper core reference from AppDelegate
        Settings {
            RootContentView(core: appDelegate.core)
                .environmentObject(appDelegate)
        }
    }
}
