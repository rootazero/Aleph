use std::time::Duration;

use crate::sandbox::exec_approval::gate::ApprovalOutcome;
use crate::sync_primitives::Arc;
use tokio::time::timeout;

use crate::exec::decision::ExecApprovalRequest;
use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::channel::{ChannelId, ConversationId, OutboundMessage};
use crate::gateway::channel_registry::ChannelRegistry;

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
                    "Approval delivery failed — denying"
                );
                // Retire the just-registered entry so a later session-FIFO
                // "/approve" cannot consume it.
                approval_manager.resolve(&record_id, ApprovalDecisionType::Deny, None);
                return ApprovalOutcome::Denied.into();
            }
            None => {
                tracing::warn!(
                    tool = %tool_name,
                    id = %record_id,
                    "No channel capability for approval delivery — denying"
                );
                approval_manager.resolve(&record_id, ApprovalDecisionType::Deny, None);
                return ApprovalOutcome::Denied.into();
            }
        }

        let resolved = approval_manager
            .await_registered(record_id, rx, wait_timeout)
            .await;
        let outcome = match resolved.decision {
            // "Allow once" approves this single invocation. Both "allow session"
            // and "allow always" carry the session-scoped grant so the dispatch
            // gate remembers it and stops re-prompting the same tool this
            // session: this confirm-gated tool path has no on-disk allowlist of
            // its own (persistent allowlisting lives in the shell-exec / gateway
            // path), so the strongest grant it can honor is session scope.
            // `AllowSession` is the explicit decision for that; `AllowAlways`
            // degrades to it here rather than being silently dropped.
            Some(ApprovalDecisionType::AllowOnce) => ApprovalOutcome::Approved,
            Some(ApprovalDecisionType::AllowSession) => ApprovalOutcome::ApprovedForSession,
            Some(ApprovalDecisionType::AllowAlways) => ApprovalOutcome::ApprovedForSession,
            Some(ApprovalDecisionType::Deny) => ApprovalOutcome::Denied,
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
            let text = format!(
                "⚠️ 工具 `{tool_name}` 需要你的授权。\n```\n{}\n```\n{reason}\n\n\
                 回复 /approve 批准本次、/approve session 本会话内不再询问、\
                 /deny 拒绝（可附原因：/deny 原因…，会转告给 agent）。",
                action.summary
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

        // Confirm-gated tools can honor at most a session-scoped grant (no
        // on-disk allowlist on this path), so the rendered decision set stops
        // at the session tier — offering "always" here would be a lie.
        let approval_req = crate::exec::approval::types::ApprovalRequest::Command(
            crate::exec::approval::types::CommandApprovalRequest {
                command: action.summary.clone(),
                cwd: action.cwd.clone(),
                reason: Some(reason.to_string()),
                allowed_decisions: vec![
                    ApprovalDecisionType::AllowOnce,
                    ApprovalDecisionType::AllowSession,
                    ApprovalDecisionType::Deny,
                ],
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
    async fn send_timeout_notice(&self, channel_id: &ChannelId, conversation_id: &ConversationId) {
        if let Some(channel) = self.registry.get(channel_id).await {
            let ch = channel.read().await;
            let msg = OutboundMessage::text(
                conversation_id.as_str(),
                "\u{23f1} 审批请求已超时，操作被拒绝。",
            );
            let _ = ch.send(msg).await;
        }
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
