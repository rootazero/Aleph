use std::time::Duration;

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
            session_key: record_session_key,
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

        // Register the pending entry BEFORE delivering the prompt so a fast
        // resolver (instant button tap / "/approve" reply) cannot race ahead
        // of registration (resolve-before-register → spurious timeout); see
        // `ExecApprovalManager::register_pending`.
        let (record_id, rx, wait_timeout) = approval_manager.register_pending(record);

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
                // Retire the just-registered entry so a later session-FIFO
                // "/approve" cannot consume it.
                approval_manager.resolve(&record_id, ApprovalDecisionType::Deny, None);
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
                approval_manager.resolve(&record_id, ApprovalDecisionType::Deny, None);
                return ApprovalOutcome::Unavailable.into();
            }
        }

        let resolved = approval_manager
            .await_registered(record_id, rx, wait_timeout)
            .await;
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
            let ch = channel.read().await;
            return match ch
                .send(OutboundMessage::text(conversation_id.as_str(), text))
                .await
            {
                Ok(_) => Some(true),
                Err(e) => {
                    tracing::warn!(error = %e, "plain-text approval fallback send failed");
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
            test_outcome_override: Some(ApprovalOutcome::Approved),
        }
    }

    /// Test helper: a bridge that always returns `ApprovalOutcome::Denied`.
    #[cfg(test)]
    pub fn for_test_always_denied() -> Self {
        Self {
            registry: Arc::new(ChannelRegistry::new()),
            test_outcome_override: Some(ApprovalOutcome::Denied),
        }
    }
}
