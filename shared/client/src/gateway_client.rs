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
use tokio_tungstenite::{connect_async, tungstenite::Message};

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
}

impl GatewayClient {
    /// Create a new client with default settings.
    pub fn new() -> Self {
        Self {
            url: DEFAULT_GATEWAY_URL.to_string(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }

    /// Set the Gateway URL.
    pub fn with_url(mut self, url: &str) -> Self {
        self.url = url.to_string();
        self
    }

    /// Set the timeout in milliseconds.
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
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
        serde_json::from_value(result).map_err(|e| {
            CliError::Other(format!("Invalid response: {}", e))
        })
    }

    /// Call an RPC method and return the raw JSON value.
    pub async fn call_raw(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, CliError> {
        // Connect to Gateway
        let (ws_stream, _) = timeout(
            Duration::from_millis(5000),
            connect_async(&self.url),
        )
        .await
        .map_err(|_| CliError::Timeout)?
        .map_err(|e| CliError::Connection(e.to_string()))?;

        let (mut write, mut read) = ws_stream.split();

        // Build JSON-RPC request
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(json!({})),
            "id": 1
        });

        // Send request
        write
            .send(Message::Text(request.to_string()))
            .await
            .map_err(|e| CliError::Connection(e.to_string()))?;

        // Wait for response with timeout
        let response = timeout(Duration::from_millis(self.timeout_ms), read.next())
            .await
            .map_err(|_| CliError::Timeout)?
            .ok_or(CliError::Disconnected)?
            .map_err(|e| CliError::Connection(e.to_string()))?;

        // Parse response
        let text = response
            .to_text()
            .map_err(|e| CliError::Other(format!("Invalid response: {}", e)))?;

        let json: Value = serde_json::from_str(text)?;

        // Check for RPC error
        if let Some(error) = json.get("error") {
            let code = error
                .get("code")
                .and_then(|c| c.as_i64())
                .unwrap_or(-1) as i32;
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
