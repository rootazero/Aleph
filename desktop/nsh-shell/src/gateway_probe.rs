//! Is an Aleph Gateway answering at a target, and, when it is not, the
//! sentence that says so. Ported from the Tauri shell's probe, with one
//! change: a remote HTTPS server must present a certificate this system
//! trusts. The probe does not accept a self-signed certificate, and a
//! server that fails that check gets its own message, so the user is not
//! told it is down.
//!
//! The probe asks `GET /ready` rather than connecting over TCP alone: a CDN
//! edge, a load balancer or a port-forward to nothing completes the TCP
//! handshake and then closes, which a connect-only probe reads as healthy.

use aleph_client::ConnectionTarget;
use std::time::Duration;

/// Upper bound on one probe: one TLS handshake plus one HTTP round trip.
/// A real remote Gateway behind a CDN measured 0.6–1.9 s, so a tighter
/// budget reports a healthy server as down on a cold connection.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The Gateway's unauthenticated readiness endpoint: 200 once ready, 503
/// while it is still booting.
const READY_PATH: &str = "/ready";

/// What one probe found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Probe {
    /// An Aleph Gateway answered `/ready` with 200 or 503.
    Ready,
    /// Nothing answered, or what answered is not an Aleph Gateway.
    Down,
    /// The TCP connection opened but no trusted TLS session could be
    /// established: an untrusted or mismatched certificate, or a server that
    /// does not speak HTTPS on this port.
    Untrusted,
}

/// Probes `target` within [`PROBE_TIMEOUT`].
pub(crate) async fn probe_target(target: &ConnectionTarget) -> Probe {
    probe_target_within(target, PROBE_TIMEOUT).await
}

/// [`probe_target`] with an explicit budget, for the tests only: no caller
/// may shorten the budget below the measured CDN handshake range.
async fn probe_target_within(target: &ConnectionTarget, timeout: Duration) -> Probe {
    let (scheme, host, port) = probe_endpoint(target);
    probe(&scheme, &host, port, timeout).await
}

async fn probe(scheme: &str, host: &str, port: u16, timeout: Duration) -> Probe {
    let origin = origin(scheme, host, port);
    match tokio::time::timeout(timeout, ready_status(scheme, host, port)).await {
        Ok(Answer::Status(code @ (200 | 503))) => {
            eprintln!("gateway probe {origin}{READY_PATH} -> {code}");
            Probe::Ready
        }
        // Reachable, but not an Aleph Gateway (a CDN 404, a stray web
        // server, a proxy error page).
        Ok(Answer::Status(code)) => {
            eprintln!("{origin} answered {code} at {READY_PATH}: not an Aleph Gateway");
            Probe::Down
        }
        Ok(Answer::NoHttp) => {
            eprintln!("{origin} accepted the connection but served no HTTP");
            Probe::Down
        }
        Ok(Answer::NoConnection) => {
            eprintln!("{origin} refused the connection");
            Probe::Down
        }
        Ok(Answer::TlsRefused) => {
            eprintln!("{origin}: no trusted TLS session");
            Probe::Untrusted
        }
        Err(_) => {
            eprintln!("{origin} did not answer within {timeout:?}");
            Probe::Down
        }
    }
}

/// How far one `GET /ready` got.
enum Answer {
    NoConnection,
    TlsRefused,
    NoHttp,
    Status(u16),
}

/// One HTTP/1.0 `GET /ready`, over TLS with the system's trust store when
/// `scheme` is `https`. Unbounded: call it through the timeout in [`probe`].
async fn ready_status(scheme: &str, host: &str, port: u16) -> Answer {
    let Ok(stream) = tokio::net::TcpStream::connect((host, port)).await else {
        return Answer::NoConnection;
    };
    let request =
        format!("GET {READY_PATH} HTTP/1.0\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    if scheme != "https" {
        let mut stream = stream;
        return http_status(&mut stream, &request).await;
    }
    let Ok(connector) = native_tls::TlsConnector::new() else {
        return Answer::TlsRefused;
    };
    // `connect` carries `host` as SNI, which a CDN or a name-based virtual
    // host needs to route the request at all.
    match tokio_native_tls::TlsConnector::from(connector)
        .connect(host, stream)
        .await
    {
        Ok(mut tls) => http_status(&mut tls, &request).await,
        Err(_) => Answer::TlsRefused,
    }
}

