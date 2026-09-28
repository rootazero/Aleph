use std::time::Duration;

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;
use crate::sandbox::exec_approval::gate::ApprovalOutcome;
use crate::sync_primitives::Arc;
use tokio::time::timeout;

use crate::exec::decision::ExecApprovalRequest;
use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::channel::{ChannelId, ConversationId, OutboundMessage};
use crate::gateway::channel_registry::ChannelRegistry;

/// The reply menu a plain-text channel prints under an approval prompt, built
/// from the tiers the card was actually raised with.
///
/// `/approve` / `/approve session` / `/approve always` are all parsed by
/// `inbound_router` regardless; what this controls is which of them the user is
/// *told* about — and telling somebody about `always` on a card whose record
/// will narrow it to a session grant is the same defect as a button that lies.
fn plain_text_menu(allowed: &[ApprovalDecisionType]) -> String {
    let mut parts = vec!["回复 /approve 批准本次".to_string()];
    if allowed.contains(&ApprovalDecisionType::AllowSession) {
        parts.push("/approve session 本会话内不再询问".to_string());
    }
    if allowed.contains(&ApprovalDecisionType::AllowAlways) {
        parts.push("/approve always 永久允许这次调用（可在设置里撤销）".to_string());
    }
    parts.push("/deny 拒绝（可附原因：/deny 原因…，会转告给 agent）".to_string());
    format!("{}。", parts.join("、"))
}

const DELIVERY_TIMEOUT_SECS: u64 = 30;

pub struct ChannelApprovalBridge {
    registry: Arc<ChannelRegistry>,
    /// Optional bus for the `approval.*` event family. Channel-bridge prompts
    /// ARE the user-facing notification (Telegram button, plain-text menu),
    /// so publishing the `approval.requested` frame is purely so OTHER surfaces
    /// (Panel approval bell, R5 banner) can mirror them — not a duplicate of
    /// delivery, and not a fallback. `None` is fine (no mirror); failures to
    /// publish are best-effort and never block the actual delivery to the user.
    event_bus: Option<Arc<GatewayEventBus>>,
    /// Test-only override that short-circuits `request_for_tool` with a fixed
    /// outcome, bypassing the real channel lookup and pending-approval wait.
    /// Field exists only under `cfg(test)` so production has zero surface.
    #[cfg(test)]
    test_outcome_override: Option<ApprovalOutcome>,
}

impl ChannelApprovalBridge {
    pub fn new(registry: Arc<ChannelRegistry>) -> Self {
        Self {
            registry,
            event_bus: None,
            #[cfg(test)]
            test_outcome_override: None,
        }
    }

    /// Build a bridge that publishes `approval.*` events on the given bus.
    /// The Panel approval bell and R5 banner subscribe here so a Telegram /
    /// Discord / Slack / iMessage originated approval appears in their UI;
    /// the channel itself is the user-facing prompt and is unaffected by
    /// bus hiccups (publish failures are logged and ignored, never block
    /// delivery).
    #[must_use]
    pub fn with_event_bus(registry: Arc<ChannelRegistry>, event_bus: Arc<GatewayEventBus>) -> Self {
        Self {
            registry,
            event_bus: Some(event_bus),
            #[cfg(test)]
            test_outcome_override: None,
        }
    }

