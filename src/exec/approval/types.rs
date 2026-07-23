use crate::exec::approval::parameter_binding::RequiredCapabilities;
use crate::sandbox::capabilities::SandboxCapabilities;
use serde::{Deserialize, Serialize};

/// Trust stage for capability approval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum TrustStage {
    /// Tool just generated, waiting for first approval
    Draft,
    /// Approved, waiting for first execution confirmation
    Trial,
    /// Executed multiple times, entered silent mode
    Verified,
}

/// Capability approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityApprovalRequest {
    pub tool_name: String,
    pub tool_description: String,
    pub required_capabilities: RequiredCapabilities,
    pub resolved_capabilities: SandboxCapabilities,
    pub trust_stage: TrustStage,
}

/// Approval request enum (unified)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalRequest {
    Command(CommandApprovalRequest),
    Capability(Box<CapabilityApprovalRequest>),
}

/// Command approval request (placeholder for existing type)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandApprovalRequest {
    pub command: String,
    pub cwd: Option<String>,
    /// Why this approval is being requested (escalation/confirmation context).
    /// Rendered to the user so they can make an informed decision; absent on
    /// payloads serialized before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The decision tiers this request permits (see
    /// [`crate::exec::allowed_decisions`]). Renderers consult this instead of
    /// hardcoding a button set; the serde default backfills pre-existing
    /// payloads with the historical unconstrained set.
    #[serde(default = "crate::exec::allowed_decisions::full_set")]
    pub allowed_decisions: Vec<crate::exec::socket::ApprovalDecisionType>,
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
    fn test_capability_approval_request_creation() {
        use crate::exec::approval::parameter_binding::CapabilityOverrides;

        let required = RequiredCapabilities {
            base_preset: "file_processor".to_string(),
            description: "Process files in temp directory".to_string(),
            overrides: CapabilityOverrides::default(),
            parameter_bindings: Default::default(),
        };

        let resolved = SandboxCapabilities::default();

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
