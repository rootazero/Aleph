//! Lifecycle for the managed proxy: bind, accept loop, graceful shutdown.
//!
//! A single TCP listener on 127.0.0.1:0 (kernel-chosen port) handles both
//! HTTP CONNECT and SOCKS5. Protocol is detected by peeking the first byte:
//! `0x05` → SOCKS5; anything else → HTTP CONNECT (request lines start with
//! `C` = 0x43, but the actual byte is irrelevant — the dispatcher just
//! routes everything that isn't SOCKS5 to the HTTP handler, which then
//! validates the request line).
//!
//! Lifecycle is bound to a [`ProxyHandle`]. Drop = shutdown: the accept
//! loop returns when the watch channel is signalled. Existing in-flight
//! connections close when their `copy_bidirectional` terminates (either
//! peer EOF or local socket dropped).

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::watch;

use super::allowlist::AllowList;
use super::{connect, socks5};

/// Handle to a running proxy. Dropping shuts it down.
pub struct ProxyHandle {
    addr: SocketAddr,
    shutdown_tx: Arc<watch::Sender<bool>>,
}

impl ProxyHandle {
    /// Local address the proxy is bound to (127.0.0.1:port).
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Convenience for env var values: `http://127.0.0.1:<port>`.
    #[must_use]
    pub fn http_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Trigger shutdown explicitly. Drop also triggers it; this is for
    /// deterministic teardown in tests.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for ProxyHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyHandle")
            .field("addr", &self.addr)
            .finish()
    }
}

