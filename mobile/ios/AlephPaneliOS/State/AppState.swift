import Foundation

/// A pending server-trust approval raised by `PanelWebView`'s TLS-challenge
/// handler. Carries the display facts plus the one-shot `decide` callback that
/// resolves the underlying `WKWebView` auth challenge (`true` = trust + pin).
/// Purely a transport concern (R4) — no business state.
struct CertPromptRequest: Identifiable {
    let id = UUID()
    let host: String
    let fingerprint: String
    let subject: String
    let sans: [String]
    let reason: String
    /// The previously-pinned fingerprint when the cert changed (possible MITM);
    /// `nil` for a first-seen host. Drives the prominent warning banner.
    let changedFrom: String?
    let decide: (Bool) -> Void
}

/// Drives which screen the shell shows: the native pairing screen (transport
/// config only) or the WASM panel. Holds no business state — that all lives in
/// the panel (R2/R4).
@MainActor
final class AppState: ObservableObject {
    enum Screen: Equatable {
        case pairing(message: String?)
        case connected(URL)
    }

    @Published private(set) var screen: Screen = .pairing(message: nil)

    /// A self-signed-cert approval awaiting the user's decision, or `nil`. Drives
    /// the `CertTrustSheet` presented over the panel.
    @Published var pendingCert: CertPromptRequest?

    private let store: ConnectionStoring
    private let probe: ReachabilityProbing
    private let envURL: () -> String?

    init(
        store: ConnectionStoring,
        probe: ReachabilityProbing,
        envURL: @escaping () -> String? = { ProcessInfo.processInfo.environment["PANEL_URL"] }
    ) {
        self.store = store
        self.probe = probe
        self.envURL = envURL
    }

    /// Startup resolution: env wins (dev/sim injection), then the persisted
    /// target, else the pairing screen. A persisted/env target is probed before
    /// navigating — unreachable falls back to pairing instead of a dead webview.
    func resolve() async {
        if let env = envURL(), !env.isEmpty,
           case .success(let target) = PairingTarget.parse(env) {
            try? store.save(target.url)
            await connectOrPair(target)
            return
        }
        if let saved = store.load() {
            await connectOrPair(PairingTarget(url: saved))
            return
        }
        screen = .pairing(message: nil)
    }

    /// Validate + probe a user-entered address; persist + connect on success,
    /// otherwise stay on the pairing screen with an inline message.
    func submit(_ raw: String) async {
        switch PairingTarget.parse(raw) {
        case .failure(let error):
            screen = .pairing(message: Self.message(for: error))
        case .success(let target):
            if await probe.probe(target) {
                try? store.save(target.url)
                screen = .connected(target.url)
            } else {
                // `unreachableMessage` names the resolved origin and says what
                // to change. The old wording was `host:port is not reachable`,
                // which stated the failure and withheld every fact the user
                // needed: which scheme was dialled, and that the way out of a
                // reverse-proxy failure is to *remove* the port.
                screen = .pairing(message: target.unreachableMessage)
            }
        }
    }

    /// Reveal the pairing screen on demand (shake gesture).
    func requestReconfigure(message: String? = nil) {
        screen = .pairing(message: message)
    }

    /// The webview failed to load a target the probe had just approved — a
    /// server that died between the two, a TLS trust prompt the user declined,
    /// or an ATS refusal.
    ///
    /// Distinct from ``requestReconfigure(message:)`` because a bare
    /// `error.localizedDescription` is a system string with no subject: "The
    /// operation couldn't be completed" says nothing about *which* address
    /// failed, and this screen's whole job is to let the user fix the address.
    /// The origin comes from the same accessor every other message uses, so all
    /// three surfaces name the endpoint the same way.
    func reportLoadFailure(url: URL, detail: String) {
        screen = .pairing(message: "Could not load \(PairingTarget(url: url).origin) — \(detail)")
    }

    /// Raise a server-trust approval sheet (called from the webview's TLS hook).
    func presentCertPrompt(_ request: CertPromptRequest) {
        pendingCert = request
    }

    /// Resolve the pending trust prompt: run its one-shot decision, then dismiss
    /// the sheet. `true` trusts + pins the cert; `false`/dismiss fails the load
    /// closed. No-op if nothing is pending.
    func resolvePendingCert(_ approved: Bool) {
        guard let request = pendingCert else { return }
        pendingCert = nil
        request.decide(approved)
    }

    /// Current persisted target as a prefill string for the pairing field.
    func currentTargetString() -> String {
        store.load()?.absoluteString ?? ""
    }

    private func connectOrPair(_ target: PairingTarget) async {
        if await probe.probe(target) {
            screen = .connected(target.url)
        } else {
            // Same sentence as a hand-typed failure, and for the same reason:
            // the previous copy ("Last server unreachable") named nothing at
            // all, so a user whose stored target carried a port they never
            // typed had no way to see it — the field shows the address, never
            // the resolved origin.
            screen = .pairing(message: target.unreachableMessage)
        }
    }

    private static func message(for error: PairingError) -> String {
        switch error {
        case .empty: return "Enter a server address"
        case .invalidURL: return "That doesn't look like a valid address"
        case .unsupportedScheme(let s): return "Unsupported scheme: \(s)"
        case .noHost: return "Address is missing a host"
        }
    }
}
