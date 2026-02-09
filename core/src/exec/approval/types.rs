use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::exec::sandbox::capabilities::Capabilities;
use crate::exec::sandbox::parameter_binding::RequiredCapabilities;

/// Trust stage for capability approval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustStage {
    /// Tool just generated, waiting for first approval
    Draft,
    /// Approved, waiting for first execution confirmation
    Trial,
    /// Executed multiple times, entered silent mode
    Verified,
}

/// Reason for escalation trigger
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationReason {
    /// Parameter exceeds custom_paths range
    PathOutOfScope,
    /// Accessing sensitive directory
    SensitiveDirectory,
    /// Using undeclared parameter binding
    UndeclaredBinding,
    /// First execution (Trial stage)
    FirstExecution,
}

/// Escalation trigger information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationTrigger {
    pub reason: EscalationReason,
    pub requested_path: Option<PathBuf>,
    pub approved_paths: Vec<String>,
}

/// Capability approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityApprovalRequest {
    pub tool_name: String,
    pub tool_description: String,
    pub required_capabilities: RequiredCapabilities,
    pub resolved_capabilities: Capabilities,
    pub trust_stage: TrustStage,
}

/// Approval request enum (unified)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalRequest {
    Command(CommandApprovalRequest),
    Capability(CapabilityApprovalRequest),
}

/// Command approval request (placeholder for existing type)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandApprovalRequest {
    pub command: String,
    pub cwd: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trust_stage_progression() {
        let draft = TrustStage::Draft;
        assert!(matches!(draft, TrustStage::Draft));

        let trial = TrustStage::Trial;
        assert!(matches!(trial, TrustStage::Trial));

        let verified = TrustStage::Verified;
        assert!(matches!(verified, TrustStage::Verified));
    }

    #[test]
    fn test_escalation_reason_variants() {
        let reasons = vec![
            EscalationReason::PathOutOfScope,
            EscalationReason::SensitiveDirectory,
            EscalationReason::UndeclaredBinding,
            EscalationReason::FirstExecution,
        ];
        assert_eq!(reasons.len(), 4);
    }

    #[test]
    fn test_capability_approval_request_creation() {
        use crate::exec::sandbox::parameter_binding::CapabilityOverrides;

        let required = RequiredCapabilities {
            base_preset: "file_processor".to_string(),
            description: "Process files in temp directory".to_string(),
            overrides: CapabilityOverrides::default(),
            parameter_bindings: Default::default(),
        };

        let resolved = Capabilities::default();

        let request = CapabilityApprovalRequest {
            tool_name: "test_tool".to_string(),
            tool_description: "A test tool".to_string(),
            required_capabilities: required,
            resolved_capabilities: resolved,
            trust_stage: TrustStage::Draft,
        };

        assert_eq!(request.tool_name, "test_tool");
        assert_eq!(request.trust_stage, TrustStage::Draft);
    }
}
