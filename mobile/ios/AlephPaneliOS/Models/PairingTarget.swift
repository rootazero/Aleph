import Foundation

/// A validated connection target: the full URL of an `aleph-server` Gateway,
/// including any `?token=…`. Parsing mirrors the desktop lite shell's
/// `ConnectionTarget::parse` (default scheme http, default port 18790) so the
/// two shells share one onboarding format. iOS has no Local variant — the phone
/// shell never embeds a server.
struct PairingTarget: Equatable {
    let url: URL

    static let defaultPort: UInt16 = 18790

    /// Host of the target (without brackets for IPv6).
    var host: String {
        URLComponents(url: url, resolvingAgainstBaseURL: false)?.host ?? ""
    }

    /// Port of the target; falls back to `defaultPort` if somehow absent.
    var port: UInt16 {
        guard let p = URLComponents(url: url, resolvingAgainstBaseURL: false)?.port else {
            return Self.defaultPort
        }
        return UInt16(p)
    }

    /// Parse user/raw input into a target. See `PairingError` for rejections.
    static func parse(_ raw: String) -> Result<PairingTarget, PairingError> {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return .failure(.empty) }

        let withScheme = trimmed.contains("://") ? trimmed : "http://\(trimmed)"
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
        // URLComponents.port is non-nil only when the user wrote an explicit
        // port (it does NOT apply scheme defaults), so this cleanly injects the
        // default only when none was supplied.
        if components.port == nil {
            components.port = Int(Self.defaultPort)
        }
        guard let url = components.url else { return .failure(.invalidURL) }
        return .success(PairingTarget(url: url))
    }
}

enum PairingError: Error, Equatable {
    case empty
    case invalidURL
    case unsupportedScheme(String)
    case noHost
}
