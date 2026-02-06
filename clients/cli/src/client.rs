//! Aleph Gateway client - SDK wrapper
//!
//! This module provides a thin wrapper around aleph-client-sdk,
//! adapting it for CLI-specific needs.

use aleph_client_sdk::{GatewayClient, StreamEvent};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::info;

use crate::config::CliConfig;
use crate::error::{CliError, CliResult};

/// Aleph client (wrapper around SDK)
pub struct AlephClient {
    inner: GatewayClient,
}

impl AlephClient {
    /// Connect to Aleph Gateway
    ///
    /// Returns the client and an event receiver for stream events.
    pub async fn connect(url: &str) -> CliResult<(Self, mpsc::Receiver<StreamEvent>)> {
        info!("Connecting to {}", url);

        let client = GatewayClient::new(url);
        let events = client.connect().await.map_err(sdk_error_to_cli)?;

        info!("Connected to Gateway");

        Ok((Self { inner: client }, events))
    }

    /// Authenticate with the server
    ///
    /// Sends client manifest and device info, returns auth token.
    pub async fn authenticate(&self, config: &CliConfig) -> CliResult<String> {
        info!("Authenticating as '{}'", config.device_name);

        let token = self
            .inner
            .authenticate(
                config,
                "cli",
                config.manifest.tool_categories.clone(),
                Some(config.manifest.specific_tools.clone()),
            )
            .await
            .map_err(sdk_error_to_cli)?;

        info!("Authentication successful");
        Ok(token)
    }

    /// Send RPC call and wait for response
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> CliResult<R> {
        self.inner
            .call(method, params)
            .await
            .map_err(sdk_error_to_cli)
    }

    /// Send notification (no response expected)
    pub async fn notify<P: Serialize>(&self, method: &str, params: Option<P>) -> CliResult<()> {
        self.inner
            .notify(method, params)
            .await
            .map_err(sdk_error_to_cli)
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    /// Get current auth token
    pub async fn auth_token(&self) -> Option<String> {
        self.inner.auth_token().await
    }

    /// Close the connection
    pub async fn close(&self) -> CliResult<()> {
        self.inner.close().await.map_err(sdk_error_to_cli)
    }

    /// Send RPC call with generic Value params (convenience method)
    pub async fn call_value(&self, method: &str, params: Option<Value>) -> CliResult<Value> {
        self.call(method, params).await
    }
}

/// Convert SDK error to CLI error
fn sdk_error_to_cli(err: aleph_client_sdk::ClientError) -> CliError {
    use aleph_client_sdk::ClientError;

    match err {
        ClientError::ConnectionFailed(msg) | ClientError::WebSocketError(msg) => {
            CliError::Connection(msg)
        }
        ClientError::AuthenticationFailed(msg) => CliError::AuthFailed(msg),
        ClientError::RpcError(msg) => {
            // Try to parse as structured RPC error
            if let Some((code, message)) = parse_rpc_error(&msg) {
                CliError::Rpc { code, message }
            } else {
                CliError::Other(msg)
            }
        }
        ClientError::Timeout => CliError::Timeout,
        ClientError::ConnectionClosed => CliError::Disconnected,
        ClientError::ConfigError(msg) => CliError::Config(msg),
        ClientError::SerializationError(msg) => {
            CliError::Json(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                msg,
            )))
        }
        ClientError::PairingTimeout => CliError::Timeout,
    }
}

/// Parse RPC error message to extract code and message
///
/// Format: "code: message"
fn parse_rpc_error(msg: &str) -> Option<(i32, String)> {
    let parts: Vec<&str> = msg.splitn(2, ':').collect();
    if parts.len() == 2 {
        if let Ok(code) = parts[0].trim().parse::<i32>() {
            return Some((code, parts[1].trim().to_string()));
        }
    }
    None
}
