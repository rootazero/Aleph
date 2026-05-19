//! Adapter: bridges tool-level `ApprovalRequester` onto the
//! `ChannelApprovalBridge` transport.
//!
//! The adapter holds:
//! - `Arc<ChannelApprovalBridge>` — delivers the prompt to the user's channel.
//! - `Arc<ExecApprovalManager>` — registers the per-request oneshot that the
//!   inbound router's `/approve` / `/deny` reply resolves.
//!
//! The adapter reads the current turn's routing context from the `TURN_CONTEXT`
//! task-local (scoped by `ScopedToolService::execute` around every tool call).
//! That context carries the session key plus the originating channel id and
//! conversation id, so the prompt routes correctly for every session type —
//! including per-peer DMs, whose channel a session key alone cannot recover.
//!
//! Missing `TURN_CONTEXT`, a non-channel turn, or an unreachable channel
//! produces `Denied` with an explicit warning — never a silent auto-approve.

use async_trait::async_trait;

use crate::sync_primitives::Arc;

use crate::exec::approval::channel_bridge::ChannelApprovalBridge;
use crate::exec::manager::{ExecApprovalManager, DEFAULT_APPROVAL_TIMEOUT_MS};
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
}

#[async_trait]
impl ApprovalRequester for ChannelApprovalBridgeAdapter {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome {
        let Some(turn) = crate::tools::turn_context::current_turn_context() else {
            tracing::warn!(
                tool = %tool_name,
                "ChannelApprovalBridgeAdapter: TURN_CONTEXT not set at approval \
                 point — cannot route prompt to a channel, denying"
            );
            return ApprovalOutcome::Denied;
        };
        if !turn.is_channel_routable() {
            tracing::warn!(
                tool = %tool_name,
                session = %turn.session_key,
                "ChannelApprovalBridgeAdapter: turn has no originating channel \
                 — cannot route approval prompt, denying"
            );
            return ApprovalOutcome::Denied;
        }

        self.bridge
            .request_for_tool(
                &self.approval_manager,
                tool_name,
                reason,
                &turn.session_key.to_string(),
                &turn.channel_id,
                &turn.conversation_id,
                "",
                self.timeout_ms,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    fn test_manager() -> Arc<ExecApprovalManager> {
        Arc::new(ExecApprovalManager::new())
    }

    /// A routable turn context pointing at a `telegram` DM.
    fn routable_turn() -> TurnContext {
        TurnContext {
            session_key: crate::routing::session_key::SessionKey::ephemeral("adapter-test"),
            channel_id: "telegram".to_string(),
            conversation_id: "user-1".to_string(),
        }
    }

    #[tokio::test]
    async fn adapter_forwards_approved() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_approved());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let out = TURN_CONTEXT
            .scope(routable_turn(), async {
                adapter.request_approval("code_exec", "run ls").await
            })
            .await;
        assert_eq!(out, ApprovalOutcome::Approved);
    }

    #[tokio::test]
    async fn adapter_forwards_denied() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_denied());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let out = TURN_CONTEXT
            .scope(routable_turn(), async {
                adapter.request_approval("code_exec", "rm -rf").await
            })
            .await;
        assert_eq!(out, ApprovalOutcome::Denied);
    }

    /// Negative path: no `TURN_CONTEXT` in scope → Denied. Exercises the real
    /// code path (no `test_outcome_override` fast path).
    #[tokio::test]
    async fn adapter_denies_when_turn_context_unset() {
        use crate::gateway::channel_registry::ChannelRegistry;

        let registry = Arc::new(ChannelRegistry::new());
        let bridge = Arc::new(ChannelApprovalBridge::new(registry));
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());

        // No `TURN_CONTEXT.scope` wrapping — task-local is unset.
        let out = adapter.request_approval("code_exec", "run ls").await;
        assert_eq!(out, ApprovalOutcome::Denied);
    }

    /// Negative path: `TURN_CONTEXT` is set but the turn has no originating
    /// channel (cron / webhook) → Denied before the bridge is even consulted.
    #[tokio::test]
    async fn adapter_denies_when_turn_not_channel_routable() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_approved());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let non_channel_turn = TurnContext {
            session_key: crate::routing::session_key::SessionKey::task(
                "main", "cron", "daily",
            ),
            channel_id: String::new(),
            conversation_id: String::new(),
        };
        let out = TURN_CONTEXT
            .scope(non_channel_turn, async {
                adapter.request_approval("code_exec", "run ls").await
            })
            .await;
        // Even with an always-approved bridge, an unroutable turn is denied.
        assert_eq!(out, ApprovalOutcome::Denied);
    }

    /// Negative path: `TURN_CONTEXT` is set and routable but no channel is
    /// registered. Bridge's send fails → adapter must deny (not approve).
    /// Exercises the REAL code path end-to-end (no `test_outcome_override`).
    #[tokio::test]
    async fn adapter_denies_when_channel_missing() {
        use crate::gateway::channel_registry::ChannelRegistry;

        let registry = Arc::new(ChannelRegistry::new());
        let bridge = Arc::new(ChannelApprovalBridge::new(registry));
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager()).with_timeout_ms(50);

        let out = TURN_CONTEXT
            .scope(routable_turn(), async {
                adapter.request_approval("code_exec", "run ls").await
            })
            .await;
        assert_eq!(out, ApprovalOutcome::Denied);
    }
}
