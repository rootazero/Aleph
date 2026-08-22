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
//! Both unset or a non-channel turn → `Denied` with an explicit warning. Never
//! a silent auto-approve.
//!
//! A resolvable-but-unregistered channel (the Panel's `gui:chat`) is NOT a
//! denial: [`FallbackApprovalRequester`] routes those to the operator
//! transport instead. See its docs.

use async_trait::async_trait;
use tracing::warn;

use crate::sync_primitives::Arc;

use crate::approval::operator_requester::OperatorApprovalRequester;
use crate::approval::session_route::channel_route;
use crate::exec::approval::channel_bridge::ChannelApprovalBridge;
use crate::exec::manager::{ExecApprovalManager, DEFAULT_APPROVAL_TIMEOUT_MS};
use crate::gateway::channel::{ChannelId, ConversationId};
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester, ApprovalResponse};
use crate::sandbox::exec_approval::ApprovalAction;

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

    /// Whether this turn's approval prompt can actually be delivered: a channel
    /// route resolves AND that channel is registered. False for Panel turns
    /// (`gui:chat` is never a registered channel) and for non-channel turns.
    pub async fn can_reach_user(&self) -> bool {
        match Self::resolve_channel_route() {
            Some((channel_id, _, _)) => self.bridge.can_deliver(&channel_id).await,
            None => false,
        }
    }
}

/// Routes an approval to the user's channel when one is reachable, and to the
/// server operator (event bus + `exec.approval.resolve`) when it is not.
///
/// The Panel is the reason this exists: its turns carry `channel_id="gui:chat"`,
/// a pseudo-channel that is deliberately never registered, so the channel
/// transport alone resolves every Panel approval to `Denied` without ever
/// prompting — `ask_user` and every `requires_confirmation` / `Ask`-tier tool
/// silently refused. Neither existing requester can cover both surfaces alone
/// and there is no combinator, so this composite is the seam.
///
/// Fail-closed, but NOT fail-fast: with no channel route the approval goes to
/// the operator and parks until the approval timeout, whose `Timeout` outcome
/// is not an approval. There is no cheap proof that an operator is watching —
/// the event bus only knows raw broadcast subscribers, and internal ones (the
/// R5 router, the provider hot-reload watcher) always exist, so a subscriber
/// count can only ever say "yes". A guard built on it would always pass, which
/// is why the former `can_reach_operator()` was deleted rather than kept as a
/// lie. Failing fast needs a count of AUTHORIZED GATEWAY CONNECTIONS (the only
/// surfaces that can call `exec.approval.resolve`); wiring the presence tracker
/// down here is the real fix.
pub struct FallbackApprovalRequester {
    channel: Arc<ChannelApprovalBridgeAdapter>,
    operator: Arc<OperatorApprovalRequester>,
}

impl FallbackApprovalRequester {
    #[must_use]
    pub const fn new(
        channel: Arc<ChannelApprovalBridgeAdapter>,
        operator: Arc<OperatorApprovalRequester>,
    ) -> Self {
        Self { channel, operator }
    }
}

#[async_trait]
impl ApprovalRequester for FallbackApprovalRequester {
    async fn request_approval(&self, action: &ApprovalAction) -> ApprovalResponse {
        if self.channel.can_reach_user().await {
            return self.channel.request_approval(action).await;
        }
        self.operator.request_approval(action).await
    }
}

#[async_trait]
impl ApprovalRequester for ChannelApprovalBridgeAdapter {
    async fn request_approval(&self, action: &ApprovalAction) -> ApprovalResponse {
        let Some((channel_id, conversation_id, session_key)) = Self::resolve_channel_route() else {
            warn!(
                tool = %action.tool_name,
                "ChannelApprovalBridgeAdapter: no channel route from TURN_CONTEXT \
                 or SESSION_ID — cannot route approval prompt, failing closed"
            );
            // `Unavailable`, not `Denied`: no card was ever rendered, so this is
            // the absence of a decision. Spelling it `Denied` made the ledger
            // treat an unroutable turn as a refusal the user had made.
            return ApprovalOutcome::Unavailable.into();
        };

        // The originating channel user (raw sender), published run-tree-wide by
        // `run_agent_loop`. Threaded onto the pending record so only this user
        // can resolve it via a channel button (group-chat approval-bypass fix).
        let originator = crate::tools::turn_context::current_originator();
        self.bridge
            .request_for_tool(
                &self.approval_manager,
                action,
                &channel_id,
                &conversation_id,
                &session_key,
                originator.as_deref(),
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

    /// A `code_exec` call carrying its real code body.
    fn code_action(code: &str) -> ApprovalAction {
        ApprovalAction::for_tool_call(
            "code_exec",
            &serde_json::json!({ "language": "shell", "code": code }),
            "needs confirmation",
        )
    }

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
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        }
    }

