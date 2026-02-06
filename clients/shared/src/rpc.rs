//! JSON-RPC 2.0 client implementation
//!
//! Handles request/response matching, ID generation, and RPC protocol details.

use crate::{ClientError, Result};
use aleph_protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{oneshot, RwLock};

#[cfg(feature = "tracing")]
use tracing::debug;

/// Pending RPC request
struct PendingRequest {
    tx: oneshot::Sender<std::result::Result<Value, JsonRpcError>>,
}

/// JSON-RPC client
///
/// Manages request ID generation and pending request tracking.
pub struct RpcClient {
    /// Pending requests waiting for response (id -> PendingRequest)
    pending: Arc<RwLock<HashMap<String, PendingRequest>>>,
    /// Request ID counter
    id_counter: AtomicU64,
}

impl RpcClient {
    /// Create a new RPC client
    pub fn new() -> Self {
        #[cfg(feature = "tracing")]
        debug!("Creating RPC client");

        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            id_counter: AtomicU64::new(1),
        }
    }

    /// Generate next request ID
    pub fn next_id(&self) -> String {
        let id = self.id_counter.fetch_add(1, Ordering::SeqCst);
        id.to_string()
    }

    /// Build a JSON-RPC request
    pub fn build_request<P: Serialize>(
        &self,
        method: &str,
        params: Option<P>,
        id: Option<String>,
    ) -> Result<JsonRpcRequest> {
        let params_value = params
            .map(|p| serde_json::to_value(p))
            .transpose()
            .map_err(|e| ClientError::SerializationError(e.to_string()))?;

        Ok(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: params_value,
            id: id.map(Value::String),
        })
    }

    /// Register a pending request
    ///
    /// Returns a receiver that will be resolved when the response arrives
    pub fn register_pending(&self, id: String) -> oneshot::Receiver<std::result::Result<Value, JsonRpcError>> {
        let (tx, rx) = oneshot::channel();

        let pending = self.pending.clone();
        tokio::spawn(async move {
            let mut pending_guard = pending.write().await;
            pending_guard.insert(id, PendingRequest { tx });
        });

        rx
    }

    /// Handle incoming response
    ///
    /// Matches response ID to pending request and resolves it
    pub async fn handle_response(&self, response: JsonRpcResponse) {
        let id = match &response.id {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => return, // No valid ID, ignore
        };

        let mut pending = self.pending.write().await;
        if let Some(req) = pending.remove(&id) {
            let result = if let Some(error) = response.error {
                Err(error)
            } else {
                Ok(response.result.unwrap_or(Value::Null))
            };
            let _ = req.tx.send(result);
        }
    }

    /// Send request and wait for response
    ///
    /// This is a convenience method that combines:
    /// 1. Generating ID
    /// 2. Registering pending request
    /// 3. Waiting for response with timeout
    pub async fn call_with_timeout<R: DeserializeOwned>(
        &self,
        rx: oneshot::Receiver<std::result::Result<Value, JsonRpcError>>,
        timeout: Duration,
        id: String,
    ) -> Result<R> {
        // Wait for response with timeout
        let result = tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| {
                // Remove pending request on timeout
                let pending = self.pending.clone();
                let id_clone = id.clone();
                tokio::spawn(async move {
                    pending.write().await.remove(&id_clone);
                });
                ClientError::Timeout
            })?
            .map_err(|_| ClientError::ConnectionClosed)?;

        match result {
            Ok(value) => {
                let result: R = serde_json::from_value(value)
                    .map_err(|e| ClientError::SerializationError(e.to_string()))?;
                Ok(result)
            }
            Err(error) => Err(ClientError::RpcError(format!(
                "{}: {}",
                error.code, error.message
            ))),
        }
    }

    /// Clear all pending requests
    pub async fn clear_pending(&self) {
        let mut pending = self.pending.write().await;
        pending.clear();
    }

    /// Get count of pending requests
    pub async fn pending_count(&self) -> usize {
        let pending = self.pending.read().await;
        pending.len()
    }
}

impl Default for RpcClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_generation() {
        let rpc = RpcClient::new();
        assert_eq!(rpc.next_id(), "1");
        assert_eq!(rpc.next_id(), "2");
        assert_eq!(rpc.next_id(), "3");
    }

    #[test]
    fn test_build_request() {
        let rpc = RpcClient::new();

        let req = rpc.build_request::<()>("test.method", None, Some("123".to_string())).unwrap();
        assert_eq!(req.method, "test.method");
        assert_eq!(req.id, Some(Value::String("123".to_string())));
    }

    #[tokio::test]
    async fn test_pending_tracking() {
        let rpc = RpcClient::new();

        let rx = rpc.register_pending("test-id".to_string());
        assert_eq!(rpc.pending_count().await, 1);

        // Simulate response
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Value::String("test-id".to_string()),
            result: Some(serde_json::json!({"success": true})),
            error: None,
        };

        rpc.handle_response(response).await;

        // Verify request was resolved
        let result = rx.await.unwrap().unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(rpc.pending_count().await, 0);
    }
}
