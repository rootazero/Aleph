import Foundation

/// A validated connection target: the full URL of an `aleph-server` Gateway,
/// including any `?token=…`.
///
/// Parsing mirrors the desktop lite shell's `ConnectionTarget::parse`, and that
/// is a promise, not a remark: the two shells deliberately share one onboarding
/// format, so an address that works on the desktop must work here. It did not.
/// This file used to default a bare host to `https` **and** force the Aleph
/// listener port onto every URL, which combined into `https://host:18790` — an
/// origin no deployment serves. See ``defaultPort(hasTypedScheme:scheme:)``.
///
/// iOS has no Local variant — the phone shell never embeds a server.
struct PairingTarget: Equatable {
    let url: URL

    /// `aleph-server`'s own default listener. Applies only to a bare host; a
    /// typed `http://` / `https://` gets the scheme's default instead.
    static let alephListenerPort: UInt16 = 18790

    /// Host of the target (without brackets for IPv6).
    var host: String {
        URLComponents(url: url, resolvingAgainstBaseURL: false)?.host ?? ""
    }

    /// Port of the target. ``parse(_:)`` always writes an explicit port, so the
    /// fallback below is defensive only — and it resolves through the same
    /// scheme-aware rule rather than reaching for the listener default, which
    /// is how `https` and 18790 got paired in the first place.
    var port: UInt16 {
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        if let explicit = components?.port, let narrowed = UInt16(exactly: explicit) {
            return narrowed
        }
        return Self.defaultPort(hasTypedScheme: true, scheme: components?.scheme)
    }

    /// Scheme of the target; `http` when somehow absent (``parse(_:)`` always
    /// writes one).
    var scheme: String {
        URLComponents(url: url, resolvingAgainstBaseURL: false)?.scheme ?? "http"
    }

    /// The endpoint this target actually resolves to, port included.
    ///
    /// Every user-facing message goes through this one accessor so a message
    /// can never name a different endpoint than the one that was dialled —
    /// mirrors `gateway_probe::target_origin` on the desktop. The port is
    /// usually the one *we* filled in, so it is invisible in the address field,
    /// and a silently wrong default port is exactly how a correct-looking
    /// address fails.
    var origin: String {
        // Foundation's `.host` strips the brackets off an IPv6 literal (the
        // desktop's `Url::host_str` keeps them), so re-add them — without this
        // an IPv6 gateway is reported as `http://::1:18790`, which parses as
        // nothing and reads as a typo in our own message.
        let literal = host.contains(":") ? "[\(host)]" : host
        return "\(scheme)://\(literal):\(port)"
    }

    /// The port to assume when the user wrote none. Two different questions,
    /// two different answers — a straight port of the desktop shell's
    /// `connection::default_port_for`:
    ///
    ///   * A bare `host` / `host/path` means *"an aleph-server lives there"*,
    ///     whose default listener is ``alephListenerPort`` — the LAN case.
    ///   * An explicitly written `http://` / `https://` means *"this URL"*,
    ///     whose default port is the scheme's own (80 / 443). The gateway ships
    ///     with TLS off, so a typed `https://` almost always names a reverse
    ///     proxy or a CDN, and those live on 443.
    ///
    /// Injecting 18790 into a typed `https://host` rewrote a working
    /// `https://gw.example.com` into `https://gw.example.com:18790`, which no
    /// proxy forwards. The CDN edge still completed the TCP handshake before
    /// closing, so the old connect-only probe called it healthy and the phone
    /// navigated to a dead origin — with no way back but a shake gesture.
    static func defaultPort(hasTypedScheme: Bool, scheme: String?) -> UInt16 {
        guard hasTypedScheme else { return alephListenerPort }
        return scheme == "https" ? 443 : 80
    }

    /// Parse user/raw input into a target. See `PairingError` for rejections.
    static func parse(_ raw: String) -> Result<PairingTarget, PairingError> {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return .failure(.empty) }

        // A bare host means "an aleph-server lives there", and the gateway
        // ships with TLS off — so the assumed scheme is `http`, matching the
        // desktop shell. (ATS permits plaintext to local-network addresses via
        // `NSAllowsLocalNetworking`, which is what a bare host denotes; a bare
        // *public* hostname is then refused by the probe with an explanation
        // instead of failing later inside the webview.)
        let hasTypedScheme = trimmed.contains("://")
        let withScheme = hasTypedScheme ? trimmed : "http://\(trimmed)"
        guard var components = URLComponents(string: withScheme) else {
            return .failure(.invalidURL)
        }
        switch components.scheme {
        case "http", "https":
            break
        case let other?:
            return .failure(.unsupportedScheme(other))
        case nil:
            return .failure(.invalidURL)
        }
        guard let host = components.host, !host.isEmpty else {
            return .failure(.noHost)
        }
        // Reject an explicit port that overflows UInt16 — the user typed something
        // like :99999 which would crash the `port` accessor later.
        if let explicitPort = components.port, !(0...65535).contains(explicitPort) {
            return .failure(.invalidURL)
        }
        // URLComponents.port is non-nil only when the user wrote an explicit
        // port (it does NOT apply scheme defaults), so this cleanly injects a
        // default only when none was supplied — and which default depends on
        // whether they typed a scheme.
        if components.port == nil {
            components.port = Int(
                defaultPort(hasTypedScheme: hasTypedScheme, scheme: components.scheme)
            )
        }
        guard let url = components.url else { return .failure(.invalidURL) }
        return .success(PairingTarget(url: url))
    }

    /// What the pairing screen says when nothing answered at this target.
    ///
    /// Derived from ``origin`` so the endpoint it names is the one that was
    /// probed, and carrying the same three-way hint as the desktop connect page
    /// (`gateway_probe::unreachable_message`). The hint matters more than the
    /// failure: an `https` target on a non-default port is the reverse-proxy
    /// case, where the fix is to **remove** the port — telling that user to
    /// "try another port" sends them the wrong way, and every install upgrading
    /// from the build that force-injected 18790 lands exactly there.
    var unreachableMessage: String {
        let hint: String
        if scheme == "https" && port != 443 {
            hint = "If it is behind a reverse proxy or CDN, remove the port so "
                + "the default 443 is used (https://\(host))."
        } else if scheme == "https" {
            hint = "If the server listens on a different port, add it "
                + "explicitly (for example \(host):8443)."
        } else {
            hint = "If it is behind HTTPS or a reverse proxy, enter the full "
                + "URL (for example https://\(host)); to use a different port, "
                + "add it explicitly (for example \(host):18790)."
        }
        return "No Aleph server answered at \(origin). "
            + "Check the address and that the server is running. \(hint)"
    }
}

enum PairingError: Error, Equatable {
    case empty
    case invalidURL
    case unsupportedScheme(String)
    case noHost
}
