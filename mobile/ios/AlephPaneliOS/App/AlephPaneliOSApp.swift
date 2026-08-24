import SwiftUI

/// Thin native shell for the Aleph phone panel. A full-screen `WKWebView` over
/// the WASM panel served by an `aleph-server`; the native layer only handles
/// transport config (which server to connect to). See R2/R6 in the root CLAUDE.md.
@main
struct AlephPaneliOSApp: App {
    @StateObject private var appState = AppState(
        store: KeychainConnectionStore(),
        probe: GatewayReadyProbe()
    )

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(appState)
        }
    }
}
