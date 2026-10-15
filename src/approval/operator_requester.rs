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
//! parked tool call, or their run dies on the approval timeout and the
//! documented workaround is `exec_tier:"full"` — the least safe tier becoming
//! the only usable one. The timeout was 120 s when that was written; an
//! attended card has had no deadline since 2026-08-28, which changes the
//! symptom from a dying run to a permanently parked one and leaves the ruling
//! untouched). The judgement now lives in
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
use tokio_util::sync::CancellationToken;

use crate::exec::decision::ApprovalRequest;
use crate::exec::manager::{ExecApprovalManager, ExecApprovalRecord};
use crate::exec::socket::ApprovalDecisionType;
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

    /// Independent single-use approval for telephone start/answer. The
    /// lifecycle service checks typed ownership/capability before calling;
    /// this helper checks only that it was wired to an operator-scoped instance.
    ///
    /// `action.summary` must contain the caller's complete bounded, redacted
    /// action body. Store it intact once; events carry only the addressing id,
    /// with an empty session key so the existing operator RPC is the surface.
    /// No standing grant, caller notice, detached timer or reminder is created.
    pub(crate) async fn request_operator_once(
        &self,
        action: &ApprovalAction,
        cancel: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> ApprovalResponse {
        fn deny() -> ApprovalResponse {
            ApprovalResponse {
                outcome: ApprovalOutcome::Denied,
                deny_reason: Some("operator single-use approval was not granted".to_string()),
            }
        }

        let created_now = tokio::time::Instant::now();
        let deadline = deadline.min(created_now + std::time::Duration::from_secs(60));
        if !self.operator_only || cancel.is_cancelled() || created_now >= deadline {
            return deny();
        }

        let turn = crate::tools::turn_context::current_turn_context();
        let (session_key, agent_id, channel_id, conversation_id) = match &turn {
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
            command: action.summary.clone(),
            cwd: action.cwd.clone(),
            analysis: action.analysis_for_record(),
            agent_id,
            session_key,
            reason: (!action.reason.is_empty()).then(|| action.reason.clone()),
            originator_user_id: crate::tools::turn_context::current_originator(),
            grant_key: None,
            allowed_decisions: crate::exec::allowed_decisions::once_only(),
        };
        // Do not use manager.create(): its debug log includes the command.
        // Floor the remaining window; zero means deny, NOT the manager's
        // no-expiry sentinel. A sub-millisecond budget cannot carry a record.
        let mut record = ExecApprovalRecord::from_request(&request, 1);
        let remaining_ms = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .as_millis() as u64;
        if cancel.is_cancelled() || remaining_ms == 0 {
            return deny();
        }
        record.expires_at_ms = record.created_at_ms.saturating_add(remaining_ms);
        let tool_call_id = record.tool_call_id.clone();
        // This registration forces operator_only + once_only + no grant key.
        // The guard exists before notification and lives across every await.
        let Some((approval_id, rx, timeout, registration)) =
            self.manager.register_operator_once(record)
        else {
            // A colliding id never replaces another approval or publishes a
            // notification for it. Fail closed through the existing outcome.
            return deny();
        };
        // Check again immediately before notification in case cancellation or
        // expiry happened during synchronous record construction/registration.
        if cancel.is_cancelled() || tokio::time::Instant::now() >= deadline {
            return deny();
        }
        if !matches!(
            self.event_bus.publish_frame(&GatewayEventFrame::ApprovalRequested {
                approval_id: approval_id.clone(),
                session_key: String::new(),
                channel_id,
                conversation_id,
                tool_call_id,
            }),
            Ok(n) if n > 0
        ) {
            // Remove only this registration via RAII. Resolving Deny as cleanup could
            // turn an undelivered notification into a refusal of sibling cards.
            return deny();
        }

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let timeout = Some(timeout.map_or(remaining, |t| t.min(remaining)));
        let resolved = tokio::select! {
            biased;
            () = cancel.cancelled() => None,
            () = tokio::time::sleep_until(deadline) => None,
            response = registration.wait(rx, timeout) => Some(response),
        };
        // A ready oneshot can win even though the decision was queued before
        // cancellation/expiry and consumed afterwards. Never trust readiness
        // (or the manager's wall-clock TTL) as the permission to execute.
        // Close the registry before the final event prompts the operator UI to
        // refetch it. On cancellation/expiry the guard-owned wait was dropped.
        drop(registration);
        let still_valid = !cancel.is_cancelled() && tokio::time::Instant::now() < deadline;
        let decision = resolved.and_then(|r| r.decision).filter(|_| still_valid);
        let frame = match decision {
            Some(d @ (ApprovalDecisionType::AllowOnce | ApprovalDecisionType::Deny)) => {
                GatewayEventFrame::ApprovalResolved {
                    approval_id,
                    session_key: String::new(),
                    decision: d,
                    resolved_by: None,
                }
            }
            _ => GatewayEventFrame::ApprovalExpired {
                approval_id,
                session_key: String::new(),
            },
        };
        let _ = self.event_bus.publish_frame(&frame);
        // Only an ACTUAL AllowOnce is permission. Never map/clamp a wider
        // decision here, and never return a session or persistent outcome.
        if decision == Some(ApprovalDecisionType::AllowOnce)
            && !cancel.is_cancelled()
            && tokio::time::Instant::now() < deadline
        {
            ApprovalResponse {
                outcome: ApprovalOutcome::Approved,
                deny_reason: None,
            }
        } else {
            deny()
        }
    }

    /// The key to publish on the `Approval*` frames for this instance.
    /// Resolve `approval_id` with a `Deny("unavailable")` and return the
    /// matching `ApprovalResponse`. Shared by the `Ok(0)` (no operator
    /// surface) and `Err(_)` (publish failed) arms — both deny with the same
    /// reason so the caller cannot tell which leg of the bus failure tripped
    /// the gate, and a caller who could legitimately distinguish them can
    /// still log the cause at the call site (the warn! lives there).
    fn deny_unavailable(&self, approval_id: &str) -> ApprovalResponse {
        self.manager.resolve(
            approval_id,
            ApprovalDecisionType::Deny,
            Some("unavailable".to_string()),
        );
        ApprovalResponse {
            outcome: ApprovalOutcome::Denied,
            deny_reason: Some(
                "approval notification could not be delivered to the operator surface".to_string(),
            ),
        }
    }

    /// Re-announce a still-parked approval on `schedule`, forever.
    ///
    /// Never returns: it is a `select!` arm whose only job is to lose the race
    /// against the wait, so the caller is freed by the answer and this future is
    /// dropped mid-sleep. Making it return on some "give up" condition would be
    /// the silent timeout again, wearing a different name — the card would still
    /// be parked, and nothing would ever say so again.
    ///
    /// Publish failures are logged and the schedule continues. A reminder is
    /// best-effort by construction (the initial publish is the one that is
    /// fatal, and it already happened): a bus hiccup on minute two must not
    /// cancel minute seven.
    async fn remind_until_answered(
        event_bus: &GatewayEventBus,
        approval_id: &str,
        frame_session_key: &str,
        schedule: impl Iterator<Item = std::time::Duration> + Send,
    ) {
        for delay in schedule {
            tokio::time::sleep(delay).await;
            match event_bus.publish_frame(&GatewayEventFrame::ApprovalReminder {
                approval_id: approval_id.to_string(),
                session_key: frame_session_key.to_string(),
            }) {
                Ok(0) => {
                    // No subscriber received the reminder; do not log a misleading
                    // "re-raised" success line. The initial publish already
                    // proved fatal for the same case (see APPROVAL-R4-01); on the
                    // reminder path we just stay quiet.
                    tracing::debug!(
                        id = %approval_id,
                        "approval reminder reached no subscribers; skipping log"
                    );
                }
                Ok(n) => {
                    tracing::info!(
                        id = %approval_id,
                        delivered_to = n,
                        "re-raised the operator interrupt for a still-parked approval"
                    );
                }
                Err(e) => {
                    tracing::debug!(error = %e, id = %approval_id, "approval reminder publish failed");
                }
            }
        }
        // `reminder_schedule` yields an endless iterator, so this is dead code
        // — reachable only if a future caller hands in a finite one, which is
        // the mistake this arm exists to make impossible.
        std::future::pending::<()>().await;
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
            // RPC, not a channel button, so the channel-callback originator
            // gate (`callback_sink.rs`) never applies to this record. But a
            // SECOND consumer does: `approval_addressable_by_caller`
            // (Ruling P13/P15) narrows by this value when the session is a
            // Project room and the value resolves against that room's
            // roster — precisely a team-chat run's room-speaker
            // (`teams::broadcast::member_run_metadata` stamps it as an Aleph
            // `u-*` id). A channel-routed run that falls back to THIS
            // requester (`FallbackApprovalRequester`, when its channel is
            // momentarily unregistered) may also have seeded this task-local,
            // but with a raw channel-platform id from a different namespace
            // — that value never resolves against any room's roster, so it
            // changes nothing for that path; see
            // `approval_addressable_by_caller`'s doc.
            originator_user_id: crate::tools::turn_context::current_originator(),
            // Session-grant identity of this action: a session-level decision
            // cascades to other pending cards of the same action.
            grant_key: action.grant_key.clone(),
            // What the gate decided this card may offer. Carried onto the
            // record, where the resolve RPC enforces it.
            allowed_decisions: action.allowed_decisions.clone(),
        };
        // Kept for the outcome mapping below: `request` is moved into `create`.
        let allowed_decisions = action.allowed_decisions.clone();
        let mut record = self.manager.create(
            &request,
            crate::approval::approval_timeout_for_current_turn(),
        );
        // The RPC leg of the same fact the blank frame key carries. Stamped on
        // the record (not derived from its `session_key`, which stays real for
        // the grant cascade) so `exec.approvals.pending` and
        // `exec.approval.resolve` — both carved open to members — refuse it.
        record.operator_only = self.operator_only;
        // Pairing key for the client: which tool row this card belongs under.
        let tool_call_id = record.tool_call_id.clone();
        // Empty for a config-tier escalation: `session_identity_of` reads an empty
        // key as `SessionIdentity::OperatorOnly`, the same classification a
        // cluster-node approval already gets, so no new wire field is needed.
        let frame_session_key = if self.operator_only {
            String::new()
        } else {
            session_key_str.clone()
        };
        // Register the pending entry BEFORE publishing the event, so an operator
        // who resolves the instant they see it cannot race ahead of
        // registration (resolve-before-register would otherwise be lost and the
        // approval would spuriously time out). The entry is resolvable the
        // moment `register_pending` returns.
        let (approval_id, rx, timeout) = self.manager.register_pending(record);

        // Initial publish failure is fatal for this approval: the operator
        // was never notified, so a "waiting" card never appeared in their
        // surface. Mirror the channel-bridge contract (`exec/approval/
        // channel_bridge.rs`) — wake the waiter with `Deny` so the caller
        // learns the user was not reached, and remove the pending entry.
        // See APPROVAL-R3-003.
        //
        // `publish_frame` returns `Ok(0)` when there are zero subscribers (or
        // every subscriber lagged behind the broadcast ring). The previous fix
        // only handled `Err(e)`, so an operator surface that was momentarily
        // disconnected at the moment of publish would silently park the
        // approval indefinitely. Treat `Ok(0)` as fatal too — APPROVAL-R4-01.
        match self
            .event_bus
            .publish_frame(&GatewayEventFrame::ApprovalRequested {
                approval_id: approval_id.clone(),
                session_key: frame_session_key.clone(),
                channel_id,
                conversation_id,
                tool_call_id,
            }) {
            Ok(0) => {
                tracing::warn!(
                    id = %approval_id,
                    "ApprovalRequested reached no subscribers (Ok(0)); denying so caller is not stranded"
                );
                return self.deny_unavailable(&approval_id);
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "failed to publish ApprovalRequested for config approval");
                return self.deny_unavailable(&approval_id);
            }
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

        // The wait and its reminders, as one future. `select!` rather than a
        // spawned task on purpose: the reminder loop never completes, so the
        // arm that finishes is always the wait, and losing the race DROPS the
        // loop. A spawned task would have to be aborted on every one of this
        // function's exits, and the reminder for a card answered ten seconds
        // ago is exactly the "notification about something that already
        // happened" this whole ruling exists to stop.
        let resolved = {
            let wait = self
                .manager
                .await_registered(approval_id.clone(), rx, timeout);
            match crate::approval::reminder_schedule(timeout) {
                None => wait.await,
                Some(schedule) => {
                    let remind = Self::remind_until_answered(
                        &self.event_bus,
                        &approval_id,
                        &frame_session_key,
                        schedule,
                    );
                    tokio::select! {
                        resolved = wait => resolved,
                        () = remind => unreachable!("the reminder loop never returns"),
                    }
                }
            }
        };
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

    // Manual polling prevents paused-clock auto-advance and makes queued-response
    // races reproducible without spawned tasks or scheduling assumptions.
    fn poll_once<F: std::future::Future>(
        future: std::pin::Pin<&mut F>,
    ) -> std::task::Poll<F::Output> {
        future.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    }

    fn once_fixture() -> (
        Arc<ExecApprovalManager>,
        OperatorApprovalRequester,
        tokio::sync::broadcast::Receiver<GatewayEventFrame>,
    ) {
        let bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let requester = OperatorApprovalRequester::for_config_tier(manager.clone(), bus.clone());
        (manager, requester, bus.subscribe_typed())
    }

    fn once_action() -> ApprovalAction {
        let mut action = ApprovalAction::bare("telephone", "start/answer requires approval");
        // The lifecycle caller supplies a complete bounded body in summary,
        // not the generic tool preview (which truncates at 200 characters).
        action.summary = format!(
            "telephone start: destination=+15551234567 body={}",
            "x".repeat(1024)
        );
        action.grant_key = Some("telephone-action".to_string());
        action.allowed_decisions = crate::exec::allowed_decisions::with_persistent();
        action
    }

    fn once_request_id(events: &mut tokio::sync::broadcast::Receiver<GatewayEventFrame>) -> String {
        match events
            .try_recv()
            .expect("registration must notify synchronously")
        {
            GatewayEventFrame::ApprovalRequested {
                approval_id,
                session_key,
                ..
            } => {
                assert!(
                    session_key.is_empty(),
                    "only the operator may receive the event"
                );
                approval_id
            }
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    fn denied(response: ApprovalResponse) {
        assert_eq!(response.outcome, ApprovalOutcome::Denied);
        assert!(response.deny_reason.is_some());
    }

    // Removing either preflight guard would leave a pending entry/event behind.
    #[tokio::test(start_paused = true)]
    async fn operator_once_preflight_refuses_without_registering_or_notifying() {
        for case in ["expired", "cancelled", "owner"] {
            let bus = Arc::new(GatewayEventBus::new());
            let manager = Arc::new(ExecApprovalManager::new());
            let mut events = bus.subscribe_typed();
            let requester = if case == "owner" {
                OperatorApprovalRequester::new(manager.clone(), bus)
            } else {
                OperatorApprovalRequester::for_config_tier(manager.clone(), bus)
            };
            let cancel = tokio_util::sync::CancellationToken::new();
            if case == "cancelled" {
                cancel.cancel();
            }
            let deadline = tokio::time::Instant::now()
                + if case == "expired" {
                    Duration::ZERO
                } else {
                    Duration::from_secs(30)
                };
            denied(
                requester
                    .request_operator_once(&once_action(), &cancel, deadline)
                    .await,
            );
            assert!(
                manager.list_pending().is_empty(),
                "{case} must not register"
            );
            assert!(events.try_recv().is_err(), "{case} must not publish");
        }
    }

    // Catches owner routing, body truncation, widening decisions and failure to
    // register before notification (the record must already be resolvable).
    #[tokio::test(start_paused = true)]
    async fn operator_once_preserves_full_body_and_real_session_but_is_operator_only() {
        let (manager, requester, mut events) = once_fixture();
        let action = once_action();
        let cancel = tokio_util::sync::CancellationToken::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut future = Box::pin(TURN_CONTEXT.scope(
            guest_turn("once-run"),
            requester.request_operator_once(&action, &cancel, deadline),
        ));
        assert!(poll_once(future.as_mut()).is_pending());
        let id = once_request_id(&mut events);
        let record = manager
            .get_pending(&id)
            .expect("registered before notification")
            .record;
        assert!(record.operator_only);
        assert_eq!(
            record.session_key,
            SessionKey::main("approval-test").to_key_string()
        );
        assert_eq!(
            record.command, action.summary,
            "full bounded body, not another preview"
        );
        assert_eq!(
            record.allowed_decisions,
            vec![ApprovalDecisionType::AllowOnce, ApprovalDecisionType::Deny]
        );
        assert!(
            record.grant_key.is_none(),
            "isolated from grants and denials on other cards"
        );
        assert!(manager.resolve(&id, ApprovalDecisionType::AllowOnce, None));
        let std::task::Poll::Ready(response) = poll_once(future.as_mut()) else {
            panic!("answer lost");
        };
        assert_eq!(response.outcome, ApprovalOutcome::Approved);
        assert!(response.deny_reason.is_none());
        assert!(manager.get_pending(&id).is_none());
        match events.try_recv().expect("final notification") {
            GatewayEventFrame::ApprovalResolved {
                session_key,
                decision,
                ..
            } => {
                assert!(session_key.is_empty());
                assert_eq!(decision, ApprovalDecisionType::AllowOnce);
            }
            other => panic!("unexpected final frame: {other:?}"),
        }
        assert!(
            events.try_recv().is_err(),
            "no second request, reminder or caller notice"
        );
    }

    // Catches both replacing the caller deadline with 60s and allowing a later
    // caller deadline to extend the one-shot beyond 60s.
    #[tokio::test(start_paused = true)]
    async fn operator_once_uses_the_earlier_absolute_deadline_and_caps_at_sixty_seconds() {
        for (caller_secs, budget_secs) in [(10, 10), (3600, 60)] {
            let (manager, requester, mut events) = once_fixture();
            let action = once_action();
            let cancel = tokio_util::sync::CancellationToken::new();
            let mut future = Box::pin(requester.request_operator_once(
                &action,
                &cancel,
                tokio::time::Instant::now() + Duration::from_secs(caller_secs),
            ));
            assert!(poll_once(future.as_mut()).is_pending());
            let id = once_request_id(&mut events);
            let record = manager.get_pending(&id).unwrap().record;
            let timeout_ms = record.expires_at_ms - record.created_at_ms;
            assert!(timeout_ms > 0 && timeout_ms <= budget_secs * 1000);
            tokio::time::advance(Duration::from_secs(budget_secs - 1)).await;
            assert!(poll_once(future.as_mut()).is_pending());
            tokio::time::advance(Duration::from_secs(1)).await;
            let std::task::Poll::Ready(response) = poll_once(future.as_mut()) else {
                panic!("deadline ignored");
            };
            denied(response);
            assert!(manager.get_pending(&id).is_none());
            assert!(
                matches!(events.try_recv().unwrap(), GatewayEventFrame::ApprovalExpired { session_key, .. } if session_key.is_empty())
            );
            assert!(
                events.try_recv().is_err(),
                "bounded approvals do not remind"
            );
        }
    }

    // The manager's wall clock need not notice a late resolve: the helper must
    // re-check its monotonic clock even with an already-ready oneshot.
    #[tokio::test(start_paused = true)]
    async fn operator_once_refuses_answers_at_or_after_deadline() {
        for (elapsed, approved) in [(9, true), (10, false), (11, false)] {
            let (manager, requester, mut events) = once_fixture();
            let action = once_action();
            let cancel = tokio_util::sync::CancellationToken::new();
            let mut future = Box::pin(requester.request_operator_once(
                &action,
                &cancel,
                tokio::time::Instant::now() + Duration::from_secs(10),
            ));
            assert!(poll_once(future.as_mut()).is_pending());
            let id = once_request_id(&mut events);
            tokio::time::advance(Duration::from_secs(elapsed)).await;
            // Paused Tokio time is independent of the manager's wall clock.
            assert!(manager.resolve(&id, ApprovalDecisionType::AllowOnce, None));
            let std::task::Poll::Ready(response) = poll_once(future.as_mut()) else {
                panic!("answer lost");
            };
            if approved {
                assert_eq!(response.outcome, ApprovalOutcome::Approved);
            } else {
                denied(response);
            }
            assert!(manager.get_pending(&id).is_none());
            let final_frame = events.try_recv().unwrap();
            if !approved {
                assert!(
                    matches!(final_frame, GatewayEventFrame::ApprovalExpired { session_key, .. } if session_key.is_empty()),
                    "late approval must not be announced as allowed"
                );
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn operator_once_queued_timely_answer_is_refused_when_consumed_at_deadline() {
        let (manager, requester, mut events) = once_fixture();
        let action = once_action();
        let cancel = CancellationToken::new();
        let mut future = Box::pin(requester.request_operator_once(
            &action,
            &cancel,
            tokio::time::Instant::now() + Duration::from_secs(10),
        ));
        assert!(poll_once(future.as_mut()).is_pending());
        let id = once_request_id(&mut events);
        assert!(manager.resolve(&id, ApprovalDecisionType::AllowOnce, None));
        tokio::time::advance(Duration::from_secs(10)).await;
        let std::task::Poll::Ready(response) = poll_once(future.as_mut()) else {
            panic!("queued answer lost");
        };
        denied(response);
        assert!(manager.get_pending(&id).is_none());
        assert!(matches!(
            events.try_recv().unwrap(),
            GatewayEventFrame::ApprovalExpired { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn operator_once_expired_submillisecond_budget_never_becomes_an_unbounded_wait() {
        let (manager, requester, mut events) = once_fixture();
        let cancel = CancellationToken::new();
        denied(
            requester
                .request_operator_once(
                    &once_action(),
                    &cancel,
                    tokio::time::Instant::now() + Duration::from_nanos(1),
                )
                .await,
        );
        assert!(manager.list_pending().is_empty());
        assert!(events.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn operator_once_human_denial_returns_only_a_static_reason() {
        let (manager, requester, mut events) = once_fixture();
        let action = once_action();
        let cancel = CancellationToken::new();
        let mut future = Box::pin(requester.request_operator_once(
            &action,
            &cancel,
            tokio::time::Instant::now() + Duration::from_secs(30),
        ));
        assert!(poll_once(future.as_mut()).is_pending());
        let id = once_request_id(&mut events);
        assert!(manager.resolve_with_reason(
            &id,
            ApprovalDecisionType::Deny,
            None,
            Some("sensitive operator text".to_string())
        ));
        let std::task::Poll::Ready(response) = poll_once(future.as_mut()) else {
            panic!("denial lost");
        };
        assert!(!response
            .deny_reason
            .as_ref()
            .unwrap()
            .contains("sensitive operator text"));
        denied(response);
        assert!(
            matches!(events.try_recv().unwrap(), GatewayEventFrame::ApprovalResolved { decision: ApprovalDecisionType::Deny, session_key, .. } if session_key.is_empty())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn operator_once_cancellation_wins_over_an_already_queued_answer() {
        let (manager, requester, mut events) = once_fixture();
        let action = once_action();
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut future = Box::pin(requester.request_operator_once(
            &action,
            &cancel,
            tokio::time::Instant::now() + Duration::from_secs(30),
        ));
        assert!(poll_once(future.as_mut()).is_pending());
        let id = once_request_id(&mut events);
        assert!(manager.resolve(&id, ApprovalDecisionType::AllowOnce, None));
        cancel.cancel();
        let std::task::Poll::Ready(response) = poll_once(future.as_mut()) else {
            panic!("cancellation ignored");
        };
        denied(response);
        assert!(manager.get_pending(&id).is_none());
        assert!(matches!(
            events.try_recv().unwrap(),
            GatewayEventFrame::ApprovalExpired { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn operator_once_cancel_or_drop_leaves_sibling_cards_waiting() {
        for drop_future in [false, true] {
            let (manager, requester, mut events) = once_fixture();
            let action = once_action();
            let sibling_request = ApprovalRequest {
                id: "sibling".to_string(),
                command: "sibling".to_string(),
                cwd: None,
                analysis: action.analysis_for_record(),
                agent_id: "approval-test".to_string(),
                session_key: SessionKey::main("approval-test").to_key_string(),
                reason: None,
                originator_user_id: None,
                grant_key: action.grant_key.clone(),
                allowed_decisions: crate::exec::allowed_decisions::with_persistent(),
            };
            let (sibling_id, mut sibling_rx, _) =
                manager.register_pending(manager.create(&sibling_request, 60_000));
            let cancel = tokio_util::sync::CancellationToken::new();
            let mut future = Box::pin(TURN_CONTEXT.scope(
                guest_turn("cancel-run"),
                requester.request_operator_once(
                    &action,
                    &cancel,
                    tokio::time::Instant::now() + Duration::from_secs(30),
                ),
            ));
            assert!(poll_once(future.as_mut()).is_pending());
            let id = once_request_id(&mut events);
            if drop_future {
                drop(future);
            } else {
                cancel.cancel();
                let std::task::Poll::Ready(response) = poll_once(future.as_mut()) else {
                    panic!("cancel ignored");
                };
                denied(response);
            }
            assert!(manager.get_pending(&id).is_none());
            assert!(manager.get_pending(&sibling_id).is_some());
            assert!(
                matches!(
                    sibling_rx.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                ),
                "cleanup must not cascade a denial"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn operator_once_no_subscribers_denies_without_cascading() {
        let manager = Arc::new(ExecApprovalManager::new());
        let requester = OperatorApprovalRequester::for_config_tier(
            manager.clone(),
            Arc::new(GatewayEventBus::new()),
        );
        let action = once_action();
        let request = ApprovalRequest {
            id: "sibling".to_string(),
            command: "sibling".to_string(),
            cwd: None,
            analysis: action.analysis_for_record(),
            agent_id: String::new(),
            session_key: String::new(),
            reason: None,
            originator_user_id: None,
            grant_key: action.grant_key.clone(),
            allowed_decisions: crate::exec::allowed_decisions::session_max(),
        };
        let (id, mut rx, _) = manager.register_pending(manager.create(&request, 60_000));
        let cancel = tokio_util::sync::CancellationToken::new();
        denied(
            requester
                .request_operator_once(
                    &action,
                    &cancel,
                    tokio::time::Instant::now() + Duration::from_secs(30),
                )
                .await,
        );
        assert_eq!(
            manager.list_pending().len(),
            1,
            "failed notification must remove only its own entry"
        );
        assert!(manager.get_pending(&id).is_some());
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
    }

    // Exercises the existing in-process RPC, not just a card renderer: a
    // forged wider decision must fail at the resolver, not become AllowOnce.
    #[tokio::test(start_paused = true)]
    async fn operator_once_rpc_rejects_session_and_persistent_decisions() {
        use crate::gateway::handlers::{exec_approvals, HandlerRegistry};
        use crate::gateway::protocol::JsonRpcRequest;
        use crate::gateway::session_store::file_backend::{
            FileSessionStore, FileSessionStoreConfig,
        };
        let (manager, requester, mut events) = once_fixture();
        let dir = tempfile::tempdir().unwrap();
        let sessions = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: dir.path().to_path_buf(),
                ..Default::default()
            })
            .unwrap(),
        );
        let mut registry = HandlerRegistry::new();
        exec_approvals::register_handlers(&mut registry, manager.clone(), sessions);
        let action = once_action();
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut future = Box::pin(TURN_CONTEXT.scope(
            guest_turn("once-rpc-run"),
            crate::tools::turn_context::with_originator(
                Some("alice".to_string()),
                requester.request_operator_once(
                    &action,
                    &cancel,
                    tokio::time::Instant::now() + Duration::from_secs(30),
                ),
            ),
        ));
        assert!(poll_once(future.as_mut()).is_pending());
        let id = once_request_id(&mut events);
        assert_eq!(
            manager.resolve_for_session(
                &SessionKey::main("approval-test").to_key_string(),
                None,
                ApprovalDecisionType::AllowOnce,
                Some("alice".to_string()),
                None,
            ),
            crate::exec::manager::SessionResolveOutcome::NothingPending,
            "the real helper must not grant via the originator's /approve",
        );
        use crate::gateway::inbound_router::approval_callback::ApprovalCallbackSink;
        let sink = crate::approval::callback_sink::ManagerCallbackSink::new(manager.clone());
        let callback = sink
            .handle_callback(&format!("approve:{id}:once"), "alice")
            .await
            .unwrap();
        assert!(!callback.resolved);
        assert!(poll_once(future.as_mut()).is_pending());
        for decision in [
            ApprovalDecisionType::AllowSession,
            ApprovalDecisionType::AllowAlways,
            ApprovalDecisionType::AllowOnce,
        ] {
            let request = JsonRpcRequest::with_id(
                "exec.approval.resolve",
                Some(serde_json::json!({"id": id, "decision": decision})),
                serde_json::json!(1),
            );
            let response = crate::gateway::caller_identity::CALLER_ROLE
                .scope(Some("operator".to_string()), registry.handle(&request))
                .await;
            if decision == ApprovalDecisionType::AllowOnce {
                assert!(response.error.is_none());
            } else {
                assert!(response.error.is_some());
                assert!(
                    manager.get_pending(&id).is_some(),
                    "invalid decision must not grant or consume"
                );
                assert!(poll_once(future.as_mut()).is_pending());
            }
        }
        let std::task::Poll::Ready(response) = poll_once(future.as_mut()) else {
            panic!("AllowOnce lost");
        };
        assert_eq!(response.outcome, ApprovalOutcome::Approved);
    }

    fn guest_turn(run_id: &str) -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("approval-test"),
            run_id: run_id.to_string(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
            telecom_authority: None,
        }
    }

    /// Link 1 (Ruling P13): the owner-scoped requester must carry whatever
    /// `TURN_ORIGINATOR` the run seeded onto the record it creates — a
    /// team-chat run's room-speaker (`teams::broadcast::member_run_metadata`)
    /// or a channel-routed run's raw sender id, should it fall back here via
    /// `FallbackApprovalRequester` — instead of hardcoding `None`.
    /// `approval_addressable_by_caller` is what narrows by this value; this
    /// requester's only job is to stop discarding it.
    #[tokio::test]
    async fn request_approval_carries_the_ambient_originator_onto_the_record() {
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let requester = OperatorApprovalRequester::new(manager.clone(), event_bus.clone());
        let mut rx = event_bus.subscribe_typed();

        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            TURN_CONTEXT
                .scope(guest_turn("run-orig"), async move {
                    crate::tools::turn_context::with_originator(
                        Some("u-bob".to_string()),
                        async move {
                            requester
                                .request_approval(&ApprovalAction::for_tool_call(
                                    "file_ops",
                                    &serde_json::json!({"operation": "delete"}),
                                    "destructive",
                                ))
                                .await
                        },
                    )
                    .await
                })
                .await
        });

        let mut approval_id = None;
        for _ in 0..6 {
            let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
            else {
                break;
            };
            if let GatewayEventFrame::ApprovalRequested {
                approval_id: id, ..
            } = frame
            {
                approval_id = Some(id);
                break;
            }
        }
        let id = approval_id.expect("expected an ApprovalRequested frame");
        assert_eq!(
            manager
                .get_pending(&id)
                .map(|p| p.record.originator_user_id.clone()),
            Some(Some("u-bob".to_string())),
            "the record must carry the ambient originator, not a hardcoded None"
        );
        mgr.resolve(&id, ApprovalDecisionType::AllowOnce, None);
        let outcome = handle.await.unwrap();
        assert_eq!(outcome.outcome, ApprovalOutcome::Approved);
    }

    /// A parked card with NO deadline re-raises the operator's interrupt on the
    /// backoff schedule, and STOPS the moment it is answered.
    ///
    /// This is the whole of the notify-and-wait ruling's second half. Removing
    /// the deadline made a missed card wait instead of expiring, which is only
    /// an improvement if something eventually fetches the human — and the thing
    /// that does fires exactly once, at raise time. Both halves are asserted
    /// here because they fail in opposite directions: no reminder is the silent
    /// wait the ruling replaced, and a reminder AFTER the answer is a
    /// notification about something that already happened.
    ///
    /// Paused clock: the schedule is minutes long, and the reminder loop's only
    /// suspension point is its sleep, so auto-advance drives it deterministically.
    #[tokio::test(start_paused = true)]
    async fn an_unanswered_card_is_re_announced_and_stops_when_answered() {
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let requester = OperatorApprovalRequester::new(manager.clone(), event_bus.clone());
        let mut rx = event_bus.subscribe_typed();

        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            TURN_CONTEXT
                .scope(guest_turn("run-remind"), async move {
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

        // The raise, then two reminders — proving the schedule advances rather
        // than firing once.
        let mut id = None;
        let mut reminders = 0;
        while reminders < 2 {
            // Bounded on purpose. A bare `recv().await` here would HANG the
            // whole suite the day the wiring breaks — and a suite that never
            // prints its result line is worse than one that prints a failure.
            // The bound is far past every step of the schedule, and the paused
            // clock advances to the EARLIEST pending deadline, so the reminders
            // still fire first and the budget does not distort them.
            let next = tokio::time::timeout(Duration::from_secs(3600), rx.recv())
                .await
                .expect("no reminder arrived within an hour of schedule time");
            match next.expect("bus closed") {
                GatewayEventFrame::ApprovalRequested {
                    approval_id: got, ..
                } => {
                    assert!(id.is_none(), "the request must be announced exactly once");
                    id = Some(got);
                }
                GatewayEventFrame::ApprovalReminder { approval_id, .. } => {
                    assert_eq!(
                        Some(&approval_id),
                        id.as_ref(),
                        "a reminder must name the card it is reminding about"
                    );
                    reminders += 1;
                }
                _ => {}
            }
        }
        let id = id.expect("expected an ApprovalRequested frame");
        assert!(
            manager.get_pending(&id).is_some(),
            "the card must still be parked while it is being re-announced"
        );

        mgr.resolve(&id, ApprovalDecisionType::AllowOnce, None);
        assert_eq!(handle.await.unwrap().outcome, ApprovalOutcome::Approved);

        // Far past every remaining step of the schedule. The loop is a `select!`
        // arm, so answering DROPS it; a spawned reminder task would still be
        // sleeping here and would ring into a resolved card.
        tokio::time::advance(Duration::from_secs(3600)).await;
        while let Ok(frame) = rx.try_recv() {
            assert!(
                !matches!(frame, GatewayEventFrame::ApprovalReminder { .. }),
                "a reminder fired after the card was answered"
            );
        }
    }

    /// An UNATTENDED turn keeps its bounded wait, and a bounded wait is never
    /// re-announced: it ends on its own, so an interrupt could only fetch a
    /// human to a card that is about to stop existing — and there is no human
    /// on any surface to fetch, which is what "unattended" means.
    ///
    /// # This is NOT the guard on `reminder_schedule`'s predicate
    ///
    /// Deleting the `timeout.is_some()` early return leaves this test GREEN.
    /// The first backoff step and the bounded default are both 120 s, so the
    /// reminder and the expiry come due in the same instant and `select!`
    /// resolves the tie in the wait's favour — the mutation is invisible from
    /// out here. What actually pins the predicate is
    /// `reminder_tests::a_bounded_wait_raises_no_reminders`, which asserts it
    /// directly (verified: that mutation reds it by name).
    ///
    /// This test earns its place on the other half — that the bounded wait
    /// really runs to expiry on a live requester — not on the negative.
    #[tokio::test(start_paused = true)]
    async fn a_bounded_card_is_never_re_announced() {
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let requester = OperatorApprovalRequester::new(manager.clone(), event_bus.clone());
        let mut rx = event_bus.subscribe_typed();

        let mut turn = guest_turn("run-unattended");
        turn.unattended = true;
        let handle = tokio::spawn(async move {
            TURN_CONTEXT
                .scope(turn, async move {
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

        // Nobody answers: the bounded wait runs out on its own.
        let outcome = handle.await.unwrap();
        assert_eq!(
            outcome.outcome,
            ApprovalOutcome::Timeout,
            "an unattended card must still expire — parking it forever wedges a \
             run no human is watching"
        );
        let mut saw_expired = false;
        while let Ok(frame) = rx.try_recv() {
            match frame {
                GatewayEventFrame::ApprovalReminder { .. } => {
                    panic!("a bounded card must raise no reminders")
                }
                GatewayEventFrame::ApprovalExpired { .. } => saw_expired = true,
                _ => {}
            }
        }
        assert!(
            saw_expired,
            "self-guard: the bounded wait must actually have run to expiry, or \
             the no-reminder assertion above passed without the clock moving"
        );
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