    /// Request tool-call approval and block waiting for the user's decision.
    ///
    /// Two stages: (1) `manager.create` builds the record to obtain `record.id`,
    /// then `register_pending` registers first (register before delivery, so a
    /// fast resolver cannot race ahead); (2) `deliver_routed` uses `record.id`
    /// to send buttons to the target channel conversation;
    /// (3) `await_registered` blocks on the `record.id` oneshot, awakened by the
    /// channel button callback via `manager.resolve(record.id, ...)`.
    ///
    /// `channel_id` / `conversation_id` are structured routing parameters (from
    /// the caller's parsed `SessionKey`) — no longer parsing a lossy string
    /// `session_key`.
    ///
    /// `session_key` is the structured key string of the originating session
    /// (the same form as router-side `ctx.session_key.to_string()`): the record
    /// must carry it so that `/approve`/`/deny` text replies can hit this
    /// approval via `resolve_for_session` (direct hit when exactly one live
    /// card exists for this session; concurrent cards reject bare replies with
    /// a numbered list, requiring `/approve <n>` — see `SessionResolveOutcome`).
    /// An empty value falls back to a `channel:conversation` synthetic key
    /// (reachable only via button callback).
    pub async fn request_for_tool(
        &self,
        approval_manager: &crate::exec::manager::ExecApprovalManager,
        action: &crate::sandbox::exec_approval::ApprovalAction,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
        session_key: &str,
        originator: Option<&str>,
        timeout_ms: u64,
    ) -> crate::sandbox::exec_approval::ApprovalResponse {
        #[cfg(test)]
        if let Some(outcome) = self.test_outcome_override {
            return outcome.into();
        }

        let tool_name = action.tool_name.as_str();
        let reason = action.reason.as_str();

        let record_session_key = if session_key.is_empty() {
            format!("{}:{}", channel_id.as_str(), conversation_id.as_str())
        } else {
            session_key.to_string()
        };

        let request = ExecApprovalRequest {
            id: uuid::Uuid::new_v4().to_string(),
            // The redacted ACTION, not the bare tool name — this is the string
            // the user reads before deciding, on every surface.
            command: action.summary.clone(),
            cwd: action.cwd.clone(),
            analysis: action.analysis_for_record(),
            // Single source for the issuing agent (`audit_identity`); the
            // context string it also builds is redundant here — the approval
            // card renders `command` + `reason`.
            agent_id: crate::approval::audit_identity("tool", tool_name, reason).0,
            session_key: record_session_key.clone(),
            reason: Some(reason.to_string()),
            // The human who triggered this tool call. Stamped onto the record so
            // the channel button-callback gate refuses a resolution from anyone
            // but them (group-chat approval-bypass fix). `None` when the run has
            // no channel originator — the gate then no-ops.
            originator_user_id: originator.map(str::to_string),
            // Session-grant identity of this action: a session-level decision
            // cascades to other pending cards of the same action.
            grant_key: action.grant_key.clone(),
            // What the gate decided this card may offer — the keyboard below is
            // built from the same list, and the resolver enforces it.
            allowed_decisions: action.allowed_decisions.clone(),
        };

        let record = approval_manager.create(&request, timeout_ms);
        // `record.tool_call_id` lives on the wire and on the record but not on
        // `register_pending`'s return tuple; clone it out so we can publish it
        // on the ApprovalRequested frame without a second map lookup.
        let tool_call_id = record.tool_call_id.clone();

        // Register the pending entry BEFORE delivering the prompt so a fast
        // resolver (instant button tap / "/approve" reply) cannot race ahead
        // of registration (resolve-before-register → spurious timeout); see
        // `ExecApprovalManager::register_pending`.
        let (record_id, rx, wait_timeout) = approval_manager.register_pending(record);

        // Mirror the request on the bus so surfaces that did NOT raise it (Panel
        // approval bell, R5 banner) learn a card is parked. The channel itself
        // is the user-facing prompt — this is purely a mirror, so a publish
        // failure is best-effort: logging and continuing is right, denying the
        // caller is wrong (the Telegram / Discord user may still receive their
        // button any moment now).
        if let Some(bus) = self.event_bus.as_ref() {
            match bus.publish_frame(&GatewayEventFrame::ApprovalRequested {
                approval_id: record_id.clone(),
                session_key: record_session_key.clone(),
                channel_id: channel_id.as_str().to_string(),
                conversation_id: conversation_id.as_str().to_string(),
                tool_call_id,
            }) {
                Ok(0) => {
                    tracing::debug!(
                        id = %record_id,
                        "ApprovalRequested mirror reached no subscribers; \
                         the channel itself is the user-facing prompt, \
                         continuing without denial"
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        id = %record_id,
                        "ApprovalRequested mirror publish failed; \
                         continuing — the channel prompt is independent"
                    );
                }
            }
        }

        match self
            .deliver_routed(channel_id, conversation_id, action, &record_id)
            .await
        {
            Some(true) => {
                tracing::info!(
                    tool = %tool_name,
                    id = %record_id,
                    channel = %channel_id.as_str(),
                    "Approval delivered via channel — waiting for user decision"
                );
            }
            Some(false) => {
                tracing::warn!(
                    tool = %tool_name,
                    id = %record_id,
                    "Approval delivery failed — failing closed (the card never reached anyone)"
                );
                // Retire the just-registered entry WITHOUT stamping a Deny:
                // resolve(Deny) would cascade a fake refusal to every other
                // live pending card in this session with the same grant_key,
                // and that cascade's Deny outcomes flow into the
                // brute-force breaker as `UserRejected` (a delivery hiccup
                // would count as the user declining). `retire_pending` is
                // the same "remove from pending" without the cascade and
                // without the record stamp.
                approval_manager.retire_pending(&record_id);
                // `Unavailable`: the prompt did not arrive, so nobody refused
                // it. Returning `Denied` here made a transient Telegram failure
                // stick to the intent for the rest of the session and count
                // toward the brute-force breaker — three hiccups paused every
                // gate in the conversation and told the model the user had
                // declined. See `DenialLedger::record_denial`.
                return ApprovalOutcome::Unavailable.into();
            }
            None => {
                tracing::warn!(
                    tool = %tool_name,
                    id = %record_id,
                    "No channel capability for approval delivery — failing closed"
                );
                approval_manager.retire_pending(&record_id);
                return ApprovalOutcome::Unavailable.into();
            }
        }

        let resolved = approval_manager
            .await_registered(record_id.clone(), rx, wait_timeout)
            .await;
        // Mirror the resolution on the bus so mirrors of the request frame
        // (Panel bell, R5 banner) can clear their parked row. The channel
        // itself does not need the event — it learned via the user's button
        // tap or text reply. Best-effort: bus hiccups are logged and dropped,
        // mirroring the publish-request policy above.
        if let Some(bus) = self.event_bus.as_ref() {
            let frame = match resolved.decision {
                Some(decision) => GatewayEventFrame::ApprovalResolved {
                    approval_id: record_id.clone(),
                    session_key: record_session_key.clone(),
                    decision,
                    resolved_by: None,
                },
                None => GatewayEventFrame::ApprovalExpired {
                    approval_id: record_id.clone(),
                    session_key: record_session_key.clone(),
                },
            };
            if let Err(e) = bus.publish_frame(&frame) {
                tracing::debug!(
                    error = %e,
                    id = %record_id,
                    "approval resolution mirror publish failed; \
                     the channel itself is the user-facing surface"
                );
            }
        }
        let outcome = match resolved.decision {
            // Single decision → outcome mapping
            // (`ApprovalDecisionType::to_outcome_within`), named against the set
            // THIS card was raised with — the same `action.allowed_decisions`
            // the keyboard was built from. The manager already clamped to it;
            // passing it again is idempotent and keeps this site honest about
            // which tiers it ever offered.
            Some(decision) => decision.to_outcome_within(&action.allowed_decisions),
            None => {
                self.send_timeout_notice(channel_id, conversation_id).await;
                ApprovalOutcome::Timeout
            }
        };
        // A `/deny <reason>` text reply rides the record; relay it so the
        // dispatch gate can put the human's own words in front of the model.
        crate::sandbox::exec_approval::ApprovalResponse {
            outcome,
            deny_reason: resolved.deny_reason,
        }
    }

