import SwiftUI

struct ContentView: View {
    /// Panel target — resolved entirely at runtime so **no secret (server IP or
    /// token) is ever committed to source** (safe to push to git). Resolution:
    ///   1. `PANEL_URL` launch env (`SIMCTL_CHILD_PANEL_URL=…` for testing); when
    ///      present it is also persisted, so later tap-to-open reuses it;
    ///   2. the previously-persisted target (`UserDefaults`, lives only on-device);
    ///   3. nothing configured → a blank page.
    /// The Debian URL+token is injected at launch (kept in a gitignored helper),
    /// never hardcoded here. Connects to the remote Debian core like the desktop
    /// lite panel; no local server is run.
    private static let storeKey = "panelURL"

    private var panelURL: URL {
        let defaults = UserDefaults.standard
        if let env = ProcessInfo.processInfo.environment["PANEL_URL"],
           !env.isEmpty, let u = URL(string: env) {
            defaults.set(env, forKey: Self.storeKey) // remember for tap-to-open
            return u
        }
        if let saved = defaults.string(forKey: Self.storeKey), let u = URL(string: saved) {
            return u
        }
        return URL(string: "about:blank")!
    }

    var body: some View {
        PanelWebView(url: panelURL)
            .ignoresSafeArea()
    }
}
