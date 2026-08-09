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
                if json.get("id").and_then(serde_json::Value::as_i64) == Some(request_id) {
                    return Ok::<Value, CliError>(json);
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
}
