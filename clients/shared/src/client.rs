//! Gateway client implementation
//!
//! Main client type that coordinates transport, RPC, and authentication.

use crate::{ClientError, Result, Transport, RpcClient};
use serde_json::Value;

#[cfg(feature = "tracing")]
use tracing::{debug, info};

/// Gateway client
///
/// High-level client for connecting to Aleph Gateway.
pub struct GatewayClient {
    url: String,
    transport: Transport,
    rpc: RpcClient,
}

impl GatewayClient {
    /// Create a new gateway client
    ///
    /// # Example
    ///
    /// ```no_run
    /// use aleph_client_sdk::GatewayClient;
    ///
    /// let client = GatewayClient::new("ws://127.0.0.1:18789");
    /// ```
    pub fn new(url: &str) -> Self {
        #[cfg(feature = "tracing")]
        info!("Creating GatewayClient for {}", url);

        Self {
            url: url.to_string(),
            transport: Transport::new(url.to_string()),
            rpc: RpcClient::new(),
        }
    }

    /// Get the gateway URL
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        // TODO: Check actual connection state
        false
    }

    /// Send RPC call
    pub async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
        #[cfg(feature = "tracing")]
        debug!("Calling RPC method: {}", method);

        // TODO: Implement actual RPC call
        Err(ClientError::RpcError("Not implemented".into()))
    }

    /// Close the connection
    pub async fn close(&self) -> Result<()> {
        #[cfg(feature = "tracing")]
        info!("Closing connection");

        // TODO: Implement connection close
        Ok(())
    }
}
