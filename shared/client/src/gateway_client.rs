//! Stateless one-shot Gateway RPC client.
//!
//! Unlike [`AlephClient`](crate::AlephClient) which maintains a persistent
//! WebSocket connection with event streaming, `GatewayClient` opens a fresh
//! connection for each RPC call and tears it down immediately after.
//! This is ideal for CLI commands that fire a single request and exit.

use crate::error::CliError;
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async_tls_with_config, tungstenite::Message};

/// Default Gateway URL
pub const DEFAULT_GATEWAY_URL: &str = "ws://127.0.0.1:18790/ws";

/// Default timeout in milliseconds
pub const DEFAULT_TIMEOUT_MS: u64 = 30000;

/// Stateless one-shot Gateway RPC client.
///
/// Each call opens a new WebSocket, sends one JSON-RPC request,
/// reads one response, then closes the socket.
pub struct GatewayClient {
    url: String,
    timeout_ms: u64,
    /// See [`Self::with_ca_cert`]. Setter is exercised by no current caller
    /// (the only `GatewayClient` user is `aleph-server gateway call`, which
    /// talks loopback and relies on the connector's automatic self-signed
    /// lookup), but the field is kept so a non-loopback caller is a one-line
    /// change rather than a rediscovery of the same trust problem.
    #[allow(dead_code)]
    ca_cert: Option<String>,
}

impl GatewayClient {
    /// Create a new client with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            url: DEFAULT_GATEWAY_URL.to_string(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            ca_cert: None,
        }
    }

    /// Set the Gateway URL.
    #[must_use]
    pub fn with_url(mut self, url: &str) -> Self {
        self.url = url.to_string();
        self
    }

    /// Pin a PEM certificate for `wss://`, as `--ca-cert` does for the
    /// persistent client.
    ///
    /// Left `None` by every caller today, and that is deliberate rather than an
    /// oversight: this client is reached through `aleph-server gateway call`,
    /// which by construction talks to the server on the same machine, and
    /// [`crate::tls::connector_for`] already finds that server's own
    /// certificate for a loopback URL with no configuration at all. The setter
    /// exists so a future non-loopback caller is a one-line change instead of a
    /// rediscovery of this whole problem.
    #[must_use]
    pub fn with_ca_cert(mut self, ca_cert: Option<String>) -> Self {
        self.ca_cert = ca_cert;
        self
    }

    /// Set the timeout in milliseconds.
    #[must_use]
    pub const fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Call an RPC method, deserializing the result into `T`.
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<T, CliError> {
        let result = self.call_raw(method, params).await?;
        serde_json::from_value(result)
            .map_err(|e| CliError::Other(format!("Invalid response: {e}")))
    }

    /// Call an RPC method and return the raw JSON value.
    pub async fn call_raw(&self, method: &str, params: Option<Value>) -> Result<Value, CliError> {
        // Connect to Gateway
        let connector = crate::tls::connector_for(&self.url, self.ca_cert.as_deref())?;
        let (ws_stream, _) = timeout(
            Duration::from_secs(5),
            connect_async_tls_with_config(&self.url, None, false, connector),
        )
        .await
        .map_err(|e| CliError::Timeout(format!("Connection timeout: {e}")))?
        .map_err(|e| CliError::Connection(e.to_string()))?;

        let (mut write, mut read) = ws_stream.split();

        // Session-init handshake. The gateway enforces "the first frame on a
        // connection must be `connect`" and answers AUTH_REQUIRED + closes the
        // socket otherwise — so firing the method straight down a fresh socket
        // made EVERY `aleph-server gateway call` fail with
        // `First request must be 'connect'`. `connect` is also the frame that
        // establishes the operator role, so it has to precede the method anyway.
        //
        // Distinct id (0) from the method's (1): the response loop below matches
        // on the method's id, so the handshake reply is skipped naturally.
        const CONNECT_ID: i64 = 0;
        let request_id = 1;

        let connect = json!({
            "jsonrpc": "2.0",
            "method": "connect",
            "params": { "device_name": "aleph-cli", "channel_kind": "cli" },
            "id": CONNECT_ID
        });
        write
            .send(Message::Text(connect.to_string().into()))
            .await
            .map_err(|e| CliError::Connection(e.to_string()))?;

        // Build JSON-RPC request
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or_else(|| json!({})),
            "id": request_id
        });

        // Send request
        write
            .send(Message::Text(request.to_string().into()))
            .await
            .map_err(|e| CliError::Connection(e.to_string()))?;

        // Wait for the response whose `id` matches our request, ignoring
        // notifications and out-of-order frames that may arrive first.
        //
        // The handshake (id=0) is a special case: a SUCCESSFUL handshake carries
        // no payload for the caller to read (the role is already granted
        // server-side), but a FAILED handshake — `AUTH_REQUIRED`, rate limit,
        // origin gate — comes back with id=0 *and* an `error` frame. Skipping
        // it on the "id != 1" filter used to leave the caller parked for the
        // method's full timeout, with the real reason only visible in the
        // server log. Surface it now instead.
        let json = tokio::time::timeout(Duration::from_millis(self.timeout_ms), async {
            loop {
                let response = read
                    .next()
                    .await
                    .ok_or_else(|| CliError::Disconnected("Server closed connection".to_string()))?
                    .map_err(|e| CliError::Connection(e.to_string()))?;

                let text = response
                    .to_text()
                    .map_err(|e| CliError::Other(format!("Invalid response: {e}")))?;

                let json: Value = serde_json::from_str(text)?;
                let id = json.get("id").and_then(serde_json::Value::as_i64);
                match id {
                    // Handshake error → bubble it up immediately. The matching
                    // is on the wire `id` only; the error shape is read below.
                    Some(0) if json.get("error").is_some() => {
                        return Ok::<Value, CliError>(json);
                    }
                    // Method response we asked for.
                    Some(n) if n == request_id => {
                        return Ok::<Value, CliError>(json);
                    }
                    // Notifications, a successful handshake, or anything we
                    // didn't ask for: keep reading.
                    _ => continue,
                }
            }
        })
        .await
        .map_err(|e| CliError::Timeout(format!("Read timeout: {e}")))??;

        // Check for RPC error
        if let Some(error) = json.get("error") {
            let code = error
                .get("code")
                .and_then(serde_json::Value::as_i64)
                .and_then(|c| c.try_into().ok())
                .unwrap_or(-1);
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error")
                .to_string();
            return Err(CliError::Rpc { code, message });
        }

        // Extract result
        json.get("result")
            .cloned()
            .or_else(|| json.get("payload").cloned())
            .ok_or_else(|| CliError::Other("No result in response".to_string()))
    }
}

