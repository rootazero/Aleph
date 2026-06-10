use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::sync_primitives::{Mutex, RwLock};

use async_trait::async_trait;

use crate::providers::adapter::ProviderResponse;
use crate::sandbox::exec_approval::parser::parse_approval;
use crate::sandbox::exec_approval::types::{ApprovalAction, ApprovalConfig, ApprovalDecision};

#[async_trait]
pub trait ApprovalRequester: Send + Sync {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// Approved for this single invocation only.
    Approved,
    /// Approved for the remainder of the session (user chose "allow always").
    /// Treated as approved everywhere; additionally recorded in the session
    /// approval memory so the same tool is not re-prompted this session.
    ApprovedForSession,
    Denied,
    Timeout,
}

impl ApprovalOutcome {
    pub fn is_approved(&self) -> bool {
        matches!(
            self,
            ApprovalOutcome::Approved | ApprovalOutcome::ApprovedForSession
        )
    }

    /// True only for a session-scoped grant — the caller should remember it so
    /// subsequent calls to the same tool skip the prompt.
    pub fn is_session_grant(&self) -> bool {
        matches!(self, ApprovalOutcome::ApprovedForSession)
    }
}

pub struct ApprovalGate {
    config: ApprovalConfig,
    /// Swappable so boot can construct the gate before the channel registry
    /// exists, then wire the real requester via `set_requester` once channels
    /// are up. An empty slot denies — never a silent auto-approve.
    requester: RwLock<Option<Arc<dyn ApprovalRequester>>>,
    retry_counts: Mutex<HashMap<String, u8>>,
}

impl ApprovalGate {
    pub fn new(config: ApprovalConfig, requester: Option<Arc<dyn ApprovalRequester>>) -> Self {
        Self {
            config,
            requester: RwLock::new(requester),
            retry_counts: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_requester(self, requester: Arc<dyn ApprovalRequester>) -> Self {
        self.set_requester(requester);
        self
    }

    /// Install (or replace) the approval requester after construction.
    ///
    /// Boot constructs the gate before the channel registry exists; once
    /// channels are up this wires the real `ChannelApprovalBridgeAdapter` so
    /// elevated-capability sandbox escalations can actually reach the user
    /// instead of being auto-denied.
    pub fn set_requester(&self, requester: Arc<dyn ApprovalRequester>) {
        *self.requester.write().unwrap_or_else(|e| e.into_inner()) = Some(requester);
    }

    pub fn parse_and_decide(
        &self,
        response: &ProviderResponse,
        tool_names: &[&str],
    ) -> ApprovalDecision {
        let decision = parse_approval(&response.text);
        let decision = self.apply_safety_floor(decision, tool_names);
        tracing::info!(
            "Approval decision: {:?}, reason: {}",
            decision.action,
            decision.reason
        );
        decision
    }

    fn apply_safety_floor(
        &self,
        mut decision: ApprovalDecision,
        tool_names: &[&str],
    ) -> ApprovalDecision {
        if self.config.always_confirm.is_empty() {
            return decision;
        }

        let needs_override = tool_names
            .iter()
            .any(|name| self.config.always_confirm.iter().any(|c| c == name));

        if needs_override {
            decision.action = ApprovalAction::AskUser;
        }
        decision
    }

    pub fn should_request_approval(&self, decision: &ApprovalDecision) -> bool {
        match decision.action {
            ApprovalAction::AskUser => true,
            ApprovalAction::Block { action: _ } => false,
            ApprovalAction::AutoExecute => false,
        }
    }

    pub async fn request_approval_for_tool(
        &self,
        tool_name: &str,
        reason: &str,
    ) -> ApprovalOutcome {
        // Clone the Arc out of the lock and drop the guard before awaiting —
        // a std `RwLock` guard is not `Send` and must not be held across await.
        let requester = self
            .requester
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match requester {
            Some(requester) => requester.request_approval(tool_name, reason).await,
            None => {
                tracing::warn!("No approval requester configured, defaulting to denied");
                ApprovalOutcome::Denied
            }
        }
    }

    pub fn should_retry(&self, tool_name: &str, decision: &ApprovalDecision) -> bool {
        matches!(
            decision.action,
            ApprovalAction::Block {
                action: crate::sandbox::exec_approval::types::BlockAction::Retry
            }
        ) && self
            .retry_counts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tool_name)
            .copied()
            .unwrap_or(0)
            < 2
    }

    pub fn record_retry(&self, tool_name: &str) {
        *self
            .retry_counts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(tool_name.to_string())
            .or_insert(0) += 1;
    }

    pub fn reset_retry(&self, tool_name: &str) {
        self.retry_counts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(tool_name);
    }

    pub fn retry_count(&self, tool_name: &str) -> u8 {
        self.retry_counts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tool_name)
            .copied()
            .unwrap_or(0)
    }
}

pub fn check_always_confirm(tool_name: &str, always_confirm: &HashSet<String>) -> bool {
    always_confirm.contains(tool_name)
}

