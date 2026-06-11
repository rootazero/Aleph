//! Adapter: bridges tool-level `ApprovalRequester` onto the
//! `ChannelApprovalBridge` transport.
//!
//! The adapter holds:
//! - `Arc<ChannelApprovalBridge>` — delivers the prompt to the user's channel.
//! - `Arc<ExecApprovalManager>` — registers the per-request oneshot that the
//!   inbound router's `/approve` / `/deny` reply resolves.
//!
//! ## Channel routing
//!
//! The adapter routes via two sources, in order:
//!
//! 1. **`TURN_CONTEXT`** task-local (HITL P3) — scoped by
//!    `ScopedToolService::execute`, the production gateway tool-dispatch
//!    chokepoint. Carries `channel_id` + `conversation_id` directly; works on
//!    every gateway-driven session type including per-peer DMs whose
//!    `SessionKey` alone does not encode channel coordinates.
//!
//! 2. **`SESSION_ID`** task-local → `channel_route(SessionKey)` — scoped by
//!    `with_session_scope` / `invoke_with_session_trace` (non-gateway paths:
//!    cron, heartbeat, scheduled tools). Recovers channel from the structured
//!    `SessionKey`.
//!
//! Both unset, a non-channel turn, or an unreachable channel → `Denied` with
//! an explicit warning. Never a silent auto-approve.

use async_trait::async_trait;
use tracing::warn;

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
    #[must_use]
    pub const fn new(
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
    pub const fn set_timeout_ms(&mut self, timeout_ms: u64) {
        self.timeout_ms = timeout_ms;
    }

    /// Resolve `(ChannelId, ConversationId, session_key)` for the current task.
    ///
    /// Tries `TURN_CONTEXT` first (gateway path — scoped by
    /// `ScopedToolService`), then `SESSION_ID` + `channel_route` fallback
    /// (legacy `invoke_with_session_trace` consumers). Returns `None` when
    /// neither is set or neither has a routable channel.
    ///
    /// The session key string matches the inbound router's
    /// `ctx.session_key.to_string()` form, so the approval record it stamps
    /// is resolvable by `/approve` / `/deny` text replies
    /// (`resolve_for_session`), not only by button callbacks.
    fn resolve_channel_route() -> Option<(ChannelId, ConversationId, String)> {
        // 1. TURN_CONTEXT — the gateway production path (HITL P3).
        if let Some(turn) = crate::tools::turn_context::current_turn_context() {
            if turn.is_channel_routable() {
                return Some((
                    ChannelId::new(turn.channel_id),
                    ConversationId::new(turn.conversation_id),
                    turn.session_key.to_key_string(),
                ));
            }
        }
        // 2. SESSION_ID → channel_route — cron/heartbeat/legacy paths.
        crate::sandbox::context::SESSION_ID
            .try_with(|sid| {
                channel_route(sid).map(|(channel_id, conversation_id)| {
                    (channel_id, conversation_id, sid.to_key_string())
                })
            })
            .ok()
            .flatten()
    }
}

#[async_trait]
impl ApprovalRequester for ChannelApprovalBridgeAdapter {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome {
        let Some((channel_id, conversation_id, session_key)) = Self::resolve_channel_route() else {
            warn!(
                tool = %tool_name,
                "ChannelApprovalBridgeAdapter: no channel route from TURN_CONTEXT \
                 or SESSION_ID — cannot route approval prompt, denying"
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
                &session_key,
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
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    fn test_manager() -> Arc<ExecApprovalManager> {
        Arc::new(ExecApprovalManager::new())
    }

    /// DM session key — `channel_route` resolves to `("telegram","123456")`.
    fn test_session_id() -> crate::session::service::SessionId {
        SessionKey::dm("main", "telegram", "123456", DmScope::PerPeer)
    }

    /// A routable turn context pointing at a `telegram` DM.
    fn routable_turn() -> TurnContext {
        TurnContext {
            session_key: SessionKey::ephemeral("adapter-test"),
            run_id: String::new(),
            channel_id: "telegram".to_string(),
            conversation_id: "user-1".to_string(),
            caller_role: None,
        }
    }

    /// Gateway path: TURN_CONTEXT routes the approval, bridge approves.
    #[tokio::test]
    async fn adapter_forwards_approved_via_turn_context() {
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
    async fn adapter_forwards_denied_via_turn_context() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_denied());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let out = TURN_CONTEXT
            .scope(routable_turn(), async {
                adapter.request_approval("code_exec", "rm -rf").await
            })
            .await;
        assert_eq!(out, ApprovalOutcome::Denied);
    }

    /// Legacy path: SESSION_ID + channel_route fallback works when
    /// TURN_CONTEXT is unset (cron/heartbeat callers).
    #[tokio::test]
    async fn adapter_forwards_via_session_id_fallback() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_approved());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let sid = test_session_id();
        let out = with_session_scope(&sid, async {
            adapter.request_approval("code_exec", "run ls").await
        })
        .await;
        assert_eq!(out, ApprovalOutcome::Approved);
    }

    /// Negative path: no TURN_CONTEXT and no SESSION_ID → Denied.
    #[tokio::test]
    async fn adapter_denies_when_no_route_source() {
        use crate::gateway::channel_registry::ChannelRegistry;

        let registry = Arc::new(ChannelRegistry::new());
        let bridge = Arc::new(ChannelApprovalBridge::new(registry));
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());

        // Neither task-local scoped.
        let out = adapter.request_approval("code_exec", "run ls").await;
        assert_eq!(out, ApprovalOutcome::Denied);
    }

    /// Negative path: TURN_CONTEXT set but turn has no originating channel
    /// (cron / webhook) → Denied before the bridge is even consulted.
    #[tokio::test]
    async fn adapter_denies_when_turn_not_channel_routable() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_approved());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let non_channel_turn = TurnContext {
            session_key: SessionKey::task("main", "cron", "daily"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
        };
        let out = TURN_CONTEXT
            .scope(non_channel_turn, async {
                adapter.request_approval("code_exec", "run ls").await
            })
            .await;
        // Even with an always-approved bridge, an unroutable turn falls
        // through to SESSION_ID fallback; with no SESSION_ID scoped either,
        // we deny.
        assert_eq!(out, ApprovalOutcome::Denied);
    }

    /// Negative path: routable TURN_CONTEXT but no registered channel.
    /// Bridge's send fails → adapter must deny (not approve).
    #[tokio::test]
    async fn adapter_denies_when_channel_missing() {
        use crate::gateway::channel_registry::ChannelRegistry;

        let registry = Arc::new(ChannelRegistry::new());
        let bridge = Arc::new(ChannelApprovalBridge::new(registry));
        let mut adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        adapter.set_timeout_ms(50);

        let out = TURN_CONTEXT
            .scope(routable_turn(), async {
                adapter.request_approval("code_exec", "run ls").await
            })
            .await;
        assert_eq!(out, ApprovalOutcome::Denied);
    }
}
