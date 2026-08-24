//! Is an Aleph Gateway actually answering at a given target — and, when it is
//! not, the sentence that says so.
//!
//! Shared by **both** shell variants on purpose. The panel-only shell probes
//! before every navigation; the full app probes its remote leg (menu / tray →
//! "Connect to Remote…") and its resident supervisor. Those are the same
//! question, and answering it in two places is how one of them ends up
//! answering a *different* question — which is precisely the defect this
//! module was extracted to retire: the full app's remote leg used a bare TCP
//! connect, so a CDN edge that accepts and then closes read as "healthy"
//! forever and its connect page never appeared.
//!
//! Nothing here talks to Tauri or to a window: it is a predicate plus the
//! wording derived from the same resolution step, so the two are unit-testable
//! without a live socket and cannot describe different endpoints.

use std::time::Duration;

use crate::connection;

/// Upper bound on the pre-navigation reachability probe — one TLS handshake
/// plus one HTTP round trip (see [`probe_reachable`]), not a bare TCP connect.
/// A server that accepts the connection but never speaks cannot hang the probe.
///
/// Sized from measurement, not from the loopback case it used to serve: a real
/// remote Gateway behind a CDN measured 0.6–1.9s here (cold TLS handshake at the
/// top of that range), so the former 2s budget left no margin and would have
/// reported a perfectly healthy server as down on a slow or cold connection —
/// the same "guessed at the answer" failure this probe exists to remove. The
/// cost of the larger budget is bounded and one-directional: only an
/// *unreachable* target waits the full timeout before the connect page appears.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The Gateway's unauthenticated readiness endpoint. `aleph-server` answers it
/// the moment it binds the port: 200 once ready, 503 while still booting.
const READY_PATH: &str = "/ready";

/// Whether an Aleph Gateway is actually answering at `scheme://host:port`.
///
/// A bare TCP connect cannot answer this question, and answering a *different*
/// question here is what strands the user. Anything sitting in front of an
/// origin — a CDN edge, a load balancer, a port-forward to nothing — completes
/// the TCP handshake and only then closes, so a connect-only probe reports
/// "healthy" for a destination that serves no HTTP at all. The shell then
/// commits the navigation, the webview shows its native "closed the connection"
/// page, and the health supervisor keeps seeing an open port — so it never
/// relocates to the connect page and the user has no way back.
///
/// So ask what we actually need to know: *does an aleph-server reply here*.
/// This is the remote-capable sibling of the full-app `daemon::probe_port`,
/// whose doc already stated the rule ("no usable HTTP at all means a foreign
/// process holds the port"); that module hardcodes the loopback host and port
/// and is compiled out of the panel-only shell, which is why the remote leg
/// never inherited it.
///
/// Certificate validity is deliberately **not** part of this predicate. A
/// self-signed cert is the documented default for a LAN gateway and trust is
/// decided by the `cert_trust` TOFU flow, not by a liveness probe — validating
/// here would report every self-signed server as down and send its owner to the
/// connect page in a loop.
async fn probe_reachable(scheme: &str, host: &str, port: u16, timeout: Duration) -> bool {
    let status = tokio::time::timeout(timeout, ready_status(scheme, host, port)).await;
    match status {
        Ok(Some(code @ (200 | 503))) => {
            tracing::debug!("gateway probe {scheme}://{host}:{port}{READY_PATH} → {code}");
            true
        }
        // Reachable TCP but not an Aleph Gateway (a CDN 404, a stray web
        // server, a proxy error page). Logged with the status so the reason a
        // target was declared down is visible instead of guessed at.
        Ok(Some(code)) => {
            tracing::warn!(
                "{scheme}://{host}:{port} answered {code} at {READY_PATH} — not an Aleph Gateway"
            );
            false
        }
        Ok(None) => {
            tracing::warn!("{scheme}://{host}:{port} accepted the connection but served no HTTP");
            false
        }
        Err(_) => {
            tracing::warn!("{scheme}://{host}:{port} did not answer within {timeout:?}");
            false
        }
    }
}

