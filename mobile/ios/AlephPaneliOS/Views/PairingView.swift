import SwiftUI

/// Native first-run / reconfigure screen. Transport config ONLY (which server
/// to connect to) — all app UI lives in the WASM panel (R2/R4). Styled to match
/// the desktop lite shell's `connect.html` wizard: a centered, bordered dark
/// card (no popup, no scrim). Shared by iPhone and iPad so every entry point —
/// desktop / iPhone / iPad — looks identical. Colors are hardcoded dark and do
/// not follow the system light/dark setting.
struct PairingView: View {
    @EnvironmentObject private var appState: AppState

    let initialText: String
    let message: String?

    @State private var address: String
    @State private var submitting = false

    // Desktop connect.html palette.
    private let screenBg = Color(hex: "0d0d10")
    private let cardBg = Color(hex: "17171c")
    private let border = Color(hex: "2a2a32")
    private let titleText = Color(hex: "e8e8ea")
    private let subtitleText = Color(hex: "9a9aa2")
    private let accent = Color(hex: "4f46e5")
    private let errorColor = Color(hex: "ff6b6b")

    init(initialText: String, message: String?) {
        self.initialText = initialText
        self.message = message
        _address = State(initialValue: initialText)
    }

    private var submitDisabled: Bool {
        submitting || address.trimmingCharacters(in: .whitespaces).isEmpty
    }

    var body: some View {
        ZStack {
            screenBg.ignoresSafeArea()

            VStack(spacing: 18) {
                HStack(spacing: 6) {
                    Text("✦").foregroundStyle(accent)
                    Text("Aleph").foregroundStyle(titleText)
                }
                .font(.title3).bold()

                card
            }
            .frame(maxWidth: 420)
            .padding(24)
        }
        // Force the whole pairing screen dark so the system TextField's default
        // placeholder reads as light-gray on the dark field regardless of the
        // device's light/dark setting (keeps the three shells identical).
        .preferredColorScheme(.dark)
    }

    private var card: some View {
        VStack(spacing: 16) {
            Text("Connect to Aleph")
                .font(.title2).bold()
                .foregroundStyle(titleText)

            Text("Enter your Aleph server address — e.g. 192.168.1.5 or https://gw.example.com")
                .font(.footnote)
                .foregroundStyle(subtitleText)
                .multilineTextAlignment(.center)

            TextField(
                "",
                text: $address,
                prompt: Text("host, host:port, or http(s)://host")
            )
            .textFieldStyle(.plain)
            .foregroundStyle(titleText)
            .tint(accent)
            .keyboardType(.URL)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled(true)
            .submitLabel(.go)
            .onSubmit(connect)
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(screenBg)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(border, lineWidth: 1))

            Button(action: connect) {
                Text(submitting ? "Connecting…" : "Connect")
                    .font(.body).bold()
                    .foregroundStyle(.white)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 11)
                    .background(accent)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
            }
            .buttonStyle(.plain)
            .disabled(submitDisabled)
            .opacity(submitDisabled ? 0.5 : 1)

            if let message {
                Text(message)
                    .font(.footnote)
                    .foregroundStyle(errorColor)
                    .multilineTextAlignment(.center)
            }
        }
        .padding(28)
        .background(cardBg)
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(border, lineWidth: 1))
        .shadow(color: .black.opacity(0.5), radius: 20, x: 0, y: 10)
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