    /// Gateway path: TURN_CONTEXT routes the approval, bridge approves.
    #[tokio::test]
    async fn adapter_forwards_approved_via_turn_context() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_approved());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let out = TURN_CONTEXT
            .scope(routable_turn(), async {
                adapter.request_approval(&code_action("ls")).await
            })
            .await;
        assert_eq!(out.outcome, ApprovalOutcome::Approved);
    }

    #[tokio::test]
    async fn adapter_forwards_denied_via_turn_context() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_denied());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let out = TURN_CONTEXT
            .scope(routable_turn(), async {
                adapter.request_approval(&code_action("rm -rf")).await
            })
            .await;
        assert_eq!(out.outcome, ApprovalOutcome::Denied);
    }

    /// Legacy path: SESSION_ID + channel_route fallback works when
    /// TURN_CONTEXT is unset (cron/heartbeat callers).
    #[tokio::test]
    async fn adapter_forwards_via_session_id_fallback() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_approved());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let sid = test_session_id();
        let out = with_session_scope(&sid, async {
            adapter.request_approval(&code_action("ls")).await
        })
        .await;
        assert_eq!(out.outcome, ApprovalOutcome::Approved);
    }

    /// Negative path: no TURN_CONTEXT and no SESSION_ID → Denied.
    #[tokio::test]
    async fn adapter_fails_closed_when_no_route_source() {
        use crate::gateway::channel_registry::ChannelRegistry;

        let registry = Arc::new(ChannelRegistry::new());
        let bridge = Arc::new(ChannelApprovalBridge::new(registry));
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());

        // Neither task-local scoped.
        let out = adapter.request_approval(&code_action("ls")).await;
        // `Unavailable`, not `Denied`: the card was never rendered, so nobody
        // refused it — and the denial ledger must not make it sticky or count
        // it toward the brute-force breaker.
        assert_eq!(out.outcome, ApprovalOutcome::Unavailable);
        assert!(!out.outcome.is_approved());
    }

    /// Negative path: TURN_CONTEXT set but turn has no originating channel
    /// (cron / webhook) → fails closed before the bridge is even consulted.
    #[tokio::test]
    async fn adapter_fails_closed_when_turn_not_channel_routable() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_approved());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge, test_manager());
        let non_channel_turn = TurnContext {
            session_key: SessionKey::task("main", "cron", "daily"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        };
        let out = TURN_CONTEXT
            .scope(non_channel_turn, async {
                adapter.request_approval(&code_action("ls")).await
            })
            .await;
        // Even with an always-approved bridge, an unroutable turn falls
        // through to SESSION_ID fallback; with no SESSION_ID scoped either,
        // we fail closed — as "nobody could be asked", not as a refusal.
        assert_eq!(out.outcome, ApprovalOutcome::Unavailable);
        assert!(!out.outcome.is_approved());
    }

    /// A Panel turn: `gui:chat` resolves as a route but is never a registered
    /// channel, so the channel transport cannot reach the user.
    fn panel_turn() -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("adapter-test"),
            run_id: String::new(),
            channel_id: "gui:chat".to_string(),
            conversation_id: "sess-1".to_string(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        }
    }

    fn unregistered_adapter() -> Arc<ChannelApprovalBridgeAdapter> {
        use crate::gateway::channel_registry::ChannelRegistry;
        let bridge = Arc::new(ChannelApprovalBridge::new(Arc::new(ChannelRegistry::new())));
        Arc::new(ChannelApprovalBridgeAdapter::new(bridge, test_manager()))
    }

    /// The bug this composite exists for: a Panel turn has no registered
    /// channel, so the approval must be routed to the operator instead of
    /// being denied without ever prompting.
    #[tokio::test]
    async fn falls_back_to_operator_when_channel_missing() {
        use crate::exec::socket::ApprovalDecisionType;
        use crate::gateway::event_bus::GatewayEventBus;
        use crate::gateway::events::GatewayEventFrame;

        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = test_manager();
        let operator = Arc::new(OperatorApprovalRequester::new(
            manager.clone(),
            event_bus.clone(),
        ));
        let requester = FallbackApprovalRequester::new(unregistered_adapter(), operator);

        // A live subscriber = an operator surface is listening.
        let mut rx = event_bus.subscribe_typed();
        let _string_sub = event_bus.subscribe();

        let handle = tokio::spawn(async move {
            TURN_CONTEXT
                .scope(panel_turn(), async move {
                    requester
                        .request_approval(&ApprovalAction::for_tool_call(
                            "bash",
                            &serde_json::json!({"cmd": "rm -rf /tmp/x"}),
                            "needs confirmation",
                        ))
                        .await
                })
                .await
        });

        // The operator resolves the card the composite published.
        loop {
            match rx.recv().await.expect("event bus closed") {
                GatewayEventFrame::ApprovalRequested { approval_id, .. } => {
                    manager.resolve(&approval_id, ApprovalDecisionType::AllowOnce, None);
                    break;
                }
                _ => continue,
            }
        }
        assert_eq!(handle.await.unwrap().outcome, ApprovalOutcome::Approved);
    }

    /// Fail-closed: an unreachable channel never becomes an implicit approval.
    /// The composite hands the call to the operator, and the operator's refusal
    /// is what decides — the missing channel decides nothing.
    #[tokio::test]
    async fn operator_denial_is_honoured_when_channel_missing() {
        use crate::exec::socket::ApprovalDecisionType;
        use crate::gateway::event_bus::GatewayEventBus;
        use crate::gateway::events::GatewayEventFrame;

        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = test_manager();
        let operator = Arc::new(OperatorApprovalRequester::new(
            manager.clone(),
            event_bus.clone(),
        ));
        let requester = FallbackApprovalRequester::new(unregistered_adapter(), operator);
        let mut rx = event_bus.subscribe_typed();

        let handle = tokio::spawn(async move {
            TURN_CONTEXT
                .scope(panel_turn(), async move {
                    requester
                        .request_approval(&ApprovalAction::for_tool_call(
                            "bash",
                            &serde_json::json!({"cmd": "rm -rf /tmp/x"}),
                            "needs confirmation",
                        ))
                        .await
                })
                .await
        });

        loop {
            match rx.recv().await.expect("event bus closed") {
                GatewayEventFrame::ApprovalRequested { approval_id, .. } => {
                    manager.resolve(&approval_id, ApprovalDecisionType::Deny, None);
                    break;
                }
                _ => continue,
            }
        }
        assert_eq!(handle.await.unwrap().outcome, ApprovalOutcome::Denied);
    }
}