/// One HTTP/1.0 `GET /ready`, over TLS when `scheme` is `https`, returning just
/// the numeric status. `None` for any transport failure or a reply that is not
/// recognisable HTTP. Unbounded — always call it through the timeout wrapper in
/// [`probe_reachable`] so a peer that accepts and then goes silent cannot hang
/// the shell.
async fn ready_status(scheme: &str, host: &str, port: u16) -> Option<u16> {
    let stream = tokio::net::TcpStream::connect((host, port)).await.ok()?;
    let request =
        format!("GET {READY_PATH} HTTP/1.0\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");

    if scheme == "https" {
        let connector = native_tls::TlsConnector::builder()
            // See the note on cert validity in `probe_reachable`.
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()
            .ok()?;
        // `connect` carries `host` as SNI, which a CDN or a name-based vhost
        // needs in order to route the request at all.
        let mut tls = tokio_native_tls::TlsConnector::from(connector)
            .connect(host, stream)
            .await
            .ok()?;
        http_status(&mut tls, &request).await
    } else {
        let mut stream = stream;
        http_status(&mut stream, &request).await
    }
}

/// Write `request` and parse the numeric status out of the reply's first line
/// (e.g. `HTTP/1.1 200 OK` → `200`). The status line always arrives in the
/// first segment, so a single bounded read is enough.
async fn http_status<S>(io: &mut S, request: &str) -> Option<u16>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    io.write_all(request.as_bytes()).await.ok()?;
    let mut chunk = [0u8; 512];
    let n = io.read(&mut chunk).await.ok()?;
    let head = String::from_utf8_lossy(chunk.get(..n)?);
    // Reject anything that is not an HTTP status line, so a raw byte stream
    // (SSH, a TLS record on a plaintext port) cannot be read as a status.
    let mut parts = head.split_whitespace();
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

/// The `(scheme, host, port)` to probe for a target. `Local` resolves to the
/// loopback Gateway; `Remote` to its URL's scheme/host/port. Extracted (pure)
/// so the routing decision is unit-testable without a live socket.
///
/// The scheme is part of the endpoint, not decoration: probing an `https://`
/// target over plaintext gets no HTTP reply at all, which the probe would then
/// have to report as "down" for a server that is perfectly healthy.
fn probe_endpoint(target: &connection::ConnectionTarget) -> (String, String, u16) {
    match target {
        connection::ConnectionTarget::Local => ("http".to_string(), "127.0.0.1".to_string(), 18790),
        connection::ConnectionTarget::Remote(url) => {
            let host = url.host_str().unwrap_or("127.0.0.1").to_string();
            let port = url.port_or_known_default().unwrap_or(18790);
            (url.scheme().to_string(), host, port)
        }
    }
}

/// Whether an Aleph Gateway is actually answering at the given target. The one
/// predicate every "should we navigate there / is it still up" caller uses, in
/// both variants. Bounded by [`PROBE_TIMEOUT`].
pub async fn target_reachable(target: &connection::ConnectionTarget) -> bool {
    target_reachable_within(target, PROBE_TIMEOUT).await
}

/// [`target_reachable`] with an explicit budget. Deliberately **not** `pub`:
/// no caller should be able to shorten this budget, because a budget below the
/// measured CDN handshake range reports a healthy server as down — the exact
/// failure [`PROBE_TIMEOUT`] documents. It exists so the tests can exercise the
/// public entry point without waiting five seconds per case.
async fn target_reachable_within(target: &connection::ConnectionTarget, timeout: Duration) -> bool {
    let (scheme, host, port) = probe_endpoint(target);
    probe_reachable(&scheme, &host, port, timeout).await
}

/// The origin string every user-facing message names. One source, so a message
/// can never describe a different endpoint than the one [`probe_endpoint`]
/// resolved — the whole point is that the port shown is the port dialled.
fn origin(scheme: &str, host: &str, port: u16) -> String {
    format!("{scheme}://{host}:{port}")
}

/// The origin a target resolves to, for callers phrasing their own message
/// (the supervisor's "lost contact" wording differs from a cold failure's).
pub fn target_origin(target: &connection::ConnectionTarget) -> String {
    let (scheme, host, port) = probe_endpoint(target);
    origin(&scheme, &host, port)
}

/// Why a target could not be reached, phrased for the connect page. Goes
/// through the same [`probe_endpoint`] the probe itself used, so the origin it
/// names is the one that was actually dialled — including a port the user
/// never typed. That port is invisible in the address field and is precisely
/// what makes this class of failure unexplainable from the UI alone.
pub fn target_unreachable_message(target: &connection::ConnectionTarget) -> String {
    let (scheme, host, port) = probe_endpoint(target);
    unreachable_message(&scheme, &host, port)
}