/// The gate is itself an [`ApprovalRequester`], delegating to its own
/// late-bound requester via [`request_approval_for_tool`].
///
/// This lets one shared `ApprovalGate` (whose channel requester is wired after
/// channels are up) serve both the sandbox escalation path and the failover
/// route escalation (borrow-cloud) gate, without exposing the inner requester.
///
/// [`request_approval_for_tool`]: ApprovalGate::request_approval_for_tool
#[async_trait]
impl ApprovalRequester for ApprovalGate {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome {
        self.request_approval_for_tool(tool_name, reason).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::exec_approval::types::BlockAction;

    fn make_response_with_approval(text: &str) -> ProviderResponse {
        ProviderResponse {
            text: Some(text.to_string()),
            tool_calls: vec![],
            thinking: None,
            thinking_signature: None,
            stop_reason: Default::default(),
            truncated_tool_call: None,
            usage: None,
        }
    }

    #[test]
    fn auto_execute_decision_no_always_confirm() {
        let config = ApprovalConfig::default();
        let gate = ApprovalGate::new(config, None);
        let response = make_response_with_approval(
            r#"<exec-approval>{"action":"auto_execute","reason":"safe"}</exec-approval>"#,
        );
        let decision = gate.parse_and_decide(&response, &[]);
        assert!(matches!(decision.action, ApprovalAction::AutoExecute));
    }

    #[test]
    fn ask_user_decision_triggers_approval() {
        let config = ApprovalConfig::default();
        let gate = ApprovalGate::new(config, None);
        let response = make_response_with_approval(
            r#"<exec-approval>{"action":"ask_user","reason":"uncertain"}</exec-approval>"#,
        );
        let decision = gate.parse_and_decide(&response, &[]);
        assert!(matches!(decision.action, ApprovalAction::AskUser));
        assert!(gate.should_request_approval(&decision));
    }

    #[test]
    fn always_confirm_overrides_auto_execute() {
        let mut config = ApprovalConfig::default();
        config.always_confirm.insert("bash_exec".to_string());
        let gate = ApprovalGate::new(config, None);
        let response = make_response_with_approval(
            r#"<exec-approval>{"action":"auto_execute","reason":"safe"}</exec-approval>"#,
        );
        let decision = gate.parse_and_decide(&response, &["bash_exec"]);
        assert!(matches!(decision.action, ApprovalAction::AskUser));
    }

    #[test]
    fn block_retry_with_count() {
        let config = ApprovalConfig::default();
        let gate = ApprovalGate::new(config, None);
        let response = make_response_with_approval(
            r#"<exec-approval>{"action":"block","block_action":"retry","reason":"alternative"}</exec-approval>"#,
        );
        let decision = gate.parse_and_decide(&response, &[]);
        assert!(gate.should_retry("test_tool", &decision));
        assert_eq!(gate.retry_count("test_tool"), 0);
        gate.record_retry("test_tool");
        assert_eq!(gate.retry_count("test_tool"), 1);
        assert!(gate.should_retry("test_tool", &decision));
        gate.record_retry("test_tool");
        assert_eq!(gate.retry_count("test_tool"), 2);
        assert!(!gate.should_retry("test_tool", &decision));
    }

    #[test]
    fn block_notify_does_not_trigger_approval() {
        let config = ApprovalConfig::default();
        let gate = ApprovalGate::new(config, None);
        let response = make_response_with_approval(
            r#"<exec-approval>{"action":"block","block_action":"notify","reason":"dangerous"}</exec-approval>"#,
        );
        let decision = gate.parse_and_decide(&response, &[]);
        assert!(matches!(
            decision.action,
            ApprovalAction::Block {
                action: BlockAction::Notify
            }
        ));
        assert!(!gate.should_request_approval(&decision));
    }

    #[test]
    fn missing_tags_defaults_to_ask_user() {
        let config = ApprovalConfig::default();
        let gate = ApprovalGate::new(config, None);
        let response = make_response_with_approval("Some reasoning without tags");
        let decision = gate.parse_and_decide(&response, &[]);
        assert!(matches!(decision.action, ApprovalAction::AskUser));
    }

    #[test]
    fn approval_outcome_is_approved() {
        assert!(ApprovalOutcome::Approved.is_approved());
        assert!(!ApprovalOutcome::Denied.is_approved());
        assert!(!ApprovalOutcome::Timeout.is_approved());
    }

    /// Regression: the gate denies when no requester is wired (the CRITICAL
    /// boot bug), and routes to the requester once `set_requester` installs
    /// one — the post-construction wiring boot now performs.
    #[tokio::test]
    async fn set_requester_makes_escalation_reach_the_requester() {
        struct AlwaysApprove;
        #[async_trait::async_trait]
        impl ApprovalRequester for AlwaysApprove {
            async fn request_approval(&self, _tool: &str, _reason: &str) -> ApprovalOutcome {
                ApprovalOutcome::Approved
            }
        }

        let gate = ApprovalGate::new(ApprovalConfig::default(), None);
        // No requester wired → denied (never a silent auto-approve).
        assert_eq!(
            gate.request_approval_for_tool("code_exec", "allow_network")
                .await,
            ApprovalOutcome::Denied
        );
        // Once the requester is installed, escalations reach it.
        gate.set_requester(Arc::new(AlwaysApprove));
        assert_eq!(
            gate.request_approval_for_tool("code_exec", "allow_network")
                .await,
            ApprovalOutcome::Approved
        );
    }
}
