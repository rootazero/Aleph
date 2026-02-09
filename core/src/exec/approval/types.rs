use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
}