impl Default for GatewayClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tokio::net::TcpListener;

    #[test]
    fn test_client_builder() {
        let client = GatewayClient::new()
            .with_url("ws://localhost:9999")
            .with_timeout(5000);

        assert_eq!(client.url, "ws://localhost:9999");
        assert_eq!(client.timeout_ms, 5000);
    }

    #[test]
    fn test_default_values() {
        let client = GatewayClient::new();
        assert_eq!(client.url, DEFAULT_GATEWAY_URL);
        assert_eq!(client.timeout_ms, DEFAULT_TIMEOUT_MS);
    }

    /// The gateway answers `AUTH_REQUIRED` for the `connect` handshake when a
    /// caller without a credential hits an operator-only method. The previous
    /// read loop matched on `id == request_id` (1) only, so the id=0 error
    /// frame was discarded and the caller waited for the method's full
    /// timeout — a thirty-second "is this thing hung?" with the real reason
    /// buried in the server log. The fix surfaces the id=0 error as soon as
    /// it arrives.
    #[tokio::test]
    async fn a_handshake_error_is_surfaced_immediately_not_at_the_method_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            // Read the connect frame so the client's write completes.
            let _ = ws.next().await.unwrap().unwrap();
            let reply = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 0,
                "error": {
                    "code": -32000, // AUTH_REQUIRED
                    "message": "auth required"
                }
            });
            ws.send(Message::Text(reply.to_string().into()))
                .await
                .unwrap();
            // Hold the socket open past the client's read so a Disconnected
            // cannot masquerade as the fix.
            tokio::time::sleep(Duration::from_secs(3)).await;
        });

        let client = GatewayClient::new()
            .with_url(&format!("ws://{addr}"))
            .with_timeout(30_000);

        let started = Instant::now();
        let err = client
            .call::<Value>("some.method", None)
            .await
            .expect_err("a refused handshake must surface, not time out");
        assert!(
            matches!(err, CliError::Rpc { code: -32000, .. }),
            "got {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the caller waited {:?} — the id=0 error frame was discarded",
            started.elapsed()
        );
    }
}
