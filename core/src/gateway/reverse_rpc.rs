//! Reverse RPC mechanism for Server-to-Client tool calls.
//!
//! Allows Server to send JSON-RPC requests to Client and await responses,
//! enabling remote tool execution on Client side.

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use dashmap::DashMap;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;
use thiserror::Error;

/// Errors that can occur during reverse RPC calls.
#[derive(Debug, Error)]
pub enum ReverseRpcError {
    #[error("Connection closed before response received")]
    ConnectionClosed,

    #[error("Request timed out after {0:?}")]
    Timeout(Duration),

    #[error("Client returned error: {code} - {message}")]
    ClientError { code: i32, message: String },

    #[error("Failed to send request: {0}")]
    SendFailed(String),
}

/// Manages pending reverse RPC requests and their responses.
pub struct ReverseRpcManager {
    /// Pending requests: request_id -> oneshot sender
    pending: DashMap<String, oneshot::Sender<JsonRpcResponse>>,

    /// Request ID counter
    id_counter: AtomicU64,

    /// Default timeout for requests
    default_timeout: Duration,
}

impl ReverseRpcManager {
    /// Create a new ReverseRpcManager with default 30s timeout.
    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(30))
    }

    /// Create a new ReverseRpcManager with custom timeout.
    pub fn with_timeout(default_timeout: Duration) -> Self {
        Self {
            pending: DashMap::new(),
            id_counter: AtomicU64::new(1),
            default_timeout,
        }
    }

    /// Generate next unique request ID.
    fn next_id(&self) -> String {
        let id = self.id_counter.fetch_add(1, Ordering::SeqCst);
        format!("rev_{}", id)
    }

    /// Create a request and register it for response handling.
    ///
    /// Returns the request to send and a future that resolves when response arrives.
    pub fn create_request(
        &self,
        method: &str,
        params: Value,
    ) -> (JsonRpcRequest, PendingRequest) {
        let id = self.next_id();
        let (tx, rx) = oneshot::channel();

        self.pending.insert(id.clone(), tx);

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: Some(params),
            id: Some(Value::String(id.clone())),
        };

        let pending = PendingRequest {
            id,
            receiver: rx,
            default_timeout: self.default_timeout,
        };

        (request, pending)
    }

    /// Handle an incoming response from Client.
    ///
    /// Matches response ID to pending request and completes the future.
    pub fn handle_response(&self, response: JsonRpcResponse) -> bool {
        if let Some(Value::String(id)) = &response.id {
            if let Some((_, tx)) = self.pending.remove(id) {
                let _ = tx.send(response);
                return true;
            }
        }
        false
    }

    /// Cancel a pending request (e.g., on connection close).
    pub fn cancel(&self, request_id: &str) {
        self.pending.remove(request_id);
    }

    /// Cancel all pending requests for a connection.
    pub fn cancel_all(&self) {
        self.pending.clear();
    }

    /// Get count of pending requests.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

impl Default for ReverseRpcManager {
    fn default() -> Self {
        Self::new()
    }
}

/// A pending reverse RPC request awaiting response.
pub struct PendingRequest {
    id: String,
    receiver: oneshot::Receiver<JsonRpcResponse>,
    default_timeout: Duration,
}

impl PendingRequest {
    /// Get the request ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Wait for response with default timeout.
    pub async fn wait(self) -> Result<Value, ReverseRpcError> {
        let timeout = self.default_timeout;
        self.wait_timeout(timeout).await
    }

    /// Wait for response with custom timeout.
    pub async fn wait_timeout(self, timeout: Duration) -> Result<Value, ReverseRpcError> {
        match tokio::time::timeout(timeout, self.receiver).await {
            Ok(Ok(response)) => {
                if let Some(error) = response.error {
                    Err(ReverseRpcError::ClientError {
                        code: error.code,
                        message: error.message,
                    })
                } else {
                    Ok(response.result.unwrap_or(Value::Null))
                }
            }
            Ok(Err(_)) => Err(ReverseRpcError::ConnectionClosed),
            Err(_) => Err(ReverseRpcError::Timeout(timeout)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_request_generates_unique_ids() {
        let manager = ReverseRpcManager::new();

        let (req1, _) = manager.create_request("test", Value::Null);
        let (req2, _) = manager.create_request("test", Value::Null);

        assert_ne!(req1.id, req2.id);
    }

    #[test]
    fn test_handle_response_matches_pending() {
        let manager = ReverseRpcManager::new();

        let (req, _pending) = manager.create_request("test", Value::Null);
        assert_eq!(manager.pending_count(), 1);

        let response = JsonRpcResponse::success(req.id.clone(), Value::String("ok".to_string()));
        let matched = manager.handle_response(response);

        assert!(matched);
        assert_eq!(manager.pending_count(), 0);
    }

    #[test]
    fn test_handle_response_ignores_unknown() {
        let manager = ReverseRpcManager::new();

        let response = JsonRpcResponse::success(
            Some(Value::String("unknown_id".to_string())),
            Value::Null,
        );
        let matched = manager.handle_response(response);

        assert!(!matched);
    }

    #[tokio::test]
    async fn test_pending_request_receives_response() {
        let manager = ReverseRpcManager::new();

        let (req, pending) = manager.create_request("test", Value::Null);

        // Simulate response in another task
        let response = JsonRpcResponse::success(req.id.clone(), Value::String("result".to_string()));
        manager.handle_response(response);

        let result = pending.wait_timeout(Duration::from_millis(100)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::String("result".to_string()));
    }

    #[tokio::test]
    async fn test_pending_request_timeout() {
        let manager = ReverseRpcManager::with_timeout(Duration::from_millis(10));

        let (_req, pending) = manager.create_request("test", Value::Null);

        // Don't send response, let it timeout
        let result = pending.wait().await;
        assert!(matches!(result, Err(ReverseRpcError::Timeout(_))));
    }
}