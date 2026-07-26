//! MCP Approval Handler
//!
//! Manages human-in-the-loop approval requests from MCP servers.
//! Approval requests are forwarded to the UI layer via Gateway events.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::{oneshot, RwLock};
use tokio::time::timeout;

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::mcp::{ApprovalDecision, ApprovalRequest, ApprovalResponse};

/// Callback for presenting approval requests to the user
pub type ApprovalPresentCallback = Box<
    dyn Fn(ApprovalRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Pending approval request
struct PendingApproval {
    request: ApprovalRequest,
    respond_to: oneshot::Sender<ApprovalResponse>,
}

/// Handles approval requests from MCP servers
pub struct ApprovalHandler {
    /// Pending approvals by request ID
    pending: Arc<RwLock<HashMap<String, PendingApproval>>>,
    /// Callback to present requests to UI (Arc-wrapped for cheap cloning out of lock)
    present_callback: Arc<RwLock<Option<Arc<ApprovalPresentCallback>>>>,
    /// Default timeout for approvals
    default_timeout: Duration,
}

impl ApprovalHandler {
    /// Create a new approval handler
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            present_callback: Arc::new(RwLock::new(None)),
            default_timeout: Duration::from_secs(60),
        }
    }

    /// Create with custom default timeout
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            present_callback: Arc::new(RwLock::new(None)),
            default_timeout: timeout,
        }
    }

    /// Set the callback for presenting approval requests
    pub async fn set_present_callback<F, Fut>(&self, callback: F)
    where
        F: Fn(ApprovalRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut cb = self.present_callback.write().await;
        *cb = Some(Arc::new(Box::new(move |req| Box::pin(callback(req)))));
    }

    /// Request approval from the user
    ///
    /// Returns the user's decision or Timeout if no response within timeout.
    pub async fn request_approval(&self, request: ApprovalRequest) -> ApprovalDecision {
        // rust-doctor-disable-next-line excessive-clone
        let request_id = request.request_id.clone();
        let timeout_secs = request.timeout_seconds.unwrap_or_else(|| {
            self.default_timeout
                .as_secs()
                .try_into()
                .unwrap_or(u32::MAX)
        });
        let timeout_duration = Duration::from_secs(timeout_secs.into());

        // Create response channel
        let (tx, rx) = oneshot::channel();

        // Store pending request
        {
            let mut pending = self.pending.write().await;
            pending.insert(
                // rust-doctor-disable-next-line excessive-clone
                request_id.clone(),
                PendingApproval {
                    // rust-doctor-disable-next-line excessive-clone
                    request: request.clone(),
                    respond_to: tx,
                },
            );
        }

        // Fire PermissionRequest + Notification observers (best-effort,
        // no-op when no hooks are registered). Lets the user route MCP
        // approval requests through any external channel (desktop toast,
        // Telegram nudge, audit log) without touching this hot path.
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::PermissionRequest,
            &request_id,
            vec![
                ("KIND", "mcp_approval".to_string()),
                ("SERVER_NAME", request.server_name.clone()),
                ("ACTION", request.action.clone()),
            ],
        )
        .await;
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::Notification,
            &request_id,
            vec![
                ("KIND", "mcp_approval".to_string()),
                ("SERVER_NAME", request.server_name.clone()),
                (
                    "MESSAGE",
                    format!("MCP approval needed: {}", request.action),
                ),
            ],
        )
        .await;

        // Clone callback ref under lock, then release before awaiting UI interaction
        let cb = {
            let callback = self.present_callback.read().await;
            match callback.as_ref() {
                Some(cb) => Arc::clone(cb),
                None => {
                    tracing::warn!("No approval callback registered, auto-rejecting");
                    let mut pending = self.pending.write().await;
                    pending.remove(&request_id);
                    return ApprovalDecision::Rejected;
                }
            }
        };

        // Present to user (lock released)
        cb(request).await;

        // Wait for response with timeout
        match timeout(timeout_duration, rx).await {
            Ok(Ok(response)) => {
                // Clean up
                let mut pending = self.pending.write().await;
                pending.remove(&request_id);

                if response.approved {
                    ApprovalDecision::Approved
                } else {
                    ApprovalDecision::Rejected
                }
            }
            Ok(Err(_)) => {
                // Channel closed unexpectedly — sender dropped without responding
                tracing::warn!(request_id = %request_id, "Approval channel closed without response, treating as rejected");
                let mut pending = self.pending.write().await;
                pending.remove(&request_id);
                ApprovalDecision::Rejected
            }
            Err(_) => {
                // Timeout
                let mut pending = self.pending.write().await;
                pending.remove(&request_id);
                tracing::warn!(request_id = %request_id, "Approval request timed out");
                ApprovalDecision::Timeout
            }
        }
    }

    /// Submit user's response to an approval request
    pub async fn respond(
        &self,
        request_id: &str,
        approved: bool,
        reason: Option<String>,
    ) -> Result<()> {
        let mut pending = self.pending.write().await;

        if let Some(approval) = pending.remove(request_id) {
            let response = ApprovalResponse { approved, reason };
            let _ = approval.respond_to.send(response);
            Ok(())
        } else {
            Err(AlephError::NotFound(format!(
                "No pending approval with ID: {request_id}"
            )))
        }
    }

    /// Get all pending approval requests
    pub async fn list_pending(&self) -> Vec<ApprovalRequest> {
        let pending = self.pending.read().await;
        // rust-doctor-disable-next-line excessive-clone
        pending.values().map(|p| p.request.clone()).collect()
    }

    /// Cancel a pending approval request
    pub async fn cancel(&self, request_id: &str) {
        let mut pending = self.pending.write().await;
        if pending.remove(request_id).is_some() {
            tracing::debug!(request_id = %request_id, "Approval request cancelled");
        }
    }

    /// Get count of pending approvals
    pub async fn pending_count(&self) -> usize {
        self.pending.read().await.len()
    }
}

