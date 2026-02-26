import SwiftUI

/// Main entry point for the Aleph macOS application.
///
/// Aleph runs as a menu bar app (LSUIElement = YES) without a Dock icon.
/// The AppDelegate handles menu bar setup, server lifecycle, and window management.
/// All complex UI is rendered via WKWebView pointing to the Leptos/WASM panel.
@main
struct AlephApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        // No main window — the app lives in the menu bar.
        // Settings and Halo windows are managed by AppDelegate.
        Settings {
            EmptyView()
        }
    }
}
