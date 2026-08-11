//! `OperatorApprovalRequester` — an [`ApprovalRequester`] that routes a config
//! tool approval to the SERVER OPERATOR (not the requesting chat-tier device).
//!
//! Unlike `ChannelApprovalBridgeAdapter` (which delivers back to the
//! requester's own channel), this registers a pending approval in the shared
//! [`ExecApprovalManager`] and publishes a `GatewayEventFrame::Approval*` event.
//! The operator resolves it via the existing `exec.approval.resolve` RPC, waking
//! the oneshot. Used by the config-tier gate in `ScopedToolService` (Phase 2b
//! sudo).
//!
//! ## Two instances, because this type serves two opposite audiences
//!
//! The `approval.` topic family used to be admin-gated wholesale, so "operator
//! tier" was a property of the transport. That prefix rule is gone (see
//! `src/gateway/CLAUDE.md` 地雷 K: a member MUST receive the card for their own
//! parked tool call, or their run dies on the 120 s timeout and the documented
//! workaround is `exec_tier:"full"` — the least safe tier becoming the only
//! usable one). The judgement now lives in
//! [`crate::gateway::event_visibility::session_identity_of`], which reads the
//! frame's `session_key`: non-empty ⇒ owner-or-admin, empty ⇒ operator-only.
//!
//! That is the right judgement for the frames this type publishes on behalf of
//! [`crate::approval::adapters::FallbackApprovalRequester`] — a Panel turn whose
//! own `Ask`-tier tool parked, where the requester IS the audience. It is the
//! **wrong** judgement for the config gate, whose entire premise is that the
//! requester may not decide: `check_operator_gate` parks the call precisely
//! because `role_is_operator` said no, and then this requester used to address
//! the resulting card to that same member's session — which both the event plane
//! and `exec.approvals.pending` / `exec.approval.resolve` (carved open to members)
//! read as "yours". A member could raise the operator gate and answer it,
//! reaching every `OPERATOR_TOOLS` entry; `loop_graph` is the sharpest instance
//! because a `root:` body is injected verbatim into every governed session's
//! system prompt, on every turn, persisted.
//!
//! So: [`OperatorApprovalRequester::new`] keeps the owner-scoped shape, and
//! [`OperatorApprovalRequester::for_config_tier`] addresses the card to the
//! operator. The split is per-instance rather than per-topic on purpose — a
//! prefix rule cannot tell the two apart, which is what 地雷 K is about.
//!
//! Note the asymmetry between the frame and the record: the **frame** carries an
//! empty `session_key` (the existing encoding for "no owner to compare against",
//! shared with cluster-node approvals), while the **record** keeps the real one.
//! Blanking the record too would look tidier and would be a bug: the manager
//! matches `record.session_key` when cascading a session grant and when cleaning
//! up a session's pending entries, so blank records would cascade decisions
//! across unrelated users. The record therefore carries the fact explicitly, in
//! [`ExecApprovalRecord::operator_only`].
//!
//! Scope (Phase 2b): `AllowOnce` + `AllowSession` only. `AllowAlways` collapses to a
//! session grant — permanent device elevation is Phase 3 (the narrowing lives in
//! [`ApprovalDecisionType::clamped`], shared by every surface).

use async_trait::async_trait;

use crate::exec::decision::ApprovalRequest;
use crate::exec::manager::{ExecApprovalManager, DEFAULT_APPROVAL_TIMEOUT_MS};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester, ApprovalResponse};
use crate::sandbox::exec_approval::ApprovalAction;
use crate::sync_primitives::Arc;

pub struct OperatorApprovalRequester {
    manager: Arc<ExecApprovalManager>,
    event_bus: Arc<GatewayEventBus>,
    /// See the module doc. `false` = the card belongs to the session that
    /// raised it (the `FallbackApprovalRequester` leg); `true` = the card is an
    /// operator-tier escalation and the raiser must not be able to answer it.
    operator_only: bool,
}

impl OperatorApprovalRequester {
    /// Owner-scoped: the card is delivered to, and resolvable by, the session
    /// that raised it (plus any admin). This is the leg
    /// [`crate::approval::adapters::FallbackApprovalRequester`] falls back to
    /// when the requester's own channel cannot be reached — the Panel's
    /// `gui:chat` is never registered, so every Panel `Ask`-tier card lands
    /// here and MUST stay visible to whoever is sitting in front of it.
    #[must_use]
    pub const fn new(manager: Arc<ExecApprovalManager>, event_bus: Arc<GatewayEventBus>) -> Self {
        Self {
            manager,
            event_bus,
            operator_only: false,
        }
    }

