//! `OperatorApprovalRequester` — an [`ApprovalRequester`] that routes a config
//! tool approval to the SERVER OPERATOR (not the requesting chat-tier device).
//!
//! Unlike `ChannelApprovalBridgeAdapter` (which delivers back to the
//! requester's own channel), this registers a pending approval in the shared
//! [`ExecApprovalManager`] and publishes a `GatewayEventFrame::Approval*` event
//! that — after the event_scope `approval.` guard — only operator-tier
//! connections receive. The operator resolves it via the existing
//! `exec.approval.resolve` RPC, waking the oneshot. Used by the config-tier gate
//! in `ScopedToolService` (Phase 2b sudo).
//!
//! Scope (Phase 2b): AllowOnce + AllowSession only. `AllowAlways` collapses to a
//! session grant — permanent device elevation is Phase 3.

use async_trait::async_trait;

use crate::exec::analysis::CommandAnalysis;
use crate::exec::decision::ApprovalRequest;
use crate::exec::manager::{ExecApprovalManager, DEFAULT_APPROVAL_TIMEOUT_MS};
use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester};
use crate::sync_primitives::Arc;

/// Maps an `ExecApprovalManager` decision into an `ApprovalOutcome`. `None` =
/// timed out / channel closed. `AllowAlways` collapses to a session grant in
/// Phase 2b (permanent device elevation deferred to Phase 3).
fn decision_to_outcome(decision: Option<ApprovalDecisionType>) -> ApprovalOutcome {
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
    pub fn new(manager: Arc<ExecApprovalManager>, event_bus: Arc<GatewayEventBus>) -> Self {
        Self { manager, event_bus }
    }
}

#[async_trait]
impl ApprovalRequester for OperatorApprovalRequester {
    async fn request_approval(&self, tool_name: &str, _reason: &str) -> ApprovalOutcome {
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
            command: tool_name.to_string(),
            cwd: None,
            analysis: CommandAnalysis {
                ok: true,
                reason: None,
                segments: vec![],
                chains: None,
            },
            agent_id,
            session_key: session_key_str.clone(),
        };
        let record = self.manager.create(&request, DEFAULT_APPROVAL_TIMEOUT_MS);
        let approval_id = record.id.clone();

        if let Err(e) = self
            .event_bus
            .publish_frame(&GatewayEventFrame::ApprovalRequested {
                approval_id: approval_id.clone(),
                session_key: session_key_str.clone(),
                channel_id,
                conversation_id,
            })
        {
            tracing::warn!(error = %e, "failed to publish ApprovalRequested for config approval");
        }

        let decision = self.manager.wait_for_decision(record).await;

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
        let _ = self.event_bus.publish_frame(&frame);

        decision_to_outcome(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