/// Writes `request` and parses the numeric status from the reply's first
/// line (`HTTP/1.1 200 OK` -> 200). The status line arrives in the first
/// segment, so one bounded read is enough.
async fn http_status<S>(io: &mut S, request: &str) -> Answer
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    if io.write_all(request.as_bytes()).await.is_err() {
        return Answer::NoHttp;
    }
    let mut chunk = [0u8; 512];
    let Ok(read) = io.read(&mut chunk).await else {
        return Answer::NoHttp;
    };
    let head = String::from_utf8_lossy(&chunk[..read]);
    // A raw byte stream (SSH, a TLS record on a plaintext port) is not an
    // HTTP status line.
    let mut parts = head.split_whitespace();
    if !parts
        .next()
        .is_some_and(|version| version.starts_with("HTTP/"))
    {
        return Answer::NoHttp;
    }
    match parts.next().and_then(|code| code.parse().ok()) {
        Some(code) => Answer::Status(code),
        None => Answer::NoHttp,
    }
}

/// The `(scheme, host, port)` a target resolves to. The scheme is part of
/// the endpoint: probing an `https://` target over plaintext gets no HTTP
/// reply from a healthy server.
fn probe_endpoint(target: &ConnectionTarget) -> (String, String, u16) {
    match target {
        ConnectionTarget::Local => ("http".into(), "127.0.0.1".into(), 18790),
        ConnectionTarget::Remote(url) => (
            url.scheme().into(),
            url.host_str().unwrap_or("127.0.0.1").into(),
            url.port_or_known_default().unwrap_or(18790),
        ),
    }
}

/// The origin every message names: the one that was dialled, port included.
fn origin(scheme: &str, host: &str, port: u16) -> String {
    format!("{scheme}://{host}:{port}")
}

/// Why `target` could not be used, phrased for the connect form; `None`
/// when the probe found a Gateway. It names the exact origin probed: the
/// port is often one the shell filled in, invisible in the address field.
///
/// The hint rule is mirrored on iOS (`PairingTarget.swift`), because the
/// two shells promise one onboarding format. Change one, change both.
pub(crate) fn probe_message(target: &ConnectionTarget, probe: Probe) -> Option<String> {
    let (scheme, host, port) = probe_endpoint(target);
    let origin = origin(&scheme, &host, port);
    match probe {
        Probe::Ready => None,
        Probe::Untrusted => Some(format!(
            "无法与 {origin} 建立受信任的 HTTPS 连接。服务器需要本机系统信任的证书\
             （trusted certificate），不接受自签名证书；也请确认该端口提供的是 HTTPS。"
        )),
        Probe::Down => Some(format!(
            "{origin} 上没有 Aleph 服务器应答。请检查地址，并确认服务器正在运行。{}",
            unreachable_hint(&scheme, &host, port)
        )),
    }
}

