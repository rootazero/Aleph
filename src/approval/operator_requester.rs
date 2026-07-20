//! `OperatorApprovalRequester` — an [`ApprovalRequester`] that routes a config
//! tool approval to the SERVER OPERATOR (not the requesting chat-tier device).
//!
//! Unlike `ChannelApprovalBridgeAdapter` (which delivers back to the
//! requester's own channel), this registers a pending approval in the shared
//! [`ExecApprovalManager`] and publishes a `GatewayEventFrame::Approval*` event
//! that — after the `event_scope` `approval.` guard — only operator-tier
//! connections receive. The operator resolves it via the existing
//! `exec.approval.resolve` RPC, waking the oneshot. Used by the config-tier gate
//! in `ScopedToolService` (Phase 2b sudo).
//!
//! Scope (Phase 2b): `AllowOnce` + `AllowSession` only. `AllowAlways` collapses to a
//! session grant — permanent device elevation is Phase 3.

use async_trait::async_trait;

use crate::exec::decision::ApprovalRequest;
use crate::exec::manager::{ExecApprovalManager, DEFAULT_APPROVAL_TIMEOUT_MS};
use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester, ApprovalResponse};
use crate::sandbox::exec_approval::ApprovalAction;
use crate::sync_primitives::Arc;

/// Maps an `ExecApprovalManager` decision into an `ApprovalOutcome`. `None` =
/// timed out / channel closed. `AllowAlways` collapses to a session grant in
/// Phase 2b (permanent device elevation deferred to Phase 3).
const fn decision_to_outcome(decision: Option<ApprovalDecisionType>) -> ApprovalOutcome {
    match decision {
        Some(ApprovalDecisionType::AllowOnce) => ApprovalOutcome::Approved,
        Some(ApprovalDecisionType::AllowSession) => ApprovalOutcome::ApprovedForSession,
        Some(ApprovalDecisionType::AllowAlways) => ApprovalOutcome::ApprovedForSession,
        Some(ApprovalDecisionType::Deny) => ApprovalOutcome::Denied,
        None => ApprovalOutcome::Timeout,
    }
}

pub struct OperatorApprovalRequester {
    manager: Arc<ExecApprovalManager>,
    event_bus: Arc<GatewayEventBus>,
}

impl OperatorApprovalRequester {
    #[must_use]
    pub const fn new(manager: Arc<ExecApprovalManager>, event_bus: Arc<GatewayEventBus>) -> Self {
        Self { manager, event_bus }
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
        };
        let record = self.manager.create(&request, DEFAULT_APPROVAL_TIMEOUT_MS);
        // Pairing key for the client: which tool row this card belongs under.
        let tool_call_id = record.tool_call_id.clone();
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
                session_key: session_key_str.clone(),
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

        let frame = match decision {
            Some(d) => GatewayEventFrame::ApprovalResolved {
                approval_id,
                session_key: session_key_str,
                decision: d,
                resolved_by: None,
            },
            None => GatewayEventFrame::ApprovalExpired {
                approval_id,
                session_key: session_key_str,
            },
        };
        if let Err(e) = self.event_bus.publish_frame(&frame) {
            tracing::warn!(error = %e, "failed to publish final approval event for config approval");
        }

        ApprovalResponse {
            outcome: decision_to_outcome(decision),
            deny_reason: resolved.deny_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn decision_mapping() {
        assert_eq!(
            decision_to_outcome(Some(ApprovalDecisionType::AllowOnce)),
            ApprovalOutcome::Approved
        );
        assert_eq!(
            decision_to_outcome(Some(ApprovalDecisionType::AllowSession)),
            ApprovalOutcome::ApprovedForSession
        );
        assert_eq!(
            decision_to_outcome(Some(ApprovalDecisionType::AllowAlways)),
            ApprovalOutcome::ApprovedForSession
        );
        assert_eq!(
            decision_to_outcome(Some(ApprovalDecisionType::Deny)),
            ApprovalOutcome::Denied
        );
        assert_eq!(decision_to_outcome(None), ApprovalOutcome::Timeout);
    }
}
