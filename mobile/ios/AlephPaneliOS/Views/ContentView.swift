import SwiftUI

struct ContentView: View {
    @EnvironmentObject private var appState: AppState

    var body: some View {
        Group {
            switch appState.screen {
            case .pairing(let message):
                PairingView(initialText: appState.currentTargetString(), message: message)
            case .connected(let url):
                PanelWebView(url: url, onLoadFailure: { appState.requestReconfigure(message: $0) })
                    .ignoresSafeArea()
            }
        }
        .background(ShakeDetector { appState.requestReconfigure() })
        .task { await appState.resolve() }
    }
}