/// What the connect page tells the user when a target does not answer.
///
/// It names the **exact origin that was probed**, port included. The port is
/// usually the one the shell filled in rather than one the user typed, so it is
/// invisible in the address field — and a silently wrong default port is
/// precisely how a correct-looking address fails. Showing the resolved origin
/// turns "it just doesn't work" into something the user can compare against
/// what they expected.
///
/// Pure so the wording is testable without a socket.
fn unreachable_message(scheme: &str, host: &str, port: u16) -> String {
    let hint = if scheme == "https" && port != 443 {
        // A non-default port under https is the reverse-proxy/CDN failure: the
        // proxy fronts 443 and does not forward this one. Telling the user to
        // "try another port" sends them the wrong way — the fix is to drop it.
        // Every install that ran the build which force-injected 18790 wakes up
        // here after upgrading, so this is the common case, not a corner.
        format!(
            "If it is behind a reverse proxy or CDN, remove the port so the \
             default 443 is used (https://{host})."
        )
    } else if scheme == "https" {
        format!(
            "If the server listens on a different port, add it explicitly \
             (for example {host}:8443)."
        )
    } else {
        format!(
            "If it is behind HTTPS or a reverse proxy, enter the full URL \
             (for example https://{host}); to use a different port, add it \
             explicitly (for example {host}:18790)."
        )
    };
    format!(
        "No Aleph server answered at {}. \
         Check the address and that the server is running. {hint}",
        origin(scheme, host, port)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_endpoint_local_is_loopback() {
        let (scheme, host, port) = probe_endpoint(&connection::ConnectionTarget::Local);
        assert_eq!(scheme, "http");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 18790);
    }

    #[test]
    fn probe_endpoint_remote_uses_url_host_port() {
        let target = connection::ConnectionTarget::parse("box.lan:9000").unwrap();
        let (scheme, host, port) = probe_endpoint(&target);
        assert_eq!(scheme, "http");
        assert_eq!(host, "box.lan");
        assert_eq!(port, 9000);
    }

    #[test]
    fn probe_endpoint_remote_default_port() {
        let target = connection::ConnectionTarget::parse("gw.example.com").unwrap();
        let (scheme, host, port) = probe_endpoint(&target);
        assert_eq!(scheme, "http");
        assert_eq!(host, "gw.example.com");
        assert_eq!(port, 18790);
    }

    /// An `https://` target must be probed over TLS on its own port — carrying
    /// the scheme is what lets the probe speak the protocol the server expects.
    #[test]
    fn probe_endpoint_keeps_the_targets_scheme() {
        let target = connection::ConnectionTarget::parse("https://gw.example.com").unwrap();
        let (scheme, host, port) = probe_endpoint(&target);
        assert_eq!(scheme, "https");
        assert_eq!(host, "gw.example.com");
        assert_eq!(port, 443);
    }

    #[tokio::test]
    async fn probe_unreachable_port_times_out_false() {
        // Port 1 on loopback is reliably closed; a short timeout keeps the
        // test fast while still exercising the connect path.
        let reachable = probe_reachable("http", "127.0.0.1", 1, Duration::from_millis(200)).await;
        assert!(!reachable);
    }

    /// The regression that stranded the user: a peer that completes the TCP
    /// handshake and then closes without speaking HTTP — exactly what a CDN
    /// edge, a load balancer, or a port-forward to nothing does — must be
    /// reported as DOWN. Reporting it up made the shell navigate to a dead
    /// origin, show the webview's native error page, and then keep believing
    /// the target was healthy, so the fallback connect page never appeared.
    #[tokio::test]
    async fn a_socket_that_accepts_but_serves_no_http_is_not_reachable() {
        let port = accept_and_close().await;
        let reachable =
            probe_reachable("http", "127.0.0.1", port, Duration::from_millis(500)).await;
        assert!(
            !reachable,
            "a socket that speaks no HTTP must not be reported as a live Gateway"
        );
    }

    /// A live HTTP server that is not an Aleph Gateway (a stray web server, a
    /// proxy's own 404 page) is likewise not a target worth navigating to.
    #[tokio::test]
    async fn a_non_gateway_http_responder_is_not_reachable() {
        let port = serve_one_status("HTTP/1.1 404 Not Found").await;
        let reachable =
            probe_reachable("http", "127.0.0.1", port, Duration::from_millis(500)).await;
        assert!(!reachable, "a 404 at /ready is not an Aleph Gateway");
    }

    /// The two statuses `aleph-server` itself answers with: 200 once ready and
    /// 503 while still booting. Both mean "this is our Gateway" — treating the
    /// booting case as down would bounce the user to the connect page during a
    /// perfectly normal restart.
    #[tokio::test]
    async fn a_gateway_answering_ready_is_reachable() {
        for line in ["HTTP/1.1 200 OK", "HTTP/1.1 503 Service Unavailable"] {
            let port = serve_one_status(line).await;
            let reachable =
                probe_reachable("http", "127.0.0.1", port, Duration::from_millis(500)).await;
            assert!(reachable, "{line} at /ready must count as a live Gateway");
        }
    }

    /// Guard on the *public* entry point, not just the private predicate: the
    /// full app reached reachability through its own door (`daemon::
    /// tcp_reachable`) and so kept the TCP-only answer for four rounds after
    /// the lite shell was fixed. Both variants now come through here.
    #[tokio::test]
    async fn target_reachable_rejects_a_socket_that_serves_no_http() {
        let port = accept_and_close().await;
        let target =
            connection::ConnectionTarget::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        assert!(
            !target_reachable_within(&target, Duration::from_millis(500)).await,
            "an open-but-silent port must not read as a live Gateway through \
             the public entry point either"
        );
    }

    /// The failure message must name the resolved origin — the port the shell
    /// filled in is invisible in what the user typed, and a wrong default port
    /// is exactly how a correct-looking address fails.
    #[test]
    fn the_unreachable_message_names_the_origin_it_probed() {
        let msg = unreachable_message("https", "aleph.example.com", 18790);
        assert!(
            msg.contains("https://aleph.example.com:18790"),
            "message must show the probed origin, got: {msg}"
        );
        // https on a non-443 port is the reverse-proxy case: the way out is to
        // drop the port, never to try yet another one.
        assert!(
            msg.contains("remove the port") && msg.contains("https://aleph.example.com)"),
            "must steer toward dropping the port, got: {msg}"
        );
        // …but a failure already on 443 has no port to drop, so that advice
        // would be a no-op instruction.
        let on_443 = unreachable_message("https", "aleph.example.com", 443);
        assert!(
            !on_443.contains("remove the port"),
            "nothing to remove on the default port, got: {on_443}"
        );
        assert!(
            on_443.contains("aleph.example.com:8443"),
            "should offer an explicit alternate port instead, got: {on_443}"
        );
        // The http case points at the reverse-proxy form, which is the one a
        // bare hostname most often needs.
        let http = unreachable_message("http", "aleph.example.com", 18790);
        assert!(
            http.contains("https://aleph.example.com"),
            "http failure should suggest the https form, got: {http}"
        );
    }

    #[test]
    fn the_message_names_the_port_the_shell_filled_in_not_the_one_typed() {
        // The regression this whole surface exists for: the user typed a bare
        // `https://` URL, the shell dialled a port they never wrote, and the
        // address field could not show it. The explanation must.
        let target = connection::ConnectionTarget::parse("https://gw.example.com").unwrap();
        assert_eq!(target_origin(&target), "https://gw.example.com:443");
        let msg = target_unreachable_message(&target);
        assert!(
            msg.contains("https://gw.example.com:443"),
            "must name the resolved origin, got: {msg}"
        );

        let bare = connection::ConnectionTarget::parse("gw.example.com").unwrap();
        assert_eq!(
            target_origin(&bare),
            "http://gw.example.com:18790",
            "a bare host still means the Aleph default — and the user never \
             typed that port either"
        );
    }

    #[test]
    fn a_local_target_reports_the_loopback_gateway() {
        assert_eq!(
            target_origin(&connection::ConnectionTarget::Local),
            "http://127.0.0.1:18790"
        );
    }

    /// Bind an ephemeral listener that accepts and immediately closes without
    /// writing a byte — the CDN-edge / port-forward-to-nothing shape.
    async fn accept_and_close() -> u16 {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });
        port
    }

    /// Bind an ephemeral listener that answers one request with `status_line`
    /// and returns its port.
    async fn serve_one_status(status_line: &'static str) -> u16 {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                // Drain the request before replying. Closing on an unread
                // receive buffer makes Windows send an RST, which would fail
                // the client's write and make this stub look like a dead peer.
                let mut scratch = [0u8; 1024];
                let _ = stream.read(&mut scratch).await;
                let _ = stream
                    .write_all(format!("{status_line}\r\nContent-Length: 0\r\n\r\n").as_bytes())
                    .await;
                let _ = stream.flush().await;
            }
        });
        port
    }
}
