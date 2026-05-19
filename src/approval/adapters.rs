//! Adapter: bridges tool-level `ApprovalRequester` onto the legacy
//! `ChannelApprovalBridge` transport.
//!
//! The adapter holds:
//! - `Arc<ChannelApprovalBridge>` — delivers the prompt to the user's channel.
//! - `Arc<ExecApprovalManager>` — registers the per-request oneshot that the
//!   channel's button-click callback resolves.
//!
//! The adapter reads the current session from the `SESSION_ID` task-local
//! (set at every tool entry point by `with_session_scope`) and derives the
//! delivery route `(ChannelId, ConversationId)` directly from the structured
//! `SessionKey` via `channel_route` — no lossy string parsing.
//!
//! A session with no channel origin (`Main` / `Task` / `Ephemeral`), a missing
//! `SESSION_ID`, or an unreachable channel produces `Denied` with an explicit
//! warning — never a silent auto-approve.

use async_trait::async_trait;

use crate::sync_primitives::Arc;

use crate::approval::session_route::channel_route;
use crate::exec::approval::channel_bridge::ChannelApprovalBridge;
use crate::exec::manager::{ExecApprovalManager, DEFAULT_APPROVAL_TIMEOUT_MS};
use crate::gateway::channel::{ChannelId, ConversationId};
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester};

/// Adapts `ChannelApprovalBridge` + `ExecApprovalManager` to the
/// `ApprovalRequester` trait.
///
/// The adapter performs the full two-stage approval flow: deliver prompt via
/// the channel, then wait on the approval manager's oneshot for the real
/// user decision. Delivery success alone never counts as an approval.
pub struct ChannelApprovalBridgeAdapter {
    bridge: Arc<ChannelApprovalBridge>,
    approval_manager: Arc<ExecApprovalManager>,
    timeout_ms: u64,
}

impl ChannelApprovalBridgeAdapter {
    /// Construct a new adapter. Uses the default 2-minute approval timeout.
    pub fn new(
        bridge: Arc<ChannelApprovalBridge>,
        approval_manager: Arc<ExecApprovalManager>,
    ) -> Self {
        Self {
            bridge,
            approval_manager,
            timeout_ms: DEFAULT_APPROVAL_TIMEOUT_MS,
        }
    }

    /// Override the default approval timeout. Useful for tests.
    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// 从 `SESSION_ID` task-local 的结构化 `SessionKey` 解出通道路由。
    /// task-local 未设置、或会话无通道来源时返回 `None`。
    fn current_channel_route() -> Option<(ChannelId, ConversationId)> {
        crate::sandbox::context::SESSION_ID
            .try_with(|sid| channel_route(sid))
            .ok()
            .flatten()
    }
}

#[async_trait]
impl ApprovalRequester for ChannelApprovalBridgeAdapter {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome {
        let Some((channel_id, conversation_id)) = Self::current_channel_route() else {
            tracing::warn!(
                tool = %tool_name,
                "ChannelApprovalBridgeAdapter: no channel route from SESSION_ID \
                 (unset, or Main/Task/Ephemeral session) — denying"
            );
            return ApprovalOutcome::Denied;
        };

        self.bridge
            .request_for_tool(
                &self.approval_manager,
                tool_name,
                reason,
                &channel_id,
                &conversation_id,
                "",
                self.timeout_ms,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::{DmScope, SessionKey};
    use crate::session::with_session_scope;

    fn test_manager() -> Arc<ExecApprovalManager> {
        Arc::new(ExecApprovalManager::new())
    }

    /// DM session key — `channel_route` 能解出 `("telegram","123456")`。
    fn test_session_id() -> crate::session::service::SessionId {
        SessionKey::dm("main", "telegram", "123456", DmScope::PerPeer)
    }

    #[tokio::test]
    async fn adapter_forwards_approved() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_approved());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let sid = test_session_id();
        let out = with_session_scope(&sid, async {
            adapter.request_approval("code_exec", "run ls").await
        })
        .await;
        assert_eq!(out, ApprovalOutcome::Approved);
    }

    #[tokio::test]
    async fn adapter_forwards_denied() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_denied());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let sid = test_session_id();
        let out = with_session_scope(&sid, async {
            adapter.request_approval("code_exec", "rm -rf").await
        })
        .await;
        assert_eq!(out, ApprovalOutcome::Denied);
    }

    /// Negative path: no `SESSION_ID` in scope → Denied. Exercises the real
    /// code path (no `test_outcome_override` fast path).
    #[tokio::test]
    async fn adapter_denies_when_session_id_unset() {
        use crate::gateway::channel_registry::ChannelRegistry;

        let registry = Arc::new(ChannelRegistry::new());
        let bridge = Arc::new(ChannelApprovalBridge::new(registry));
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());

        // No `with_session_scope` wrapping — task-local is unset.
        let out = adapter.request_approval("code_exec", "run ls").await;
        assert_eq!(out, ApprovalOutcome::Denied);
    }

    /// Negative path: SESSION_ID is set but no channel capability is registered
    /// in the registry. Bridge's `request_approval` returns `None` → adapter
    /// must deny (not approve). This exercises the REAL code path end-to-end
    /// via `request_for_tool` → `request_approval` → `None` → `Denied`.
    #[tokio::test]
    async fn adapter_denies_when_channel_missing() {
        use crate::gateway::channel_registry::ChannelRegistry;

        let registry = Arc::new(ChannelRegistry::new());
        let bridge = Arc::new(ChannelApprovalBridge::new(registry));
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager()).with_timeout_ms(50);

        let sid = test_session_id();
        let out = with_session_scope(&sid, async {
            adapter.request_approval("code_exec", "run ls").await
        })
        .await;
        assert_eq!(out, ApprovalOutcome::Denied);
    }
}
