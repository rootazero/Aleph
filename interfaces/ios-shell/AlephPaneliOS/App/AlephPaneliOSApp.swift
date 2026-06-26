import SwiftUI

/// Thin native shell for the Aleph phone panel.
///
/// This app is *not* a UI rewrite — it is a full-screen `WKWebView` that loads
/// the existing Leptos/WASM panel served by an `aleph-server`. It exists so the
/// phone form-factor (<640px) can be exercised in a real iPhone simulator /
/// device, matching the "WASM panel + thin native shell" iOS decision.
@main
struct AlephPaneliOSApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}
