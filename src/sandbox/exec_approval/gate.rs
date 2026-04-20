use std::collections::HashSet;

use async_trait::async_trait;

use crate::sandbox::exec_approval::parser::parse_approval;
use crate::sandbox::exec_approval::types::{ApprovalAction, ApprovalConfig, ApprovalDecision};
use crate::providers::adapter::ProviderResponse;

#[async_trait]
pub trait ApprovalRequester: Send + Sync {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Approved,
    Denied,
    Timeout,
}

impl ApprovalOutcome {
    pub fn is_approved(&self) -> bool {
        matches!(self, ApprovalOutcome::Approved)
    }
}

pub struct ApprovalGate {
    config: ApprovalConfig,
    requester: Option<Box<dyn ApprovalRequester>>,
    retry_count: u8,
}

impl ApprovalGate {
    pub fn new(config: ApprovalConfig, requester: Option<Box<dyn ApprovalRequester>>) -> Self {
        Self {
            config,
            requester,
            retry_count: 0,
        }
    }

    pub fn with_requester(mut self, requester: Box<dyn ApprovalRequester>) -> Self {
        self.requester = Some(requester);
        self
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
            ApprovalAction::Block { action: _ } => true,
            ApprovalAction::AutoExecute => false,
        }
    }

    pub async fn request_approval_for_tool(
        &self,
        tool_name: &str,
        reason: &str,
    ) -> ApprovalOutcome {
        match &self.requester {
            Some(requester) => requester.request_approval(tool_name, reason).await,
            None => {
                tracing::warn!("No approval requester configured, defaulting to denied");
                ApprovalOutcome::Denied
            }
        }
    }

    pub fn should_retry(&self, decision: &ApprovalDecision) -> bool {
        matches!(
            decision.action,
            ApprovalAction::Block {
                action: crate::sandbox::exec_approval::types::BlockAction::Retry
            }
        ) && self.retry_count < 2
    }

    pub fn record_retry(&mut self) {
        self.retry_count += 1;
    }

    pub fn reset_retry(&mut self) {
        self.retry_count = 0;
    }

    pub fn retry_count(&self) -> u8 {
        self.retry_count
    }
}

pub fn check_always_confirm(tool_name: &str, always_confirm: &HashSet<String>) -> bool {
    always_confirm.contains(tool_name)
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
            stop_reason: Default::default(),
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
        let mut gate = ApprovalGate::new(config, None);
        let response = make_response_with_approval(
            r#"<exec-approval>{"action":"block","block_action":"retry","reason":"alternative"}</exec-approval>"#,
        );
        let decision = gate.parse_and_decide(&response, &[]);
        assert!(gate.should_retry(&decision));
        assert_eq!(gate.retry_count(), 0);
        gate.record_retry();
        assert_eq!(gate.retry_count(), 1);
        assert!(gate.should_retry(&decision));
        gate.record_retry();
        assert_eq!(gate.retry_count(), 2);
        assert!(!gate.should_retry(&decision));
    }

    #[test]
    fn block_notify_triggers_approval() {
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
        assert!(gate.should_request_approval(&decision));
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
}
