//! Minimal HTTP CONNECT proxy (RFC 7231 §4.3.6).
//!
//! Accepts a single CONNECT request, validates the target hostname against
//! an [`AllowList`], dials the upstream, and bridges the byte stream both
//! ways. No request bodies, no GET/POST forwarding — CONNECT is the only
//! method supported (the proxy is meant for HTTPS tunnels and TLS-wrapped
//! traffic, with `http://` traffic also routed via CONNECT-by-loopback by
//! most clients when `HTTPS_PROXY` is set).
//!
//! Wire format (request):
//! ```text
//! CONNECT host:port HTTP/1.1\r\n
//! Host: host:port\r\n
//! [...other headers...]\r\n
//! \r\n
//! ```
//!
//! On success the proxy responds `HTTP/1.1 200 Connection Established\r\n\r\n`.
//! Disallowed hosts get `HTTP/1.1 403 Forbidden`. Upstream errors get 502.
//! Malformed requests get 400.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use super::allowlist::AllowList;

const MAX_HEADER_BYTES: usize = 8 * 1024;
const UPSTREAM_DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Parsed CONNECT request line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConnectRequest {
    pub host: String,
    pub port: u16,
}

/// Parse one CONNECT request from an already-buffered first line. Returns
/// `Ok` if and only if the line is `CONNECT <host>:<port> HTTP/1.x`.
pub(super) fn parse_request_line(line: &str) -> Result<ConnectRequest, ProxyHttpError> {
    let line = line.trim_end_matches(['\r', '\n']);
    let mut parts = line.split(' ');
    let method = parts.next().ok_or(ProxyHttpError::BadRequest)?;
    let target = parts.next().ok_or(ProxyHttpError::BadRequest)?;
    let version = parts.next().ok_or(ProxyHttpError::BadRequest)?;
    if !method.eq_ignore_ascii_case("CONNECT") {
        return Err(ProxyHttpError::MethodNotConnect);
    }
    if !version.starts_with("HTTP/1.") {
        return Err(ProxyHttpError::BadRequest);
    }
    // host:port — IPv6 literals must be bracketed: `[::1]:443`.
    let (host, port) = if let Some(rest) = target.strip_prefix('[') {
        let (h, tail) = rest.split_once(']').ok_or(ProxyHttpError::BadRequest)?;
        let port_str = tail.strip_prefix(':').ok_or(ProxyHttpError::BadRequest)?;
        let port: u16 = port_str.parse().map_err(|_| ProxyHttpError::BadRequest)?;
        (h.to_string(), port)
    } else {
        let (h, p) = target.rsplit_once(':').ok_or(ProxyHttpError::BadRequest)?;
        let port: u16 = p.parse().map_err(|_| ProxyHttpError::BadRequest)?;
        (h.to_string(), port)
    };
    if host.is_empty() {
        return Err(ProxyHttpError::BadRequest);
    }
    Ok(ConnectRequest { host, port })
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(super) enum ProxyHttpError {
    #[error("malformed CONNECT request")]
    BadRequest,
    #[error("only the CONNECT method is supported")]
    MethodNotConnect,
}

/// Handle a single inbound HTTP CONNECT connection.
///
/// Reads up to [`MAX_HEADER_BYTES`] looking for the `\r\n\r\n` terminator,
/// parses the request line, enforces the allowlist, dials the upstream, and
/// splices the streams. Returns when either side closes.
pub(super) async fn handle(inbound: TcpStream, allowlist: AllowList) -> Result<(), std::io::Error> {
    let (rd, mut wr) = inbound.into_split();
    let mut rd = BufReader::with_capacity(MAX_HEADER_BYTES, rd);

    // First line.
    let mut request_line = String::new();
    let n = rd.read_line(&mut request_line).await?;
    if n == 0 {
        return Ok(());
    }
    let req = match parse_request_line(&request_line) {
        Ok(r) => r,
        Err(_) => {
            let _ = wr.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
            return Ok(());
        }
    };

    // Drain remaining headers until the CRLFCRLF terminator (a bare CRLF or
    // LF line). We bound the total header size to defend against an
    // unterminated client.
    let mut total = request_line.len();
    let mut header = String::new();
    loop {
        header.clear();
        let read = rd.read_line(&mut header).await?;
        if read == 0 {
            return Ok(());
        }
        total += read;
        if total > MAX_HEADER_BYTES {
            let _ = wr
                .write_all(b"HTTP/1.1 431 Request Header Fields Too Large\r\n\r\n")
                .await;
            return Ok(());
        }
        if header.trim_end_matches(['\r', '\n']).is_empty() {
            break;
        }
    }

    // Allowlist check.
    if !allowlist.permits(&req.host) {
        tracing::debug!(
            target = "sandbox.proxy",
            host = %req.host,
            port = req.port,
            "CONNECT denied by allowlist"
        );
        let _ = wr
            .write_all(b"HTTP/1.1 403 Forbidden\r\nProxy-Agent: aleph-sandbox\r\n\r\n")
            .await;
        return Ok(());
    }

    // Dial upstream with timeout. `dial_validated` resolves the host once and
    // pins the connection to a non-SSRF-blocked resolved IP — closing the
    // DNS-rebinding gap where an allowlisted *name* re-resolves to a loopback /
    // metadata / internal address at connect time.
    let upstream = match tokio::time::timeout(
        UPSTREAM_DIAL_TIMEOUT,
        super::dial::dial_validated(&req.host, req.port, &allowlist),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            tracing::warn!(
                target = "sandbox.proxy",
                host = %req.host,
                port = req.port,
                "CONNECT blocked: host resolved to an SSRF-blocked address (possible DNS rebinding)"
            );
            let _ = wr
                .write_all(b"HTTP/1.1 403 Forbidden\r\nProxy-Agent: aleph-sandbox\r\n\r\n")
                .await;
            return Ok(());
        }
        Ok(Err(e)) => {
            tracing::debug!(
                target = "sandbox.proxy",
                host = %req.host,
                port = req.port,
                error = %e,
                "CONNECT upstream dial failed"
            );
            let _ = wr.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
            return Ok(());
        }
        Err(_) => {
            let _ = wr.write_all(b"HTTP/1.1 504 Gateway Timeout\r\n\r\n").await;
            return Ok(());
        }
    };

    wr.write_all(b"HTTP/1.1 200 Connection Established\r\nProxy-Agent: aleph-sandbox\r\n\r\n")
        .await?;

    // Re-join the inbound halves into a single stream for symmetric splice.
    let mut inbound = rd.into_inner().reunite(wr).expect("same socket halves");
    let mut upstream = upstream;
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut upstream).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_request() {
        let req = parse_request_line("CONNECT api.example.com:443 HTTP/1.1\r\n").unwrap();
        assert_eq!(req.host, "api.example.com");
        assert_eq!(req.port, 443);
    }

    #[test]
    fn parses_ipv6_bracketed_target() {
        let req = parse_request_line("CONNECT [::1]:443 HTTP/1.1\r\n").unwrap();
        assert_eq!(req.host, "::1");
        assert_eq!(req.port, 443);
    }

    #[test]
    fn rejects_non_connect_method() {
        let err = parse_request_line("GET / HTTP/1.1\r\n").unwrap_err();
        assert_eq!(err, ProxyHttpError::MethodNotConnect);
    }

    #[test]
    fn rejects_missing_port() {
        assert_eq!(
            parse_request_line("CONNECT api.example.com HTTP/1.1\r\n").unwrap_err(),
            ProxyHttpError::BadRequest
        );
    }

    #[test]
    fn rejects_old_http_version() {
        assert_eq!(
            parse_request_line("CONNECT host:443 HTTP/0.9\r\n").unwrap_err(),
            ProxyHttpError::BadRequest
        );
    }

    #[test]
    fn rejects_non_numeric_port() {
        assert_eq!(
            parse_request_line("CONNECT host:abc HTTP/1.1\r\n").unwrap_err(),
            ProxyHttpError::BadRequest
        );
    }

    #[test]
    fn rejects_empty_host() {
        assert_eq!(
            parse_request_line("CONNECT :443 HTTP/1.1\r\n").unwrap_err(),
            ProxyHttpError::BadRequest
        );
    }
}
