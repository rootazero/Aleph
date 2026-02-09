//! JSON-RPC 2.0 client implementation

use super::super::connection::{AlephConnector, ConnectionError};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{oneshot, RwLock};
use tokio::time::{timeout, Duration};

/// RPC client for type-safe JSON-RPC 2.0 communication
///
/// This client provides a high-level interface for making RPC calls
/// to the Aleph Gateway. It handles:
/// - Request ID generation
/// - Request/response matching
/// - Timeout management
/// - Type-safe serialization/deserialization
///
/// ## Example
///
/// ```rust,ignore
/// use aleph_ui_logic::connection::create_connector;
/// use aleph_ui_logic::protocol::RpcClient;
///
/// #[tokio::main]
/// async fn main() {
///     let mut connector = create_connector();
///     connector.connect("ws://127.0.0.1:18789").await.unwrap();
///
///     let client = RpcClient::new(connector);
///
///     // Type-safe RPC call
///     let result: MemoryStats = client
///         .call("memory.stats", ())
///         .await
///         .unwrap();
/// }
/// ```
pub struct RpcClient<C: AlephConnector> {
    connector: Arc<RwLock<C>>,
    pending_requests: Arc<RwLock<HashMap<String, oneshot::Sender<Result<Value, RpcError>>>>>,
    next_id: Arc<RwLock<u64>>,
    default_timeout: Duration,
}

impl<C: AlephConnector> RpcClient<C> {
    /// Create a new RPC client with the given connector
    ///
    /// # Arguments
    ///
    /// - `connector`: The WebSocket connector to use
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let connector = create_connector();
    /// let client = RpcClient::new(connector);
    /// ```
    pub fn new(connector: C) -> Self {
        Self {
            connector: Arc::new(RwLock::new(connector)),
            pending_requests: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(RwLock::new(0)),
            default_timeout: Duration::from_secs(30),
        }
    }

    /// Set the default timeout for RPC calls
    ///
    /// # Arguments
    ///
    /// - `timeout`: The timeout duration
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.default_timeout = timeout;
    }

    /// Make a type-safe RPC call
    ///
    /// # Type Parameters
    ///
    /// - `P`: The parameter type (must be serializable)
    /// - `R`: The result type (must be deserializable)
    ///
    /// # Arguments
    ///
    /// - `method`: The RPC method name (e.g., "memory.stats")
    /// - `params`: The method parameters
    ///
    /// # Returns
    ///
    /// The deserialized result of type `R`
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if:
    /// - Serialization fails
    /// - Connection fails
    /// - Request times out
    /// - Server returns an error
    /// - Deserialization fails
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let stats: MemoryStats = client
    ///     .call("memory.stats", ())
    ///     .await?;
    /// ```
    pub async fn call<P, R>(&self, method: &str, params: P) -> Result<R, RpcError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        // Generate request ID
        let id = self.generate_id().await;

        // Serialize parameters
        let params_value = serde_json::to_value(params)?;

        // Construct request
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params_value,
        });

        // Create response channel
        let (tx, rx) = oneshot::channel();
        self.pending_requests
            .write()
            .await
            .insert(id.clone(), tx);

        // Send request
        {
            let mut connector = self.connector.write().await;
            connector
                .send(request)
                .await
                .map_err(RpcError::Connection)?;
        }

        // Wait for response with timeout
        let response = timeout(self.default_timeout, rx)
            .await
            .map_err(|_| RpcError::Timeout)??;

        // Parse and return result
        let result = response?;
        Ok(serde_json::from_value(result)?)
    }

    /// Make an RPC call without expecting a result (fire-and-forget)
    ///
    /// This is useful for notifications where you don't need to wait
    /// for a response.
    ///
    /// # Arguments
    ///
    /// - `method`: The RPC method name
    /// - `params`: The method parameters
    pub async fn notify<P>(&self, method: &str, params: P) -> Result<(), RpcError>
    where
        P: Serialize,
    {
        let params_value = serde_json::to_value(params)?;

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params_value,
        });

        let mut connector = self.connector.write().await;
        connector
            .send(request)
            .await
            .map_err(RpcError::Connection)?;

        Ok(())
    }

    /// Handle an incoming response message
    ///
    /// This should be called when a response is received from the server.
    /// It matches the response to the pending request and sends the result
    /// through the channel.
    pub async fn handle_response(&self, response: Value) {
        // Extract ID from response
        let id = match response.get("id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return, // Not a response, ignore
        };

        // Get pending request
        let tx = self.pending_requests.write().await.remove(&id);

        if let Some(tx) = tx {
            // Check for error
            if let Some(error) = response.get("error") {
                let error_msg = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");
                let _ = tx.send(Err(RpcError::ServerError(error_msg.to_string())));
                return;
            }

            // Extract result
            if let Some(result) = response.get("result") {
                let _ = tx.send(Ok(result.clone()));
            } else {
                let _ = tx.send(Err(RpcError::MissingResult));
            }
        }
    }

    /// Generate a unique request ID
    async fn generate_id(&self) -> String {
        let mut id = self.next_id.write().await;
        *id += 1;
        format!("req-{}", *id)
    }

    /// Get the connector (for advanced usage)
    pub fn connector(&self) -> Arc<RwLock<C>> {
        Arc::clone(&self.connector)
    }
}

/// RPC error types
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    /// Connection error
    #[error("Connection error: {0}")]
    Connection(#[from] ConnectionError),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Server returned an error
    #[error("Server error: {0}")]
    ServerError(String),

    /// Request timeout
    #[error("Request timeout")]
    Timeout,

    /// Missing result in response
    #[error("Missing result in response")]
    MissingResult,

    /// Channel error (receiver dropped)
    #[error("Channel error")]
    ChannelError,
}

impl From<oneshot::error::RecvError> for RpcError {
    fn from(_: oneshot::error::RecvError) -> Self {
        RpcError::ChannelError
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_error_display() {
        let err = RpcError::Timeout;
        assert_eq!(err.to_string(), "Request timeout");

        let err = RpcError::ServerError("test error".to_string());
        assert_eq!(err.to_string(), "Server error: test error");
    }
}