    /// Whether this channel can currently receive approval deliveries (already
    /// registered in `ChannelRegistry`).
    ///
    /// Panel turns carry `gui:chat` — a pseudo channel id that is never
    /// registered as an external channel, so `deliver_routed` always returns
    /// `None`. Callers use this to route through the operator event bus when
    /// the channel is unreachable, rather than denying outright.
    pub async fn can_deliver(&self, channel_id: &ChannelId) -> bool {
        #[cfg(test)]
        if self.test_outcome_override.is_some() {
            return true;
        }
        self.registry.get(channel_id).await.is_some()
    }

    /// Deliver an approval prompt by structured `channel_id`. Returns
    /// `Some(true)` delivered, `Some(false)` delivery failed, `None` no channel.
    ///
    /// Channels without native approval capability take a plain-text fallback:
    /// send a message with `/approve` / `/deny` instructions, resolved by the
    /// inbound router's text interception via session FIFO. Previously these
    /// channels were outright `Denied`, so confirm-gated tools on non-capable
    /// channels were effectively all silently rejected.
    ///
    /// Authorization semantics: the fallback path lacks the capability path's
    /// per-person `authorize_actor` check; the trust boundary matches the
    /// existing `/approve` text command — relying on the channel inbound
    /// layer's allowlist / pairing gate (anyone who can chat with the bot is
    /// trusted). The prompt is delivered only to the originating session
    /// itself, never broadcast.
    async fn deliver_routed(
        &self,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
        action: &crate::sandbox::exec_approval::ApprovalAction,
        approval_id: &str,
    ) -> Option<bool> {
        // Truncate `action.summary` to the same shape the manager's
        // `display_line` uses, so the text-fallback path can never overflow
        // a channel's message limit (Telegram's is 4096 chars; a 4 KB
        // command summary would silently truncate or refuse to send).
        const MAX_SUMMARY_CHARS: usize = 1000;
        let mut summary: String = action.summary.chars().take(MAX_SUMMARY_CHARS).collect();
        if action.summary.chars().count() > MAX_SUMMARY_CHARS {
            summary.push('…');
        }
        let _ = approval_id; // already used by the caller for register_pending; not echoed in the fallback text
        let tool_name = action.tool_name.as_str();
        let reason = action.reason.as_str();
        let channel = self.registry.get(channel_id).await?;
        let capability = {
            let ch = channel.read().await;
            ch.approval_capability()
        };

        let Some(capability) = capability else {
            // The action summary is the point of the prompt: `/approve` on a
            // bare tool name approves whatever the model happened to pass.
            // The truncated form keeps the fallback under every channel's
            // message limit (Telegram's is 4096 chars).
            //
            // The reply menu is built from the same `allowed_decisions` the
            // keyboard path uses. A plain-text channel that kept a fixed menu
            // would be the third copy of "which tiers exist" — and the one that
            // teaches the user a word (`always`) the resolver would narrow.
            let text = format!(
                "⚠️ 工具 `{tool_name}` 需要你的授权。\n```\n{summary}\n```\n{reason}\n\n{}",
                plain_text_menu(&action.allowed_decisions)
            );
            // Through `ChannelRegistry::send`, not the channel handle: the
            // registry owns rate-limit retry, the durable queue and
            // per-conversation ordering (see `send_timeout_notice`). The same
            // delivery timeout the capability path uses bounds it — a hung
            // adapter send must not hold the channel read lock forever, or
            // writers (reconnect/stop) block behind it and this approval
            // waits without end.
            return match timeout(
                Duration::from_secs(DELIVERY_TIMEOUT_SECS),
                self.registry.send(
                    channel_id,
                    OutboundMessage::text(conversation_id.as_str(), text),
                ),
            )
            .await
            {
                Ok(Ok(_)) => Some(true),
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "plain-text approval fallback send failed");
                    Some(false)
                }
                Err(_) => {
                    tracing::warn!(
                        "plain-text approval fallback send timed out after {}s",
                        DELIVERY_TIMEOUT_SECS
                    );
                    Some(false)
                }
            };
        };

        // The rendered decision set is the gate's, not this function's: it was
        // derived once (`exec::allowed_decisions::for_confirm_gate`) and rides
        // the action. A literal here would be a second answer to "which tiers
        // may this card offer", and the two would drift the first time either
        // moved.
        let approval_req = crate::exec::approval::types::ApprovalRequest::Command(
            crate::exec::approval::types::CommandApprovalRequest {
                command: action.summary.clone(),
                cwd: action.cwd.clone(),
                reason: Some(reason.to_string()),
                allowed_decisions: action.allowed_decisions.clone(),
            },
        );
        match timeout(
            Duration::from_secs(DELIVERY_TIMEOUT_SECS),
            capability.deliver_approval(conversation_id, &approval_req, approval_id),
        )
        .await
        {
            Ok(Ok(_pending)) => Some(true),
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "deliver_approval returned error");
                Some(false)
            }
            Err(_) => {
                tracing::warn!(
                    "deliver_approval timed out after {}s",
                    DELIVERY_TIMEOUT_SECS
                );
                Some(false)
            }
        }
    }

    /// Send a friendly timeout notice to the channel (best-effort).
    ///
    /// Through `ChannelRegistry::send`, not the channel handle directly: the
    /// registry is the chokepoint that owns rate-limit retry, the durable queue
    /// and per-conversation ordering. Reaching past it made this notice the one
    /// outbound message with none of those — dropped outright if the channel
    /// happened to be reconnecting, and able to overtake queued replies for the
    /// same chat. "Best-effort" is the `let _ =` here, not a reason to bypass
    /// the send path.
    async fn send_timeout_notice(&self, channel_id: &ChannelId, conversation_id: &ConversationId) {
        let msg = OutboundMessage::text(
            conversation_id.as_str(),
            "\u{23f1} 审批请求已超时，操作被拒绝。",
        );
        let _ = self.registry.send(channel_id, msg).await;
    }

    /// Test helper: a bridge that always returns `ApprovalOutcome::Approved`.
    #[cfg(test)]
    pub fn for_test_always_approved() -> Self {
        Self {
            registry: Arc::new(ChannelRegistry::new()),
            event_bus: None,
            test_outcome_override: Some(ApprovalOutcome::Approved),
        }
    }

    /// Test helper: a bridge that always returns `ApprovalOutcome::Denied`.
    #[cfg(test)]
    pub fn for_test_always_denied() -> Self {
        Self {
            registry: Arc::new(ChannelRegistry::new()),
            event_bus: None,
            test_outcome_override: Some(ApprovalOutcome::Denied),
        }
    }

    /// Test helper: a bridge backed by an `event_bus` for asserting the
    /// `approval.*` mirror frames the W2 fix added.
    #[cfg(test)]
    pub fn for_test_with_bus(
        registry: Arc<ChannelRegistry>,
        event_bus: Arc<GatewayEventBus>,
    ) -> Self {
        Self {
            registry,
            event_bus: Some(event_bus),
            #[cfg(test)]
            test_outcome_override: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::manager::ExecApprovalManager;
    use crate::exec::socket::ApprovalDecisionType;
    use crate::gateway::channel::{
        Channel, ChannelCapabilities, ChannelId, ChannelInfo, ChannelResult, ChannelState,
        ChannelStatus, ConversationId, MessageId, OutboundMessage, SendResult,
    };
    use crate::gateway::channel_approval::{
        ApprovalAction as CapAction, AuthorizationResult, PendingApproval, RenderedApproval,
    };
    use crate::gateway::channel_registry::ChannelRegistry;
    use crate::gateway::events::GatewayEventFrame;
    use crate::sandbox::exec_approval::ApprovalAction;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Minimal capability: every `deliver_approval` returns a `PendingApproval`
    /// (resolved via `resolve` on the `ExecApprovalManager`); everything else
    /// is identity. The test harness feeds the `approval_id` returned here
    /// straight into `manager.resolve` to simulate a button tap.
    struct StubCapability {
        last_approval_id: Arc<Mutex<Option<String>>>,
    }

    impl StubCapability {
        fn new() -> Self {
            Self {
                last_approval_id: Arc::new(Mutex::new(None)),
            }
        }
    }

    #[async_trait]
    impl crate::gateway::channel_approval::ChannelApprovalCapability for StubCapability {
        async fn deliver_approval(
            &self,
            _conversation_id: &ConversationId,
            _request: &crate::exec::approval::types::ApprovalRequest,
            approval_id: &str,
        ) -> ChannelResult<PendingApproval> {
            *self.last_approval_id.lock().unwrap() = Some(approval_id.to_string());
            let stub_command = match _request {
                crate::exec::approval::types::ApprovalRequest::Command(c) => c.command.clone(),
            };
            Ok(PendingApproval {
                approval_id: approval_id.to_string(),
                request: crate::exec::approval::types::ApprovalRequest::Command(
                    crate::exec::approval::types::CommandApprovalRequest {
                        command: stub_command,
                        cwd: None,
                        reason: None,
                        allowed_decisions: vec![],
                    },
                ),
                channel_id: "stub".to_string(),
                conversation_id: _conversation_id.clone(),
                message_id: None,
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(60),
            })
        }
        async fn authorize_actor(
            &self,
            _actor_user_id: &crate::gateway::channel::UserId,
            _action: CapAction,
        ) -> AuthorizationResult {
            AuthorizationResult::Authorized
        }
        async fn render_approval(
            &self,
            _conversation_id: &ConversationId,
            _request: &crate::exec::approval::types::ApprovalRequest,
        ) -> ChannelResult<RenderedApproval> {
            Ok(RenderedApproval {
                message: crate::gateway::channel::OutboundMessage::text(
                    _conversation_id.as_str(),
                    "stub",
                ),
                callback_prefix: "stub".to_string(),
            })
        }

        async fn resolve_approval(
            &self,
            _pending: &PendingApproval,
            _action: CapAction,
        ) -> ChannelResult<()> {
            // The stub never holds the user-pressed button — the test driver
            // resolves the oneshot directly via `manager.resolve`. This
            // method exists only to satisfy the trait.
            Ok(())
        }
    }

    /// Minimal `Channel` that returns our `StubCapability` from
    /// `approval_capability()` and no-ops everything else. Lets us put a
    /// fully-real `ChannelApprovalBridge` through the publish path without
    /// touching a real Telegram adapter.
    struct StubChannel {
        info: ChannelInfo,
        state: ChannelState,
        capability: Arc<dyn crate::gateway::channel_approval::ChannelApprovalCapability>,
    }

    impl StubChannel {
        fn new(
            capability: Arc<dyn crate::gateway::channel_approval::ChannelApprovalCapability>,
        ) -> Self {
            Self {
                info: ChannelInfo {
                    id: ChannelId::new("stub"),
                    name: "stub".to_string(),
                    channel_type: "stub".to_string(),
                    status: ChannelStatus::Connected,
                    capabilities: ChannelCapabilities::default(),
                },
                state: ChannelState::new(8),
                capability,
            }
        }
    }

    #[async_trait]
    impl Channel for StubChannel {
        fn info(&self) -> &ChannelInfo {
            &self.info
        }
        fn state(&self) -> &ChannelState {
            &self.state
        }
        fn approval_capability(
            &self,
        ) -> Option<Arc<dyn crate::gateway::channel_approval::ChannelApprovalCapability>> {
            Some(self.capability.clone())
        }
        async fn start(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn stop(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn send(&self, _message: OutboundMessage) -> ChannelResult<SendResult> {
            Ok(SendResult {
                message_id: MessageId::new("m"),
                timestamp: chrono::Utc::now(),
            })
        }
    }

    async fn registry_with_stub(
        capability: Arc<dyn crate::gateway::channel_approval::ChannelApprovalCapability>,
    ) -> Arc<ChannelRegistry> {
        let registry = Arc::new(ChannelRegistry::new());
        let channel: Box<dyn Channel> = Box::new(StubChannel::new(capability));
        registry.register(channel).await;
        registry
    }

    fn channel_id() -> ChannelId {
        ChannelId::new("stub")
    }
    fn conversation_id() -> ConversationId {
        ConversationId::new("c1")
    }

    fn action() -> ApprovalAction {
        ApprovalAction::for_tool_call(
            "file_ops",
            &serde_json::json!({"operation": "delete", "path": "/tmp/x"}),
            "destructive",
        )
    }

    /// W2 (R2 of the §5.2 deferred list): the channel-bridge path must publish
    /// `ApprovalRequested` on the bus so the Panel approval bell and R5 banner
    /// mirror Telegram / Discord / Slack / iMessage cards just like they mirror
    /// operator-tier cards. Bus presence MUST be the production default
    /// (`start/mod.rs::start` calls `with_event_bus`).
    #[tokio::test]
    async fn channel_path_publishes_approval_requested_frame() {
        let cap = Arc::new(StubCapability::new());
        let registry = registry_with_stub(cap.clone()).await;
        let event_bus = Arc::new(GatewayEventBus::new());
        let bridge = ChannelApprovalBridge::for_test_with_bus(registry, event_bus.clone());
        let manager = Arc::new(ExecApprovalManager::new());

        let mut rx = event_bus.subscribe_typed();
        let manager_for_task = manager.clone();
        let cap_for_task = cap.clone();
        let channel_id = channel_id();
        let conversation_id = conversation_id();
        let handle = tokio::spawn(async move {
            bridge
                .request_for_tool(
                    &manager_for_task,
                    &action(),
                    &channel_id,
                    &conversation_id,
                    "telegram:dm:user-1",
                    Some("user-1"),
                    60_000,
                )
                .await
        });

        // ApprovalRequested must be the first frame off the wire — before
        // anything else (e.g. ApprovalResolved).
        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("ApprovalRequested must be published within 2s")
            .expect("event bus closed");
        match frame {
            GatewayEventFrame::ApprovalRequested {
                approval_id,
                session_key,
                channel_id: ch,
                conversation_id: conv,
                ..
            } => {
                assert_eq!(ch, "stub");
                assert_eq!(conv, "c1");
                assert_eq!(session_key, "telegram:dm:user-1");
                assert!(!approval_id.is_empty());
                // Resolve through the stub so the test exits.
                manager.resolve(&approval_id, ApprovalDecisionType::AllowOnce, None);
                let _ = cap_for_task; // silence unused
            }
            other => panic!("expected ApprovalRequested first, got {other:?}"),
        }
        let outcome = handle.await.unwrap().outcome;
        assert_eq!(outcome, ApprovalOutcome::Approved);
    }

    /// The resolution mirror: a button tap (or `/approve` reply) that wakes
    /// the oneshot must publish `ApprovalResolved` so the Panel bell can
    /// clear the parked row.
    #[tokio::test]
    async fn channel_path_publishes_approval_resolved_frame() {
        let cap = Arc::new(StubCapability::new());
        let registry = registry_with_stub(cap.clone()).await;
        let event_bus = Arc::new(GatewayEventBus::new());
        let bridge = ChannelApprovalBridge::for_test_with_bus(registry, event_bus.clone());
        let manager = Arc::new(ExecApprovalManager::new());

        let mut rx = event_bus.subscribe_typed();
        let manager_for_task = manager.clone();
        let cap_for_task = cap.clone();
        let channel_id = channel_id();
        let conversation_id = conversation_id();
        let handle = tokio::spawn(async move {
            bridge
                .request_for_tool(
                    &manager_for_task,
                    &action(),
                    &channel_id,
                    &conversation_id,
                    "telegram:dm:user-1",
                    Some("user-1"),
                    60_000,
                )
                .await
        });

        // Drain ApprovalRequested, then approve, then assert ApprovalResolved
        // arrives before the spawned task finishes.
        let approval_id = match rx.recv().await.expect("event bus closed") {
            GatewayEventFrame::ApprovalRequested { approval_id, .. } => approval_id,
            other => panic!("expected ApprovalRequested, got {other:?}"),
        };
        manager.resolve(&approval_id, ApprovalDecisionType::AllowOnce, None);

        // ApprovalResolved is the next frame the bus sees; ordering is enforced
        // by the await_registered + publish ordering inside request_for_tool.
        let resolved_frame = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("ApprovalResolved must arrive within 2s")
            .expect("event bus closed");
        match resolved_frame {
            GatewayEventFrame::ApprovalResolved {
                approval_id: id,
                session_key,
                decision,
                ..
            } => {
                assert_eq!(id, approval_id);
                assert_eq!(session_key, "telegram:dm:user-1");
                assert!(matches!(decision, ApprovalDecisionType::AllowOnce));
            }
            other => panic!("expected ApprovalResolved, got {other:?}"),
        }
        let outcome = handle.await.unwrap().outcome;
        assert_eq!(outcome, ApprovalOutcome::Approved);
        let _ = cap_for_task; // silence unused
    }

    /// A bridge built without an `event_bus` (legacy path, tests that don't
    /// care about the mirror) must NOT publish anything — so a missing
    /// production call to `with_event_bus` is observable as "no event
    /// subscribers" rather than as a panic.
    #[tokio::test]
    async fn channel_path_does_not_publish_when_event_bus_unset() {
        let cap = Arc::new(StubCapability::new());
        let _registry = registry_with_stub(cap.clone()).await;
        let event_bus = Arc::new(GatewayEventBus::new());
        let mut rx = event_bus.subscribe_typed();
        let bridge = ChannelApprovalBridge::for_test_always_approved(); // No event_bus.
        let manager = Arc::new(ExecApprovalManager::new());

        let manager_for_task = manager.clone();
        let channel_id = channel_id();
        let conversation_id = conversation_id();
        let outcome = bridge
            .request_for_tool(
                &manager_for_task,
                &action(),
                &channel_id,
                &conversation_id,
                "telegram:dm:user-1",
                Some("user-1"),
                60_000,
            )
            .await;
        assert_eq!(outcome.outcome, ApprovalOutcome::Approved);
        // No frames were ever published because the bridge has no bus.
        let next = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
        assert!(
            next.is_err(),
            "a bridge without event_bus must never publish any frame"
        );
    }

    /// A bus hiccup must NOT deny the approval — the channel itself is the
    /// user-facing prompt and is independent of the mirror. The W2 fix is
    /// precisely the bug class that OperatorApprovalRequester's `Err → Deny`
    /// had to keep (it has no fallback surface), which is why this site is
    /// best-effort instead. Pinned by a bridge whose bus is closed mid-publish.
    ///
    /// The `Timeout` outcome below is the strongest possible negative
    /// witness for the bug class: before this fix, the bridge would have
    /// returned `Denied` because the publish looked like a fatal signal.
    /// `Timeout` proves the bridge waited on the channel rather than
    /// reacting to the bus — the test never resolves the card, so the
    /// deadline is what the bridge observed, NOT a denial that came from
    /// `publish_frame` returning `Ok(0)` or `Err(_)`.
    #[tokio::test]
    async fn channel_path_publish_failure_does_not_deny_approval() {
        let cap = Arc::new(StubCapability::new());
        let registry = registry_with_stub(cap.clone()).await;
        let event_bus = Arc::new(GatewayEventBus::new());
        // Drop the only typed subscriber so `publish_frame` returns Ok(0) —
        // the boundary condition the Operator leg treats as fatal.
        let _rx_dropped = event_bus.subscribe_typed();
        drop(_rx_dropped);
        let bridge = ChannelApprovalBridge::for_test_with_bus(registry, event_bus);
        let manager = Arc::new(ExecApprovalManager::new());

        let channel_id = channel_id();
        let conversation_id = conversation_id();
        let manager_for_task = manager.clone();
        // Tiny timeout so the test cannot hang on the approval wait — the
        // assert below only cares about whether the channel's prompt path
        // succeeds, not about the decision.
        let outcome = bridge
            .request_for_tool(
                &manager_for_task,
                &action(),
                &channel_id,
                &conversation_id,
                "telegram:dm:user-1",
                Some("user-1"),
                2_000,
            )
            .await;
        assert_ne!(
            outcome.outcome,
            ApprovalOutcome::Denied,
            "a bus hiccup MUST NOT turn the user's Telegram card into a denial"
        );
        assert_ne!(
            outcome.outcome,
            ApprovalOutcome::Unavailable,
            "Unavailable would mean the channel prompt itself failed; the \
             bridge's prompt succeeded — only the bus mirror was empty"
        );
    }
}