    /// Operator-scoped: for the config-tier gate in `ScopedToolService`, which
    /// parks a call *because* the caller may not make this decision. Addressing
    /// such a card to the caller's own session hands them the decision back.
    #[must_use]
    pub const fn for_config_tier(
        manager: Arc<ExecApprovalManager>,
        event_bus: Arc<GatewayEventBus>,
    ) -> Self {
        Self {
            manager,
            event_bus,
            operator_only: true,
        }
    }

    /// The key to publish on the `Approval*` frames for this instance.
    ///
    /// Empty for a config-tier escalation: `session_identity_of` reads an empty
    /// key as [`crate::gateway::event_visibility::SessionIdentity::OperatorOnly`],
    /// the same classification a cluster-node approval already gets, so no new
    /// wire field and no new arm are needed.
    fn frame_session_key(&self, session_key: &str) -> String {
        if self.operator_only {
            String::new()
        } else {
            session_key.to_string()
        }
    }
}

#[async_trait]
impl ApprovalRequester for OperatorApprovalRequester {
    async fn request_approval(&self, action: &ApprovalAction) -> ApprovalResponse {
        let tool_name = action.tool_name.as_str();
        let reason = action.reason.as_str();
        let turn = crate::tools::turn_context::current_turn_context();
        let (session_key_str, agent_id, channel_id, conversation_id) = match &turn {
            Some(t) => (
                t.session_key.to_key_string(),
                t.session_key.agent_id().to_string(),
                t.channel_id.clone(),
                t.conversation_id.clone(),
            ),
            None => (String::new(), String::new(), String::new(), String::new()),
        };

        let request = ApprovalRequest {
            id: uuid::Uuid::new_v4().to_string(),
            // The ACTION, not the tool name: the Panel card renders `command`
            // verbatim, so a bare name is an operator deciding blind.
            command: action.summary.clone(),
            cwd: action.cwd.clone(),
            analysis: action.analysis_for_record(),
            agent_id,
            session_key: session_key_str.clone(),
            // Carried on the pending record so the operator's resolving
            // surface (panel pending list) can show WHY the tool is gated,
            // not just what it does.
            reason: (!reason.is_empty()).then(|| reason.to_string()),
            // Operator/Panel approvals resolve via the `exec.approval.resolve`
            // RPC, not a channel button, so the originator gate never applies.
            originator_user_id: None,
            // Session-grant identity of this action: a session-level decision
            // cascades to other pending cards of the same action.
            grant_key: action.grant_key.clone(),
            // What the gate decided this card may offer. Carried onto the
            // record, where the resolve RPC enforces it.
            allowed_decisions: action.allowed_decisions.clone(),
        };
        // Kept for the outcome mapping below: `request` is moved into `create`.
        let allowed_decisions = action.allowed_decisions.clone();
        let mut record = self.manager.create(&request, DEFAULT_APPROVAL_TIMEOUT_MS);
        // The RPC leg of the same fact the blank frame key carries. Stamped on
        // the record (not derived from its `session_key`, which stays real for
        // the grant cascade) so `exec.approvals.pending` and
        // `exec.approval.resolve` — both carved open to members — refuse it.
        record.operator_only = self.operator_only;
        // Pairing key for the client: which tool row this card belongs under.
        let tool_call_id = record.tool_call_id.clone();
        let frame_session_key = self.frame_session_key(&session_key_str);
        // Register the pending entry BEFORE publishing the event, so an operator
        // who resolves the instant they see it cannot race ahead of
        // registration (resolve-before-register would otherwise be lost and the
        // approval would spuriously time out). The entry is resolvable the
        // moment `register_pending` returns.
        let (approval_id, rx, timeout) = self.manager.register_pending(record);

        if let Err(e) = self
            .event_bus
            .publish_frame(&GatewayEventFrame::ApprovalRequested {
                approval_id: approval_id.clone(),
                session_key: frame_session_key.clone(),
                channel_id,
                conversation_id,
                tool_call_id,
            })
        {
            tracing::warn!(error = %e, "failed to publish ApprovalRequested for config approval");
        }

        // Phase 3b-2b: surface an in-band "waiting for operator approval" notice
        // on the requester's OWN run output stream, so a chat-tier device sees
        // why its config tool is suspended instead of a silently-spinning tool.
        // Reuses the existing intermediate ResponseChunk path (rendered by every
        // channel + the Panel, never persisted to the transcript). Best-effort:
        // only when we have a gateway run to target; publish failures are
        // non-fatal and must not derail the approval.
        if let Some(t) = &turn {
            if !t.run_id.is_empty() {
                let notice = format!("⏳ 正在等待管理员授权运行工具 `{tool_name}`…");
                if let Err(e) = self
                    .event_bus
                    .publish_frame(&GatewayEventFrame::ResponseChunk {
                        run_id: t.run_id.clone(),
                        seq: 0,
                        delta: notice.clone(),
                        full_text: notice.clone(),
                        content: notice,
                        chunk_index: 0,
                        is_final: false,
                        is_intermediate: true,
                    })
                {
                    tracing::debug!(error = %e, "failed to publish waiting-for-approval notice");
                }
            }
        }

        let resolved = self
            .manager
            .await_registered(approval_id.clone(), rx, timeout)
            .await;
        let decision = resolved.decision;

        // Same scoping as the request frame: an escalation the member could not
        // be told about must not be closed out to them either — the resolved
        // frame carries the approval id, which is the addressing capability the
        // request frame was withheld to deny. They still learn their call
        // un-parked, from its result.
        let frame = match decision {
            Some(d) => GatewayEventFrame::ApprovalResolved {
                approval_id,
                session_key: frame_session_key,
                decision: d,
                resolved_by: None,
            },
            None => GatewayEventFrame::ApprovalExpired {
                approval_id,
                session_key: frame_session_key,
            },
        };
        if let Err(e) = self.event_bus.publish_frame(&frame) {
            tracing::warn!(error = %e, "failed to publish final approval event for config approval");
        }

        ApprovalResponse {
            // Against the set this card was raised with (see
            // `ApprovalAction::allowed_decisions`). On an operator ESCALATION
            // that set never contains the persistent tier — the requesting turn
            // is by construction not operator-tier — so answering "always" here
            // cannot permanently strip the gate a member has to pass.
            outcome: decision.map_or(ApprovalOutcome::Timeout, |d| {
                d.to_outcome_within(&allowed_decisions)
            }),
            deny_reason: resolved.deny_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::socket::ApprovalDecisionType;
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};
    use std::time::Duration;

    fn guest_turn(run_id: &str) -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("approval-test"),
            run_id: run_id.to_string(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
        }
    }