impl Default for ApprovalHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_approval_handler_creation() {
        let handler = ApprovalHandler::new();
        assert!(handler.list_pending().await.is_empty());
        assert_eq!(handler.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_respond_to_nonexistent() {
        let handler = ApprovalHandler::new();
        let result = handler.respond("nonexistent", true, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cancel_request() {
        let handler = ApprovalHandler::new();
        handler.cancel("nonexistent").await; // Should not panic
    }

    #[tokio::test]
    async fn test_no_callback_auto_rejects() {
        let handler = ApprovalHandler::new();
        let request = ApprovalRequest::new("req-1", "Test action", "test-server");

        let result = handler.request_approval(request).await;
        assert_eq!(result, ApprovalDecision::Rejected);
    }

    #[tokio::test]
    async fn test_with_timeout() {
        let handler = ApprovalHandler::with_timeout(Duration::from_secs(120));
        assert_eq!(handler.default_timeout, Duration::from_secs(120));
    }

    #[tokio::test]
    async fn test_approval_flow() {
        use crate::sync_primitives::{AtomicBool, Ordering};
        use tokio::sync::Notify;

        let handler = Arc::new(ApprovalHandler::new());
        let callback_called = Arc::new(AtomicBool::new(false));
        let notify = Arc::new(Notify::new());

        // Set up callback that will approve
        let callback_called_clone = callback_called.clone();
        let handler_clone = handler.clone();
        let notify_clone = notify.clone();

        handler
            .set_present_callback(move |req: ApprovalRequest| {
                let callback_called = callback_called_clone.clone();
                let handler = handler_clone.clone();
                let notify = notify_clone.clone();
                async move {
                    callback_called.store(true, Ordering::SeqCst);
                    // Simulate UI approval
                    let _ = handler.respond(&req.request_id, true, None).await;
                    notify.notify_one();
                }
            })
            .await;

        let request = ApprovalRequest::new("req-2", "Delete file", "filesystem-server");

        let result = handler.request_approval(request).await;

        assert!(callback_called.load(Ordering::SeqCst));
        assert_eq!(result, ApprovalDecision::Approved);
    }

    #[tokio::test]
    async fn test_rejection_flow() {
        let handler = Arc::new(ApprovalHandler::new());
        let handler_clone = handler.clone();

        handler
            .set_present_callback(move |req: ApprovalRequest| {
                let handler = handler_clone.clone();
                async move {
                    // Simulate UI rejection
                    let _ = handler
                        .respond(&req.request_id, false, Some("User declined".to_string()))
                        .await;
                }
            })
            .await;

        let request = ApprovalRequest::new("req-3", "Send email", "email-server");

        let result = handler.request_approval(request).await;
        assert_eq!(result, ApprovalDecision::Rejected);
    }

    #[tokio::test]
    async fn test_timeout_flow() {
        let handler = ApprovalHandler::with_timeout(Duration::from_millis(50));

        // Set callback that does NOT respond
        handler
            .set_present_callback(|_req: ApprovalRequest| async {
                // Do nothing - simulate user not responding
            })
            .await;

        let request = ApprovalRequest::new("req-4", "Slow operation", "slow-server");

        let result = handler.request_approval(request).await;
        assert_eq!(result, ApprovalDecision::Timeout);
        assert_eq!(handler.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_list_pending() {
        let handler = ApprovalHandler::new();

        // Without callback, requests auto-reject and don't stay pending
        // This test verifies the list_pending works on empty state
        assert!(handler.list_pending().await.is_empty());
    }
}