fn unreachable_hint(scheme: &str, host: &str, port: u16) -> String {
    if scheme == "https" && port != 443 {
        // A reverse proxy or CDN fronts 443 and does not forward another
        // port: the fix is to drop the port, not to try a different one.
        format!("如果它在反向代理或 CDN 之后，请去掉端口，使用默认的 443（https://{host}）。")
    } else if scheme == "https" {
        format!("如果服务器监听其他端口，请写明端口（例如 {host}:8443）。")
    } else {
        format!(
            "如果它在 HTTPS 或反向代理之后，请输入完整地址（例如 https://{host}）；\
             要用其他端口，请写明端口（例如 {host}:18790）。"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> ConnectionTarget {
        ConnectionTarget::parse(raw).unwrap()
    }

    #[test]
    fn local_probes_the_loopback_gateway() {
        assert_eq!(
            probe_endpoint(&ConnectionTarget::Local),
            ("http".into(), "127.0.0.1".into(), 18790)
        );
    }

    #[test]
    fn remote_uses_the_url_scheme_host_and_port() {
        assert_eq!(
            probe_endpoint(&parse("box.lan:9000")),
            ("http".into(), "box.lan".into(), 9000)
        );
        assert_eq!(
            probe_endpoint(&parse("gw.example.com")),
            ("http".into(), "gw.example.com".into(), 18790)
        );
        assert_eq!(
            probe_endpoint(&parse("https://gw.example.com")),
            ("https".into(), "gw.example.com".into(), 443)
        );
    }

    #[tokio::test]
    async fn a_closed_port_is_down() {
        let probe = probe("http", "127.0.0.1", 1, Duration::from_millis(200)).await;
        assert_eq!(probe, Probe::Down);
    }

    #[tokio::test]
    async fn a_socket_that_serves_no_http_is_down() {
        let port = accept_and_close().await;
        let target = parse(&format!("http://127.0.0.1:{port}"));
        assert_eq!(
            probe_target_within(&target, Duration::from_millis(500)).await,
            Probe::Down
        );
    }

    #[tokio::test]
    async fn a_non_gateway_http_responder_is_down() {
        let port = serve_one_status("HTTP/1.1 404 Not Found").await;
        let probe = probe("http", "127.0.0.1", port, Duration::from_millis(500)).await;
        assert_eq!(probe, Probe::Down);
    }

    #[tokio::test]
    async fn a_gateway_answering_ready_is_ready() {
        for line in ["HTTP/1.1 200 OK", "HTTP/1.1 503 Service Unavailable"] {
            let port = serve_one_status(line).await;
            let probe = probe("http", "127.0.0.1", port, Duration::from_millis(500)).await;
            assert_eq!(probe, Probe::Ready, "{line}");
        }
    }

    #[tokio::test]
    async fn a_failed_tls_handshake_is_untrusted_not_down() {
        // A plaintext server on an https target fails the handshake the
        // same way an untrusted certificate does.
        let port = serve_one_status("HTTP/1.1 200 OK").await;
        let probe = probe("https", "127.0.0.1", port, Duration::from_secs(2)).await;
        assert_eq!(probe, Probe::Untrusted);
    }

    #[test]
    fn an_untrusted_server_is_told_it_needs_a_trusted_certificate() {
        let message = probe_message(&parse("https://gw.example.com"), Probe::Untrusted).unwrap();
        assert!(message.contains("https://gw.example.com:443"), "{message}");
        assert!(
            message.contains("受信任") && message.contains("trusted certificate"),
            "{message}"
        );
    }

    #[test]
    fn a_ready_gateway_has_no_message() {
        assert_eq!(
            probe_message(&parse("https://gw.example.com"), Probe::Ready),
            None
        );
    }

    #[test]
    fn the_down_message_names_the_probed_origin_and_hint() {
        let behind_proxy =
            probe_message(&parse("https://aleph.example.com:18790"), Probe::Down).unwrap();
        assert!(behind_proxy.contains("https://aleph.example.com:18790"));
        assert!(
            behind_proxy.contains("去掉端口")
                && behind_proxy.contains("https://aleph.example.com）")
        );
        let default_port = probe_message(&parse("https://aleph.example.com"), Probe::Down).unwrap();
        assert!(default_port.contains("https://aleph.example.com:443"));
        assert!(
            !default_port.contains("去掉端口") && default_port.contains("aleph.example.com:8443")
        );
        let plain = probe_message(&parse("aleph.example.com"), Probe::Down).unwrap();
        assert!(plain.contains("http://aleph.example.com:18790"));
        assert!(plain.contains("https://aleph.example.com"));
    }

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

    async fn serve_one_status(status_line: &'static str) -> u16 {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