    #[tokio::test]
    async fn emits_waiting_notice_when_run_id_present() {
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let requester = OperatorApprovalRequester::new(manager.clone(), event_bus.clone());
        let mut rx = event_bus.subscribe_typed();

        // request_approval blocks on the decision oneshot; resolve it once the
        // approval is registered so the test terminates.
        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            TURN_CONTEXT
                .scope(guest_turn("run-123"), async move {
                    requester
                        .request_approval(&ApprovalAction::for_tool_call(
                            "set_provider",
                            &serde_json::json!({"provider": "openai"}),
                            "needs config",
                        ))
                        .await
                })
                .await
        });

        let mut saw_notice = false;
        let mut approval_id: Option<String> = None;
        for _ in 0..6 {
            let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
            else {
                break;
            };
            match frame {
                GatewayEventFrame::ApprovalRequested {
                    approval_id: id, ..
                } => {
                    approval_id = Some(id);
                }
                GatewayEventFrame::ResponseChunk {
                    run_id,
                    is_intermediate,
                    is_final,
                    ..
                } => {
                    assert_eq!(run_id, "run-123", "notice must target the requester's run");
                    assert!(
                        is_intermediate,
                        "notice must be an intermediate (ephemeral) chunk"
                    );
                    assert!(!is_final, "notice must not be the final answer");
                    saw_notice = true;
                }
                _ => {}
            }
            if saw_notice {
                if let Some(id) = &approval_id {
                    mgr.resolve(id, ApprovalDecisionType::AllowOnce, None);
                    break;
                }
            }
        }

        assert!(
            saw_notice,
            "expected a run-scoped waiting-for-approval ResponseChunk"
        );
        let outcome = handle.await.unwrap();
        assert_eq!(outcome.outcome, ApprovalOutcome::Approved);
    }

    /// The event leg of the config-tier split.
    ///
    /// `event_visibility::session_identity_of` reads an EMPTY `session_key` on
    /// the `Approval*` family as `OperatorOnly` and a non-empty one as
    /// owner-or-admin. The config gate parks a call because the caller may not
    /// decide, so publishing the caller's own key pushes the card back to them.
    /// Pinned on the frame rather than on the classifier because the classifier
    /// is right — it is the key handed to it that was wrong.
    #[tokio::test]
    async fn a_config_tier_card_is_addressed_to_the_operator_not_to_its_raiser() {
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let requester =
            OperatorApprovalRequester::for_config_tier(manager.clone(), event_bus.clone());
        let mut rx = event_bus.subscribe_typed();

        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            TURN_CONTEXT
                .scope(guest_turn("run-cfg"), async move {
                    requester
                        .request_approval(&ApprovalAction::for_tool_call(
                            "loop_graph",
                            &serde_json::json!({"action": "node", "kind": "root"}),
                            "config tier",
                        ))
                        .await
                })
                .await
        });

        let mut saw_request = false;
        for _ in 0..6 {
            let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
            else {
                break;
            };
            if let GatewayEventFrame::ApprovalRequested {
                approval_id,
                session_key,
                ..
            } = frame
            {
                assert!(
                    session_key.is_empty(),
                    "a config-tier escalation must carry no session key — a non-empty one \
                     classifies as BySessionKeyOrAdmin and delivers the card to the member \
                     whose call the gate just parked"
                );
                saw_request = true;
                mgr.resolve(&approval_id, ApprovalDecisionType::AllowOnce, None);
                break;
            }
        }

        assert!(saw_request, "expected an ApprovalRequested frame");
        let outcome = handle.await.unwrap();
        assert_eq!(outcome.outcome, ApprovalOutcome::Approved);
    }

    /// The twin that must NOT change: the fallback leg's cards belong to the
    /// session that raised them (`src/gateway/CLAUDE.md` 地雷 K — a member has
    /// to receive the card for their own parked tool call, or their run dies on
    /// the 120 s timeout and the only workaround is `exec_tier:"full"`).
    #[tokio::test]
    async fn the_fallback_leg_still_addresses_the_card_to_its_own_session() {
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let requester = OperatorApprovalRequester::new(manager.clone(), event_bus.clone());
        let mut rx = event_bus.subscribe_typed();

        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            TURN_CONTEXT
                .scope(guest_turn("run-own"), async move {
                    requester
                        .request_approval(&ApprovalAction::for_tool_call(
                            "file_ops",
                            &serde_json::json!({"operation": "delete"}),
                            "destructive",
                        ))
                        .await
                })
                .await
        });

        let mut saw_request = false;
        for _ in 0..6 {
            let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
            else {
                break;
            };
            if let GatewayEventFrame::ApprovalRequested {
                approval_id,
                session_key,
                ..
            } = frame
            {
                assert_eq!(
                    session_key,
                    SessionKey::main("approval-test").to_key_string(),
                    "the fallback leg must keep addressing the card to its own session"
                );
                saw_request = true;
                mgr.resolve(&approval_id, ApprovalDecisionType::AllowOnce, None);
                break;
            }
        }

        assert!(saw_request, "expected an ApprovalRequested frame");
        let outcome = handle.await.unwrap();
        assert_eq!(outcome.outcome, ApprovalOutcome::Approved);
    }

    #[tokio::test]
    async fn no_notice_when_run_id_empty() {
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let requester = OperatorApprovalRequester::new(manager.clone(), event_bus.clone());
        let mut rx = event_bus.subscribe_typed();

        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            TURN_CONTEXT
                .scope(guest_turn(""), async move {
                    requester
                        .request_approval(&ApprovalAction::for_tool_call(
                            "set_provider",
                            &serde_json::json!({"provider": "openai"}),
                            "needs config",
                        ))
                        .await
                })
                .await
        });

        // Drain frames; resolve on ApprovalRequested; assert no ResponseChunk
        // appears before the approval resolves.
        let mut saw_chunk = false;
        for _ in 0..6 {
            let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
            else {
                break;
            };
            match frame {
                GatewayEventFrame::ApprovalRequested { approval_id, .. } => {
                    mgr.resolve(&approval_id, ApprovalDecisionType::AllowOnce, None);
                }
                GatewayEventFrame::ResponseChunk { .. } => {
                    saw_chunk = true;
                }
                GatewayEventFrame::ApprovalResolved { .. } => break,
                _ => {}
            }
        }

        assert!(!saw_chunk, "no notice must be emitted when run_id is empty");
        let outcome = handle.await.unwrap();
        assert_eq!(outcome.outcome, ApprovalOutcome::Approved);
    }
}
