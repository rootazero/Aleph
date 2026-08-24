import Foundation

/// Whether an Aleph Gateway is actually answering at a target, asked before the
/// shell commits a navigation.
///
/// Takes the whole ``PairingTarget`` rather than a host/port pair on purpose:
/// the scheme is part of the endpoint, not decoration. Probing an `https://`
/// target over plaintext gets no reply at all, which the probe would then have
/// to report as "down" for a server that is perfectly healthy.
protocol ReachabilityProbing {
    func probe(_ target: PairingTarget) async -> Bool
}

/// Asks the Gateway's unauthenticated `/ready` endpoint over the target's own
/// scheme, and accepts only the two statuses `aleph-server` itself answers with.
///
/// The previous implementation opened a bare TCP connection and called a
/// completed handshake "reachable". That answers a *different question*, and
/// answering a different question here is what strands the user: anything
/// fronting an origin — a CDN edge, a load balancer, a port-forward to nothing —
/// completes the handshake and only then closes. The phone then navigated to a
/// dead origin, `WKWebView` showed its native "closed the connection" page, and
/// the only way out was the shake gesture. This is the same fix the desktop
/// shell's `gateway_probe::probe_reachable` carries; the two must not drift
/// again.
///
/// Two properties are deliberate:
///
///   * **Certificate validity is not part of the predicate.** A self-signed cert
///     is the documented default for a LAN gateway and trust is decided by the
///     `CertTrustStore` TOFU flow in `PanelWebView`, not by a liveness probe.
///     Validating here would report every self-signed server as down and send
///     its owner back to the pairing screen in a loop.
///   * **ATS still applies.** `URLSession` enforces App Transport Security, so a
///     cleartext target outside the local network fails the probe rather than
///     failing later inside the webview — and the pairing screen's `http` hint
///     ("enter the full URL, for example https://…") is exactly the right
///     advice for that case.
struct GatewayReadyProbe: ReachabilityProbing {
    /// The Gateway's unauthenticated readiness endpoint. `aleph-server` answers
    /// it the moment it binds the port.
    private static let readyPath = "/ready"

    /// 200 once ready, 503 while still booting. Both mean "this is our
    /// Gateway" — treating the booting case as down would bounce the user to
    /// the pairing screen during a perfectly normal restart.
    private static let gatewayStatuses: Set<Int> = [200, 503]

    let timeout: TimeInterval

    /// Sized from the desktop shell's measurement of a real remote Gateway
    /// behind a CDN (0.6–1.9s, cold TLS handshake at the top of that range),
    /// with headroom for a cellular first byte. The former 2s left no margin
    /// and would have reported a healthy server as down on a slow link — the
    /// same "guessed at the answer" failure this probe exists to remove. The
    /// cost is one-directional: only an *unreachable* target waits it out.
    init(timeout: TimeInterval = 5.0) {
        self.timeout = timeout
    }

    func probe(_ target: PairingTarget) async -> Bool {
        guard var components = URLComponents(url: target.url, resolvingAgainstBaseURL: false)
        else { return false }
        // The persisted target may carry a route and a `?token=…`. `/ready` is
        // unauthenticated, so strip both rather than leaking the token into a
        // request that has no use for it.
        components.path = Self.readyPath
        components.query = nil
        components.fragment = nil
        guard let url = components.url else { return false }

        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        // A cached 200 would happily "prove" a server that has since died.
        request.cachePolicy = .reloadIgnoringLocalAndRemoteCacheData
        request.timeoutInterval = timeout

        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = timeout
        configuration.timeoutIntervalForResource = timeout
        configuration.requestCachePolicy = .reloadIgnoringLocalAndRemoteCacheData
        let session = URLSession(
            configuration: configuration,
            delegate: AcceptAnyServerTrust(),
            delegateQueue: nil
        )
        // A delegate-bearing session retains its delegate until invalidated.
        defer { session.finishTasksAndInvalidate() }

        do {
            let (_, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse else { return false }
            return Self.gatewayStatuses.contains(http.statusCode)
        } catch {
            // Transport failure, ATS refusal, or timeout. All of them mean "we
            // could not get an answer", which is the only claim this predicate
            // is allowed to make — never "the server said no".
            return false
        }
    }
}

/// Accepts any server certificate **for the liveness probe only**.
///
/// This is not a trust decision and must never become one: the real decision
/// lives in `PanelWebView`'s TOFU handler, which pins a fingerprint and shows
/// the user a sheet. Rejecting self-signed certs here would make a LAN gateway
/// — the documented default deployment — permanently unreachable before the
/// user ever gets the chance to approve it.
///
/// Stateless, hence safe to hand to a `URLSession` on an arbitrary queue.
private final class AcceptAnyServerTrust: NSObject, URLSessionDelegate, @unchecked Sendable {
    func urlSession(
        _ session: URLSession,
        didReceive challenge: URLAuthenticationChallenge,
        completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust
        else {
            completionHandler(.performDefaultHandling, nil)
            return
        }
        completionHandler(.useCredential, URLCredential(trust: trust))
    }
}
