import SwiftUI

struct ContentView: View {
    @EnvironmentObject private var appState: AppState

    var body: some View {
        Group {
            switch appState.screen {
            case .pairing(let message):
                PairingView(initialText: appState.currentTargetString(), message: message)
            case .connected(let url):
                PanelWebView(
                    url: url,
                    onLoadFailure: { appState.reportLoadFailure(url: url, detail: $0) },
                    onCertPrompt: { appState.presentCertPrompt($0) }
                )
                .ignoresSafeArea()
            }
        }
        .background(ShakeDetector { appState.requestReconfigure() })
        .sheet(item: $appState.pendingCert) { request in
            // The two buttons are the only exits — an interactive swipe-dismiss
            // would drop the held TLS challenge without resolving it.
            CertTrustSheet(request: request) { approved in
                appState.resolvePendingCert(approved)
            }
            .interactiveDismissDisabled()
        }
        .task { await appState.resolve() }
    }
}
