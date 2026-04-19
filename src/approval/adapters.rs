//! Adapter: bridges tool-level `ApprovalRequester` onto the legacy
//! `ChannelApprovalBridge` transport.

use std::sync::Arc;

use async_trait::async_trait;

use crate::agent_loop::exec_approval::gate::{ApprovalOutcome, ApprovalRequester};
use crate::exec::approval::channel_bridge::ChannelApprovalBridge;

/// Adapts `ChannelApprovalBridge` to the `ApprovalRequester` trait.
///
/// Synthesises a legacy `ApprovalRequest` from the tool name and reason,
/// delegates to `ChannelApprovalBridge::request_for_tool`, and maps the
/// result to `ApprovalOutcome`.
pub struct ChannelApprovalBridgeAdapter {
    bridge: Arc<ChannelApprovalBridge>,
}

impl ChannelApprovalBridgeAdapter {
    pub fn new(bridge: Arc<ChannelApprovalBridge>) -> Self {
        Self { bridge }
    }
}

#[async_trait]
impl ApprovalRequester for ChannelApprovalBridgeAdapter {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome {
        self.bridge.request_for_tool(tool_name, reason).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn adapter_forwards_approved() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_approved());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge);
        let out = adapter.request_approval("code_exec", "run ls").await;
        assert_eq!(out, ApprovalOutcome::Approved);
    }

    #[tokio::test]
    async fn adapter_forwards_denied() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_denied());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge);
        let out = adapter.request_approval("code_exec", "rm -rf").await;
        assert_eq!(out, ApprovalOutcome::Denied);
    }
}
