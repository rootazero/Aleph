//! Narrow bridge between the Interface layer and core approval management.
//!
//! `InboundMessageRouter` stays pure I/O: hands the `callback_data` to the
//! injected `ApprovalCallbackSink`, then renders the returned text back to the
//! channel — it neither parses nor resolves.

use async_trait::async_trait;

/// Result of parsing an approval button callback.
pub struct ApprovalCallbackResult {
    /// Whether a pending approval was actually resolved (false = expired / unknown id).
    pub resolved: bool,
    /// User-visible text rendered back to the channel.
    pub response_text: String,
}

/// Approval callback sink injected into the router. The concrete impl wraps
/// `ExecApprovalManager`.
#[async_trait]
pub trait ApprovalCallbackSink: Send + Sync {
    /// Returns `Some` if and only if `callback_data` is an approval button
    /// callback; `None` means it is not an approval callback and the router
    /// should let the request through into the normal message flow.
    async fn handle_callback(
        &self,
        callback_data: &str,
        user_id: &str,
    ) -> Option<ApprovalCallbackResult>;
}