/// Errors when starting the proxy.
#[derive(Debug, thiserror::Error)]
pub enum ProxyStartError {
    #[error("bind 127.0.0.1:0: {0}")]
    Bind(#[source] std::io::Error),
    #[error("local_addr: {0}")]
    LocalAddr(#[source] std::io::Error),
}

/// Spawn a managed proxy bound to 127.0.0.1:0 with the given allowlist.
///
/// Returns immediately with a [`ProxyHandle`] holding the listener's local
/// address. The accept loop runs on the current Tokio runtime as a detached
/// task and exits when the handle is dropped.
pub async fn spawn(allowlist: AllowList) -> Result<ProxyHandle, ProxyStartError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(ProxyStartError::Bind)?;
    let addr = listener.local_addr().map_err(ProxyStartError::LocalAddr)?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(accept_loop(listener, allowlist, shutdown_rx));

    Ok(ProxyHandle {
        addr,
        shutdown_tx: Arc::new(shutdown_tx),
    })
}

async fn accept_loop(
    listener: TcpListener,
    allowlist: AllowList,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                tracing::debug!(target = "sandbox.proxy", "shutdown signalled, exiting accept loop");
                return;
            }
            accept = listener.accept() => {
                match accept {
                    Ok((stream, peer)) => {
                        let al = allowlist.clone();
                        tokio::spawn(async move {
                            if let Err(e) = dispatch(stream, al).await {
                                tracing::debug!(
                                    target = "sandbox.proxy",
                                    peer = %peer,
                                    error = %e,
                                    "proxy connection terminated with error"
                                );
                            }
                        });
                    }
                    Err(e) => {
                        // Transient accept errors (per-fd limits, etc.) should
                        // not kill the proxy; pause briefly then resume.
                        tracing::debug!(
                            target = "sandbox.proxy",
                            error = %e,
                            "accept failed; backing off"
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
            }
        }
    }
}

async fn dispatch(
    stream: tokio::net::TcpStream,
    allowlist: AllowList,
) -> Result<(), std::io::Error> {
    // Peek the first byte to decide protocol. We need a real read (not a
    // peek-only) because tokio doesn't expose MSG_PEEK on TcpStream; we
    // splice the byte back via a small in-process splitter when we hand
    // off to the protocol handler. To keep this simple, we let the
    // handler re-read by buffering through `BufReader` (HTTP) or by
    // consuming the byte (SOCKS5: the version byte is read again as part
    // of the greeting).
    let mut first = [0u8; 1];
    let n = stream.peek(&mut first).await?;
    if n == 0 {
        return Ok(()); // peer closed.
    }
    if first[0] == 0x05 {
        socks5::handle(stream, allowlist).await
    } else {
        connect::handle(stream, allowlist).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// Tokio test currently uses the default 1-thread runtime; that's
    /// fine because the proxy spawns its own task.

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn proxy_binds_and_returns_local_addr() {
        let handle = spawn(AllowList::new(["example.com"])).await.unwrap();
        let addr = handle.local_addr();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert!(addr.port() > 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_connect_denied_for_disallowed_host() {
        let handle = spawn(AllowList::new(["allowed.example"])).await.unwrap();
        let mut client = TcpStream::connect(handle.local_addr()).await.unwrap();
        client
            .write_all(b"CONNECT denied.example:443 HTTP/1.1\r\nHost: denied.example:443\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        // Read all until close (handler responds 403 then drops).
        let _ = tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut buf)).await;
        let response = String::from_utf8_lossy(&buf);
        assert!(
            response.starts_with("HTTP/1.1 403"),
            "expected 403, got: {response}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_connect_succeeds_for_allowed_host_via_loopback_target() {
        // Spin up a tiny loopback "upstream" that emits a marker then closes.
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = upstream.accept().await {
                let _ = s.write_all(b"HELLO_FROM_UPSTREAM").await;
                let _ = s.shutdown().await;
            }
        });

        let handle = spawn(AllowList::new(["127.0.0.1"])).await.unwrap();
        let mut client = TcpStream::connect(handle.local_addr()).await.unwrap();
        let target = format!("127.0.0.1:{}", upstream_addr.port());
        let req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
        client.write_all(req.as_bytes()).await.unwrap();

        // Read everything the proxy emits until EOF (upstream closes after
        // the marker, copy_bidirectional propagates the half-close).
        let mut all = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(3), client.read_to_end(&mut all)).await;
        let text = String::from_utf8_lossy(&all);
        assert!(
            text.starts_with("HTTP/1.1 200"),
            "expected 200 response, got: {text}"
        );
        let split = text
            .find("\r\n\r\n")
            .expect("response should contain CRLFCRLF terminator");
        let payload = &text[split + 4..];
        assert!(
            payload.contains("HELLO_FROM_UPSTREAM"),
            "expected upstream payload after headers, got: {payload}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn socks5_no_auth_negotiation() {
        let handle = spawn(AllowList::new(["disallowed"])).await.unwrap();
        let mut client = TcpStream::connect(handle.local_addr()).await.unwrap();
        // VER=5, NMETHODS=1, METHODS=[NO_AUTH]
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut greeting = [0u8; 2];
        client.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting, [0x05, 0x00]); // VER, METHOD=NO_AUTH
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn socks5_domain_request_denied() {
        let handle = spawn(AllowList::new(["allowed.example"])).await.unwrap();
        let mut client = TcpStream::connect(handle.local_addr()).await.unwrap();
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut greeting = [0u8; 2];
        client.read_exact(&mut greeting).await.unwrap();

        // Request: VER CMD RSV ATYP=DOMAIN  LEN  "denied.example"   PORT
        let host = b"denied.example";
        let mut req = vec![0x05, 0x01, 0x00, 0x03, host.len() as u8];
        req.extend_from_slice(host);
        req.extend_from_slice(&443u16.to_be_bytes());
        client.write_all(&req).await.unwrap();

        let mut reply = [0u8; 10];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[0], 0x05); // VER
        assert_eq!(reply[1], 0x02); // REP=NOT_ALLOWED
        assert_eq!(reply[3], 0x01); // ATYP=IPv4
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_terminates_accept_loop() {
        let handle = spawn(AllowList::new(["x"])).await.unwrap();
        let addr = handle.local_addr();
        handle.shutdown();
        // Give the loop a moment to exit.
        tokio::time::sleep(Duration::from_millis(80)).await;
        // A fresh connect either fails (socket closed) or hangs at handshake.
        // Either way, no greeting bytes should arrive within 200ms.
        let connect_result =
            tokio::time::timeout(Duration::from_millis(200), TcpStream::connect(addr)).await;
        if let Ok(Ok(mut s)) = connect_result {
            // OS may still accept on the listener queue briefly; bytes should
            // not flow because no task is reading. Send greeting and confirm
            // it does not get answered.
            let _ = s.write_all(&[0x05, 0x01, 0x00]).await;
            let mut buf = [0u8; 1];
            let read = tokio::time::timeout(Duration::from_millis(150), s.read(&mut buf)).await;
            assert!(read.is_err() || matches!(read, Ok(Ok(0))));
        }
        // Drop the handle (already dropped by shutdown trigger).
        drop(handle);
    }
}
