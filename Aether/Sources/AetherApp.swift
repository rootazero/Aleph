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
        // Empty Settings scene to satisfy SwiftUI App requirement
        // Actual settings window is managed by AppDelegate.showSettings()
        Settings {
            EmptyView()
        }
    }
}
