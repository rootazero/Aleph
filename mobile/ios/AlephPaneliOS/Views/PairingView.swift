import SwiftUI

/// Native first-run / reconfigure screen. Transport config ONLY (which server
/// to connect to) — all app UI lives in the WASM panel (R2/R4). Mirrors the
/// desktop lite shell's `connect.html` manual-entry card.
struct PairingView: View {
    @EnvironmentObject private var appState: AppState

    let initialText: String
    let message: String?

    @State private var address: String
    @State private var submitting = false

    init(initialText: String, message: String?) {
        self.initialText = initialText
        self.message = message
        _address = State(initialValue: initialText)
    }

    var body: some View {
        VStack(spacing: 16) {
            Text("Connect to Aleph")
                .font(.title2).bold()
            Text("Enter your Aleph server address — e.g. 192.168.1.5 or http://gw.example.com")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)

            TextField("host, host:port, or http(s)://host", text: $address)
                .textFieldStyle(.roundedBorder)
                .keyboardType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled(true)
                .submitLabel(.go)
                .onSubmit(connect)

            Button(action: connect) {
                Text(submitting ? "Connecting…" : "Connect")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .disabled(submitting || address.trimmingCharacters(in: .whitespaces).isEmpty)

            if let message {
                Text(message)
                    .font(.footnote)
                    .foregroundStyle(.red)
                    .multilineTextAlignment(.center)
            }
        }
        .padding(28)
        .frame(maxWidth: 420)
    }

    private func connect() {
        guard !submitting else { return }
        submitting = true
        Task {
            await appState.submit(address)
            submitting = false
        }
    }
}
